//! `no_std` validation (tier 4.04).
//!
//! A `no_std` compilation unit promises "I provide the host environment: don't
//! assume `malloc`, don't link libc." This pass walks the typed HIR and rejects
//! every heap-allocating construction with **E1400** — because in a no_std
//! build there is no allocator wired (`__ruxen_global_allocator` is the staged
//! remainder), so an `Array`/`String`/`Map`/`Set` construction would either
//! fail to link or silently allocate against a libc that isn't there.
//!
//! ## What counts as a heap allocation (E1400)
//!
//! The unambiguous allocation sites: a `String` literal / interpolation, an
//! `Array`/`Map` literal or fill, and any call (method or free fn) whose result
//! type is one of the heap collection types (`String`/`Array`/`Map`/`Set`) —
//! e.g. `Array.new`, `"x".to_s`, `map.clone`. Borrows (`&String`), reads, and
//! pass-throughs do NOT allocate and are allowed: the check keys on the
//! *construction kind* and/or the *result type being a freshly-produced heap
//! value*, not on a heap type merely appearing.
//!
//! Scope (v1): this is the enforcement half of no_std. The `core` package
//! surface, the `alloc` tier (heap types *with* a user `global_allocator`),
//! and the `no_std` source directive are the staged remainder —
//! `docs/decisions/phase4-no-std-wasm.md`. v1 drives the bar: a no_std unit
//! that constructs a heap value is rejected with a clean, located diagnostic.

use crate::diagnostics::Diagnostic;
use crate::hir::nodes::{HirExpr, HirExprKind, HirItem, HirProgram, HirStatement};
use crate::hir::types::Ty;

/// Is `ty` a heap-allocated collection/string whose construction routes through
/// the (absent-in-no_std) allocator?
fn is_heap_alloc_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::String | Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_))
}

/// Validate a typed HIR program under no_std rules. Returns one E1400 per
/// heap-allocating construction found in user code.
pub fn validate(program: &HirProgram) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for item in &program.items {
        validate_item(item, &mut diags);
    }
    diags
}

fn validate_item(item: &HirItem, diags: &mut Vec<Diagnostic>) {
    match item {
        HirItem::Function(f) => validate_expr(&f.body, diags),
        HirItem::Class(c) => {
            for m in &c.methods {
                validate_expr(&m.body, diags);
            }
        }
        HirItem::Impl(b) => {
            for it in &b.items {
                if let crate::hir::nodes::HirImplItem::Method(m) = it {
                    validate_expr(&m.body, diags);
                }
            }
        }
        HirItem::Module(m) => {
            for it in &m.items {
                validate_item(it, diags);
            }
        }
        // Structs/enums/mixins/aliases/newtypes/consts carry no executable
        // user body that allocates at the surface; const initializers that
        // allocate are out of scope for v1 (they'd need const-eval anyway).
        _ => {}
    }
}

/// Push an E1400 for a heap-allocating construction `expr` describes as `what`.
fn flag(expr: &HirExpr, what: &str, diags: &mut Vec<Diagnostic>) {
    diags.push(Diagnostic::error_with_code(
        format!(
            "heap allocation ({what}) is not allowed in a no_std unit: there is \
             no global allocator in a no_std build. Provide one (staged) or move \
             this code behind a hosted build."
        ),
        expr.span.clone(),
        "E1400",
    ));
}

/// Recursively walk `expr`, flagging heap-allocating constructions and
/// descending into every sub-expression.
fn validate_expr(expr: &HirExpr, diags: &mut Vec<Diagnostic>) {
    match &expr.kind {
        // ── Unambiguous allocation sites ──────────────────────────────
        HirExprKind::StringLiteral(_) => flag(expr, "string literal", diags),
        HirExprKind::Interpolation { parts } => {
            flag(expr, "string interpolation", diags);
            for p in parts {
                if let crate::hir::nodes::HirInterpolationPart::Expr { expr: e, .. } = p {
                    validate_expr(e, diags);
                }
            }
        }
        HirExprKind::ArrayLiteral(elems) => {
            flag(expr, "array literal", diags);
            for e in elems {
                validate_expr(e, diags);
            }
        }
        HirExprKind::ArrayFill { value, .. } => {
            flag(expr, "array fill", diags);
            validate_expr(value, diags);
        }
        HirExprKind::MapLiteral(pairs) => {
            flag(expr, "map literal", diags);
            for (k, v) in pairs {
                validate_expr(k, diags);
                validate_expr(v, diags);
            }
        }

        // ── Calls that PRODUCE a heap value (e.g. `Array.new`, `.clone`) ──
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            if is_heap_alloc_ty(&expr.ty) {
                flag(expr, "heap-typed method result", diags);
            }
            validate_expr(object, diags);
            for a in args {
                validate_expr(a, diags);
            }
            if let Some(b) = block {
                validate_expr(b, diags);
            }
        }
        HirExprKind::FnCall { args, .. } => {
            if is_heap_alloc_ty(&expr.ty) {
                flag(expr, "heap-typed call result", diags);
            }
            for a in args {
                validate_expr(a, diags);
            }
        }
        HirExprKind::MacroCall { args, .. } => {
            // `array![…]` / `hash!{…}` produce heap values.
            if is_heap_alloc_ty(&expr.ty) {
                flag(expr, "collection macro", diags);
            }
            for a in args {
                validate_expr(a, diags);
            }
        }

        // ── Structural recursion (no allocation at this node) ──────────
        HirExprKind::FieldAccess { object, .. } => validate_expr(object, diags),
        HirExprKind::BinaryOp { left, right, .. } => {
            validate_expr(left, diags);
            validate_expr(right, diags);
        }
        HirExprKind::UnaryOp { operand, .. } => validate_expr(operand, diags),
        HirExprKind::Borrow { expr: e, .. } => validate_expr(e, diags),
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            for s in stmts {
                validate_stmt(s, diags);
            }
            if let Some(t) = tail {
                validate_expr(t, diags);
            }
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            validate_expr(cond, diags);
            validate_expr(then_branch, diags);
            if let Some(e) = else_branch {
                validate_expr(e, diags);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            validate_expr(scrutinee, diags);
            for arm in arms {
                validate_expr(&arm.body, diags);
            }
        }
        HirExprKind::Loop { body } => validate_expr(body, diags),
        HirExprKind::While { condition, body } => {
            validate_expr(condition, diags);
            validate_expr(body, diags);
        }
        HirExprKind::For { iterable, body, .. } => {
            validate_expr(iterable, diags);
            validate_expr(body, diags);
        }
        HirExprKind::Assign { target, value, .. } => {
            validate_expr(target, diags);
            validate_expr(value, diags);
        }
        HirExprKind::CompoundAssign { target, value, .. } => {
            validate_expr(target, diags);
            validate_expr(value, diags);
        }
        HirExprKind::Return(Some(e)) | HirExprKind::Break(Some(e)) => validate_expr(e, diags),
        HirExprKind::Closure { body, .. } => validate_expr(body, diags),
        HirExprKind::Construct { fields, .. } => {
            for (_, e) in fields {
                validate_expr(e, diags);
            }
        }
        HirExprKind::EnumVariant { fields, .. } => {
            for (_, e) in fields {
                validate_expr(e, diags);
            }
        }
        HirExprKind::Tuple(elems) => {
            for e in elems {
                validate_expr(e, diags);
            }
        }
        HirExprKind::Index { object, index } => {
            validate_expr(object, diags);
            validate_expr(index, diags);
        }
        HirExprKind::Cast { expr: e, .. } => validate_expr(e, diags),
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                validate_expr(s, diags);
            }
            if let Some(e) = end {
                validate_expr(e, diags);
            }
        }

        // Leaves with no sub-expressions / no allocation.
        HirExprKind::IntLiteral(_)
        | HirExprKind::FloatLiteral(_)
        | HirExprKind::BoolLiteral(_)
        | HirExprKind::CharLiteral(_)
        | HirExprKind::UnitLiteral
        | HirExprKind::RegexLiteral { .. }
        | HirExprKind::VarRef(_)
        | HirExprKind::Return(None)
        | HirExprKind::Break(None)
        | HirExprKind::Continue
        | HirExprKind::NullLiteral
        | HirExprKind::Error => {}
    }
}

fn validate_stmt(stmt: &HirStatement, diags: &mut Vec<Diagnostic>) {
    match stmt {
        HirStatement::Let { value: Some(v), .. } => validate_expr(v, diags),
        HirStatement::Let { value: None, .. } => {}
        HirStatement::Expr(e) => validate_expr(e, diags),
    }
}
