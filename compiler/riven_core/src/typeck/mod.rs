//! Type checking orchestration for the Riven compiler.
//!
//! This module coordinates name resolution, type inference, trait resolution,
//! and coercion checking to produce a fully type-checked HIR.

pub mod coerce;
pub mod infer;
mod method_resolvers;
pub mod mixins;
#[cfg(test)]
mod tests;
pub mod unify;

use crate::diagnostics::Diagnostic;
use crate::hir::context::TypeContext;
use crate::hir::nodes::HirProgram;
use crate::parser::ast;
use crate::resolve::symbols::SymbolTable;
use crate::resolve::{ResolveResult, Resolver};
use infer::InferenceEngine;
use mixins::MixinResolver;

/// The result of full type checking.
pub struct TypeCheckResult {
    pub program: HirProgram,
    pub symbols: SymbolTable,
    pub type_context: TypeContext,
    pub diagnostics: Vec<Diagnostic>,
}

/// Run the full type checking pipeline on a parsed AST program.
///
/// Pipeline:
/// 1. Stdlib bootstrap load (parses `library/std/<pkg>/src/lib.rvn` once per call)
/// 2. Name resolution (AST → HIR with DefIds, unresolved types)
/// 3. Trait/impl collection
/// 4. Type inference (resolve all Infer types)
/// 5. Final validation (check no unresolved types remain)
///
/// Wave 2 (#06.8): stdlib modules like `std.rand` now live in `.rvn`
/// files loaded by [`crate::resolve::bootstrap::run_bootstrap`]. Test
/// harnesses that call this function pick up the prelude automatically;
/// callers that *don't* want the prelude (rare — pure compiler-internal
/// tests) should use [`type_check_with_bootstrap`] with an explicit
/// empty slice instead.
pub fn type_check(program: &ast::Program) -> TypeCheckResult {
    let mut bootstrap_diagnostics: Vec<crate::diagnostics::Diagnostic> = Vec::new();
    // Phase D of #06.95: drive bootstrap through the package-aware
    // path so each `std.<pkg>` submodule's `items` list is
    // auto-populated from the matching `library/std/<pkg>/src/lib.rvn`
    // — no hand-maintained FIXUPS row required per package.
    let bootstrap_packages =
        crate::resolve::bootstrap::run_bootstrap_with_package_names(&mut bootstrap_diagnostics);
    let mut result = type_check_with_bootstrap_packages(program, &bootstrap_packages);
    result.diagnostics.extend(bootstrap_diagnostics);
    result
}

/// Package-aware variant of [`type_check_with_bootstrap`]. Each
/// element of `bootstrap_packages` carries the package name (e.g.
/// `"io"`) alongside its parsed `Program`, so the resolver can
/// auto-populate each `std.<pkg>` submodule from the matching
/// `library/std/<pkg>/src/lib.rvn` — the table-driven fixup is no
/// longer required for the per-package bulk case.
pub fn type_check_with_bootstrap_packages(
    program: &ast::Program,
    bootstrap_packages: &[(String, ast::Program)],
) -> TypeCheckResult {
    let resolver = Resolver::new();
    let ResolveResult {
        mut program,
        mut symbols,
        mut type_context,
        mut diagnostics,
    } = resolver.resolve_with_bootstrap_packages(program, bootstrap_packages);

    diagnostics.extend(crate::implicit_includes::validate_program(
        &program, &symbols,
    ));

    let mut trait_resolver = MixinResolver::new();
    trait_resolver.collect_impls(&program, &symbols);

    let mut engine = InferenceEngine::new(&mut type_context, &mut symbols, &trait_resolver);
    engine.infer_program(&mut program);
    diagnostics.extend(engine.diagnostics);

    resolve_all_types(&mut program, &type_context);
    diagnostics.extend(validate(&program, &symbols, &type_context));

    TypeCheckResult {
        program,
        symbols,
        type_context,
        diagnostics,
    }
}

/// Type-check a parsed user `program` with stdlib `bootstrap_programs`
/// merged into the resolver scope before user-code resolution. The
/// driver (`rivenc`) calls this with the output of
/// `riven_core::resolve::bootstrap::run_bootstrap`; callers that don't
/// want the prelude (today: most tests) keep using
/// [`type_check`](type_check) and get an empty-bootstrap shortcut for
/// free.
///
/// The pipeline is identical to `type_check` once name resolution is
/// done — bootstrap programs only contribute to Phase 1.
pub fn type_check_with_bootstrap(
    program: &ast::Program,
    bootstrap_programs: &[ast::Program],
) -> TypeCheckResult {
    // Phase 1: Name resolution (with bootstrap prelude merged in)
    let resolver = Resolver::new();
    let ResolveResult {
        mut program,
        mut symbols,
        mut type_context,
        mut diagnostics,
    } = resolver.resolve_with_bootstrap(program, bootstrap_programs);

    // Phase 2: Validate derive usage and collect all trait impls
    diagnostics.extend(crate::implicit_includes::validate_program(
        &program, &symbols,
    ));

    let mut trait_resolver = MixinResolver::new();
    trait_resolver.collect_impls(&program, &symbols);

    // Phase 3: Type inference
    let mut engine = InferenceEngine::new(&mut type_context, &mut symbols, &trait_resolver);
    engine.infer_program(&mut program);
    diagnostics.extend(engine.diagnostics);

    // Phase 4: Final resolution pass — resolve all remaining inference variables
    resolve_all_types(&mut program, &type_context);

    // Phase 5: Validation — check for unresolved types, missing annotations, etc.
    let validation_diags = validate(&program, &symbols, &type_context);
    diagnostics.extend(validation_diags);

    TypeCheckResult {
        program,
        symbols,
        type_context,
        diagnostics,
    }
}

/// Final pass: resolve all remaining inference variables in the HIR.
fn resolve_all_types(program: &mut HirProgram, ctx: &TypeContext) {
    for item in &mut program.items {
        resolve_item_types(item, ctx);
    }
}

fn resolve_item_types(item: &mut crate::hir::nodes::HirItem, ctx: &TypeContext) {
    use crate::hir::nodes::HirItem;
    match item {
        HirItem::Class(class) => {
            for field in &mut class.fields {
                field.ty = ctx.resolve(&field.ty);
            }
            for method in &mut class.methods {
                resolve_func_types(method, ctx);
            }
            for imp in &mut class.impl_blocks {
                for ii in &mut imp.items {
                    if let crate::hir::nodes::HirImplItem::Method(m) = ii {
                        resolve_func_types(m, ctx);
                    }
                }
            }
        }
        HirItem::Impl(imp) => {
            for ii in &mut imp.items {
                if let crate::hir::nodes::HirImplItem::Method(m) = ii {
                    resolve_func_types(m, ctx);
                }
            }
        }
        HirItem::Function(func) => resolve_func_types(func, ctx),
        HirItem::Module(m) => {
            for sub in &mut m.items {
                resolve_item_types(sub, ctx);
            }
        }
        HirItem::Const(c) => {
            c.ty = ctx.resolve(&c.ty);
            resolve_expr_types(&mut c.value, ctx);
        }
        HirItem::Struct(s) => {
            for field in &mut s.fields {
                field.ty = ctx.resolve(&field.ty);
            }
        }
        HirItem::Enum(e) => {
            for variant in &mut e.variants {
                match &mut variant.kind {
                    crate::hir::nodes::HirVariantKind::Tuple(fields)
                    | crate::hir::nodes::HirVariantKind::Struct(fields) => {
                        for field in fields {
                            field.ty = ctx.resolve(&field.ty);
                        }
                    }
                    crate::hir::nodes::HirVariantKind::Unit => {}
                }
            }
        }
        HirItem::Mixin(t) => {
            for item in &mut t.items {
                match item {
                    crate::hir::nodes::HirMixinItem::MethodSig {
                        return_ty, params, ..
                    } => {
                        *return_ty = ctx.resolve(return_ty);
                        for p in params {
                            p.ty = ctx.resolve(&p.ty);
                        }
                    }
                    crate::hir::nodes::HirMixinItem::DefaultMethod(m) => {
                        resolve_func_types(m, ctx);
                    }
                    _ => {}
                }
            }
        }
        HirItem::TypeAlias(ta) => {
            ta.ty = ctx.resolve(&ta.ty);
        }
        HirItem::Newtype(nt) => {
            nt.inner_ty = ctx.resolve(&nt.inner_ty);
        }
    }
}

fn resolve_func_types(func: &mut crate::hir::nodes::HirFuncDef, ctx: &TypeContext) {
    func.return_ty = ctx.resolve(&func.return_ty);
    for param in &mut func.params {
        param.ty = ctx.resolve(&param.ty);
    }
    resolve_expr_types(&mut func.body, ctx);
}

fn resolve_expr_types(expr: &mut crate::hir::nodes::HirExpr, ctx: &TypeContext) {
    expr.ty = ctx.resolve(&expr.ty);
    use crate::hir::nodes::HirExprKind::*;
    match &mut expr.kind {
        Block(stmts, tail) => {
            for stmt in stmts {
                match stmt {
                    crate::hir::nodes::HirStatement::Let { ty, value, .. } => {
                        *ty = ctx.resolve(ty);
                        if let Some(ref mut v) = value {
                            resolve_expr_types(v, ctx);
                        }
                    }
                    crate::hir::nodes::HirStatement::Expr(e) => resolve_expr_types(e, ctx),
                }
            }
            if let Some(ref mut t) = tail {
                resolve_expr_types(t, ctx);
            }
        }
        BinaryOp { left, right, .. } => {
            resolve_expr_types(left, ctx);
            resolve_expr_types(right, ctx);
        }
        UnaryOp { operand, .. } => resolve_expr_types(operand, ctx),
        Borrow { expr: inner, .. } => resolve_expr_types(inner, ctx),
        If {
            cond,
            then_branch,
            else_branch,
        } => {
            resolve_expr_types(cond, ctx);
            resolve_expr_types(then_branch, ctx);
            if let Some(ref mut e) = else_branch {
                resolve_expr_types(e, ctx);
            }
        }
        Match { scrutinee, arms } => {
            resolve_expr_types(scrutinee, ctx);
            for arm in arms {
                resolve_expr_types(&mut arm.body, ctx);
                if let Some(ref mut g) = arm.guard {
                    resolve_expr_types(g, ctx);
                }
            }
        }
        While { condition, body } => {
            resolve_expr_types(condition, ctx);
            resolve_expr_types(body, ctx);
        }
        Loop { body } => resolve_expr_types(body, ctx),
        For { iterable, body, .. } => {
            resolve_expr_types(iterable, ctx);
            resolve_expr_types(body, ctx);
        }
        MethodCall {
            object,
            args,
            block,
            ..
        } => {
            resolve_expr_types(object, ctx);
            for arg in args {
                resolve_expr_types(arg, ctx);
            }
            if let Some(ref mut b) = block {
                resolve_expr_types(b, ctx);
            }
        }
        FnCall { args, .. } => {
            for arg in args {
                resolve_expr_types(arg, ctx);
            }
        }
        FieldAccess { object, .. } => resolve_expr_types(object, ctx),
        Assign { target, value, .. } => {
            resolve_expr_types(target, ctx);
            resolve_expr_types(value, ctx);
        }
        CompoundAssign { target, value, .. } => {
            resolve_expr_types(target, ctx);
            resolve_expr_types(value, ctx);
        }
        Return(Some(v)) | Break(Some(v)) => {
            resolve_expr_types(v, ctx);
        }
        Return(None) | Break(None) => {}
        Closure { body, params, .. } => {
            for p in params {
                p.ty = ctx.resolve(&p.ty);
            }
            resolve_expr_types(body, ctx);
        }
        Construct { fields, .. } => {
            for (_, e) in fields {
                resolve_expr_types(e, ctx);
            }
        }
        EnumVariant { fields, .. } => {
            for (_, e) in fields {
                resolve_expr_types(e, ctx);
            }
        }
        Tuple(elems) | ArrayLiteral(elems) => {
            for e in elems {
                resolve_expr_types(e, ctx);
            }
        }
        ArrayFill { value, .. } => resolve_expr_types(value, ctx),
        Index { object, index } => {
            resolve_expr_types(object, ctx);
            resolve_expr_types(index, ctx);
        }
        Cast { expr: inner, .. } => resolve_expr_types(inner, ctx),
        Range { start, end, .. } => {
            if let Some(ref mut s) = start {
                resolve_expr_types(s, ctx);
            }
            if let Some(ref mut e) = end {
                resolve_expr_types(e, ctx);
            }
        }
        Interpolation { parts } => {
            for p in parts {
                if let crate::hir::nodes::HirInterpolationPart::Expr {
                    expr: ref mut e, ..
                } = p
                {
                    resolve_expr_types(e, ctx);
                }
            }
        }
        MacroCall { args, .. } => {
            for a in args {
                resolve_expr_types(a, ctx);
            }
        }
        _ => {}
    }
}

/// Validate the type-checked program.
fn validate(program: &HirProgram, symbols: &SymbolTable, ctx: &TypeContext) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for item in &program.items {
        validate_item(item, symbols, ctx, &mut diags);
    }
    diags
}

fn validate_item(
    item: &crate::hir::nodes::HirItem,
    symbols: &SymbolTable,
    ctx: &TypeContext,
    diags: &mut Vec<Diagnostic>,
) {
    use crate::hir::nodes::HirItem;
    match item {
        HirItem::Function(func) => validate_func(func, symbols, ctx, diags),
        HirItem::Class(class) => {
            for method in &class.methods {
                validate_func(method, symbols, ctx, diags);
            }
            for imp in &class.impl_blocks {
                for ii in &imp.items {
                    if let crate::hir::nodes::HirImplItem::Method(m) = ii {
                        validate_func(m, symbols, ctx, diags);
                    }
                }
            }
        }
        HirItem::Impl(imp) => {
            for ii in &imp.items {
                if let crate::hir::nodes::HirImplItem::Method(m) = ii {
                    validate_func(m, symbols, ctx, diags);
                }
            }
        }
        HirItem::Module(m) => {
            for sub in &m.items {
                validate_item(sub, symbols, ctx, diags);
            }
        }
        _ => {}
    }
}

fn validate_func(
    func: &crate::hir::nodes::HirFuncDef,
    _symbols: &SymbolTable,
    ctx: &TypeContext,
    diags: &mut Vec<Diagnostic>,
) {
    // Check that public functions have explicit annotations (already done in infer)
    // Check that no Infer types remain in the signature
    if !ctx.is_fully_resolved(&func.return_ty) {
        diags.push(Diagnostic::error(
            format!("could not infer return type for function `{}`", func.name),
            func.span.clone(),
        ));
    }
    for param in &func.params {
        if !ctx.is_fully_resolved(&param.ty) {
            diags.push(Diagnostic::error(
                format!(
                    "could not infer type for parameter `{}` in function `{}`",
                    param.name, func.name
                ),
                param.span.clone(),
            ));
        }
    }
}
