//! Type checking orchestration for the Ruxen compiler.
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
    /// Raw identifiers `resolve` could not resolve, surfaced structurally so
    /// the autoload fixpoint matches registrations by name (see `ResolveResult`).
    pub unresolved_names: Vec<String>,
}

/// Type-check `program` against the hosted stdlib: the front end
/// ([`crate::driver::check`]) run to [`crate::driver::Stage::Typeck`], with
/// any stdlib bootstrap diagnostic appended to the diagnostics.
pub fn type_check(program: &ast::Program) -> TypeCheckResult {
    let options = crate::driver::Options {
        stop_after: crate::driver::Stage::Typeck,
        ..Default::default()
    };
    crate::driver::check(program, &options).into_type_check_result()
}

/// Resolve and type-check `program` (already async-lowered) against
/// `bootstrap_packages`. Each element carries the package name (e.g.
/// `"io"`) alongside its parsed `Program`, so the resolver can
/// auto-populate each `std.<pkg>` submodule from the matching
/// `library/std/<pkg>/src/lib.rx`. [`crate::driver::check`] runs the whole
/// front end around it (implicit `main`, async lowering) and is the one
/// production caller.
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
        unresolved_names,
        type_registry,
        method_aliases,
    } = resolver.resolve_with_bootstrap_packages(program, bootstrap_packages);

    diagnostics.extend(crate::implicit_includes::validate_program(
        &program, &symbols,
    ));

    let mut trait_resolver = MixinResolver::new();
    trait_resolver.collect_impls(&program, &symbols);
    // Phase E.E of #06.95: also walk the type_registry so module-
    // nested bootstrap classes (e.g. `BufReader.File`) get their
    // methods registered under their qualified type name. The
    // user-program walk above only sees HIR items lowered from the
    // user's AST; bootstrap items live in the symbol table but not
    // in `program.items`, so they'd otherwise be invisible.
    trait_resolver.register_classes_from_registry(&type_registry, &symbols);
    // Ruby `alias new old` method synonyms (docs/decisions/alias-keyword.md):
    // register each alias name with the canonical method's signature so a call
    // via the alias type-checks. Run AFTER the canonical methods are present.
    trait_resolver.register_method_aliases(&method_aliases);

    let mut engine = InferenceEngine::new(&mut type_context, &mut symbols, &trait_resolver);
    engine.infer_program(&mut program);
    diagnostics.extend(engine.diagnostics);

    resolve_all_types(&mut program, &type_context);
    settle_trailing_block_kinds(&mut program, &symbols, &trait_resolver);
    diagnostics.extend(validate(&program, &symbols, &type_context));

    TypeCheckResult {
        program,
        symbols,
        type_context,
        diagnostics,
        unresolved_names,
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
            // Finalize inferred types in inline methods / impl-block methods
            // (ruby-naming.spec.md §3.4a) — without this, struct-method
            // bodies keep `Infer(_)` types after inference and codegen
            // mis-lowers field reads.
            for method in &mut s.methods {
                resolve_func_types(method, ctx);
            }
            for imp in &mut s.impl_blocks {
                for ii in &mut imp.items {
                    if let crate::hir::nodes::HirImplItem::Method(m) = ii {
                        resolve_func_types(m, ctx);
                    }
                }
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
            for method in &mut e.methods {
                resolve_func_types(method, ctx);
            }
            for imp in &mut e.impl_blocks {
                for ii in &mut imp.items {
                    if let crate::hir::nodes::HirImplItem::Method(m) = ii {
                        resolve_func_types(m, ctx);
                    }
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
    use crate::hir::visit::VisitMut;
    TyResolver { ctx }.visit_func_mut(func);
}

fn resolve_expr_types(expr: &mut crate::hir::nodes::HirExpr, ctx: &TypeContext) {
    use crate::hir::visit::VisitMut;
    TyResolver { ctx }.visit_expr_mut(expr);
}

/// Replaces every type the shared HIR walk reaches (expression, `let`,
/// closure-parameter, cast-target, method generic-argument and signature
/// types) with its resolved form.
struct TyResolver<'c> {
    ctx: &'c TypeContext,
}

impl crate::hir::visit::VisitMut for TyResolver<'_> {
    fn visit_ty_mut(&mut self, ty: &mut crate::hir::types::Ty) {
        *ty = self.ctx.resolve(ty);
    }
}

/// Blocks, procs and lambdas: the parser marks every trailing block
/// `ClosureKind::Block`; the parameter it binds to decides what it is. Runs
/// after [`resolve_all_types`], over every function body. The settled
/// `HirExprKind::Closure::kind` is the single source of truth later stages
/// read (borrow check: owning vs borrowed captures; MIR: capture drops).
///
/// - A parameter for which [`Ty::is_borrowed_block`] holds (`Fn[...]`, incl.
///   `&block` and the implicit-yield `__block`) only calls the block, so it
///   stays a Block and borrows its captures.
/// - Any other parameter type (`Proc[...]` / `any Fn[...]`, a type parameter)
///   may keep it, so it becomes a Proc that owns its captures.
/// - A callee whose signature is unknown (no recorded def, no mixin default
///   of that name on the receiver) also gets a Proc: owning is the choice
///   that can never free a capture twice. Pinned by
///   `blocks_procs_lambdas::semantics_unknown_signature_block_is_proc`.
///
/// A `FnCall` carries its trailing block as the argument at the block
/// parameter's index; a `MethodCall` keeps it in `block`, bound to the final
/// parameter of `resolved`, the method inference selected (own, inherited,
/// or the mixin / `Kernel` / type-param-bound default it typed the call
/// with). Only a call inference recorded no def for (a bodiless bound
/// requirement, an unknown receiver) looks the signature up by name on the
/// receiver; a default found that way is written to `resolved` too, so the
/// borrow checker reads the same signature when the block slot holds a
/// forwarded `&block` instead of a literal.
///
/// [`Ty::is_borrowed_block`]: crate::hir::types::Ty::is_borrowed_block
fn settle_trailing_block_kinds(
    program: &mut HirProgram,
    symbols: &SymbolTable,
    traits: &MixinResolver,
) {
    use crate::hir::visit::VisitMut;
    let mut settler = BlockKindSettler { symbols, traits };
    for item in &mut program.items {
        settler.visit_item_mut(item);
    }
}

struct BlockKindSettler<'c> {
    symbols: &'c SymbolTable,
    traits: &'c MixinResolver,
}

impl crate::hir::visit::VisitMut for BlockKindSettler<'_> {
    fn visit_expr_mut(&mut self, expr: &mut crate::hir::nodes::HirExpr) {
        crate::hir::visit::walk_expr_mut(self, expr);
        self.settle_call(expr);
    }
}

impl BlockKindSettler<'_> {
    fn settle_call(&self, expr: &mut crate::hir::nodes::HirExpr) {
        use crate::hir::nodes::{HirExprKind, UNRESOLVED_DEF};
        let signature = |def| self.symbols.get(def).and_then(|d| d.kind.signature());

        match &mut expr.kind {
            HirExprKind::MethodCall {
                object,
                method,
                resolved,
                method_name,
                block: Some(block),
                ..
            } => {
                if *resolved == UNRESOLVED_DEF && *method != UNRESOLVED_DEF {
                    *resolved = *method;
                }
                let param_ty = match signature(*resolved) {
                    Some(sig) => sig.params.last().map(|p| p.ty.clone()),
                    None => {
                        let found = self
                            .traits
                            .lookup_block_method_by_name(object.ty.peel_refs(), method_name);
                        if let Some((_, Some(default_def))) = &found {
                            *resolved = *default_def;
                        }
                        found.and_then(|(sig, _)| sig.params.last().map(|p| p.ty.clone()))
                    }
                };
                settle_block(block, param_ty.as_ref());
            }
            HirExprKind::FnCall { callee, args, .. } => {
                let params = signature(*callee)
                    .map(|s| s.params.as_slice())
                    .unwrap_or(&[]);
                for (idx, arg) in args.iter_mut().enumerate() {
                    settle_block(arg, params.get(idx).map(|p| &p.ty));
                }
            }
            _ => {}
        }
    }
}

/// Settle one trailing block bound to a parameter of type `param_ty`
/// (`None`: unknown). Closures the parser did not mark `Block` (lambdas,
/// procs) and non-closure arguments are left alone.
fn settle_block(
    closure: &mut crate::hir::nodes::HirExpr,
    param_ty: Option<&crate::hir::types::Ty>,
) {
    use crate::hir::nodes::HirExprKind;
    use crate::parser::ast::ClosureKind;
    if let HirExprKind::Closure { kind, .. } = &mut closure.kind {
        if *kind == ClosureKind::Block && !param_ty.is_some_and(|ty| ty.is_borrowed_block()) {
            *kind = ClosureKind::Proc;
        }
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
            validate_runtime_dispatch_includes(class, symbols, diags);
        }
        HirItem::Impl(imp) => {
            for ii in &imp.items {
                if let crate::hir::nodes::HirImplItem::Method(m) = ii {
                    validate_func(m, symbols, ctx, diags);
                }
            }
        }
        HirItem::Struct(s) => {
            for method in &s.methods {
                validate_func(method, symbols, ctx, diags);
            }
            for imp in &s.impl_blocks {
                for ii in &imp.items {
                    if let crate::hir::nodes::HirImplItem::Method(m) = ii {
                        validate_func(m, symbols, ctx, diags);
                    }
                }
            }
        }
        HirItem::Enum(e) => {
            for method in &e.methods {
                validate_func(method, symbols, ctx, diags);
            }
            for imp in &e.impl_blocks {
                for ii in &imp.items {
                    if let crate::hir::nodes::HirImplItem::Method(m) = ii {
                        validate_func(m, symbols, ctx, diags);
                    }
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

/// Mixin vtables spec §B1 / Phase A — enforce that classes including
/// a `dispatch runtime` mixin actually implement all of its required
/// methods. Statically-dispatched mixins keep today's permissive
/// structural-satisfaction behaviour (the existing code paths handle
/// them); only runtime-dispatch mixins require a complete method
/// table because the vtable (Phase B/C) cannot dispatch missing
/// methods at runtime.
///
/// Emits **E1117** with the list of missing methods.
fn validate_runtime_dispatch_includes(
    class: &crate::hir::nodes::HirClassDef,
    symbols: &SymbolTable,
    diags: &mut Vec<Diagnostic>,
) {
    use crate::resolve::symbols::DefKind;

    // Collect the names of every method available on this class —
    // user-body methods (already in `class.methods`) plus any
    // `DefKind::Method` whose parent is this class's def_id (covers
    // lib-decl-registered methods, derive-synthesised methods, etc.).
    let mut have: std::collections::HashSet<String> =
        class.methods.iter().map(|m| m.name.clone()).collect();
    for def in symbols.children_of(class.def_id) {
        if let DefKind::Method { .. } = &def.kind {
            have.insert(def.name.clone());
        }
    }
    // Inner impl-block methods carry their own method names — count
    // those as "provided" as well so `include M; def m() ... end` in
    // an inline impl-block satisfies the requirement.
    for imp in &class.impl_blocks {
        for ii in &imp.items {
            if let crate::hir::nodes::HirImplItem::Method(m) = ii {
                have.insert(m.name.clone());
            }
        }
    }

    for imp in &class.impl_blocks {
        let Some(trait_ref) = &imp.trait_ref else {
            continue;
        };
        // Look up the mixin def to find its dispatch mode + required
        // methods. Skip if the name doesn't resolve (an earlier pass
        // already emitted an "unknown mixin" diagnostic).
        let mut info_opt = None;
        for def in symbols.named(&trait_ref.name) {
            if let DefKind::Trait { info } = &def.kind {
                info_opt = Some(info);
                break;
            }
        }
        let Some(info) = info_opt else {
            continue;
        };
        if !matches!(info.dispatch_mode, ast::DispatchMode::Runtime) {
            continue;
        }

        // Default-method names count as "provided" — the mixin body
        // already supplied them.
        let mut effectively_have = have.clone();
        for d in &info.default_methods {
            effectively_have.insert(d.clone());
        }

        let missing: Vec<&str> = info
            .required_methods
            .iter()
            .filter(|m| !effectively_have.contains(m.as_str()))
            .map(|m| m.as_str())
            .collect();
        if missing.is_empty() {
            continue;
        }
        let list = missing.join("`, `");
        diags.push(Diagnostic::error_with_code(
            format!(
                "class `{}` includes runtime-dispatch mixin `{}` but is missing required method(s): `{}`",
                class.name, trait_ref.name, list
            ),
            imp.span.clone(),
            "E1117",
        ));
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
        diags.push(Diagnostic::error_with_code(
            format!("could not infer return type for function `{}`", func.name),
            func.span.clone(),
            "E0736",
        ));
    }
    for param in &func.params {
        if !ctx.is_fully_resolved(&param.ty) {
            diags.push(Diagnostic::error_with_code(
                format!(
                    "could not infer type for parameter `{}` in function `{}`",
                    param.name, func.name
                ),
                param.span.clone(),
                "E0736",
            ));
        }
    }
}
