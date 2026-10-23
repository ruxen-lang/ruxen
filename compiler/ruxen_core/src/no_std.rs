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
use crate::hir::nodes::{HirExpr, HirExprKind, HirItem, HirProgram};
use crate::hir::types::Ty;
use crate::hir::visit::{walk_expr, walk_item, Visit};

/// Is `ty` a heap-allocated collection/string whose construction routes through
/// the (absent-in-no_std) allocator?
fn is_heap_alloc_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::String | Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_))
}

/// Validate a typed HIR program under no_std rules. Returns one E1400 per
/// heap-allocating construction found in user code: every function body,
/// including class, struct and enum methods, `extension` methods and mixin
/// default methods (the shared `hir::visit` walk).
pub fn validate(program: &HirProgram) -> Vec<Diagnostic> {
    let mut v = NoStdValidator { diags: Vec::new() };
    for item in &program.items {
        v.visit_item(item);
    }
    v.diags
}

struct NoStdValidator {
    diags: Vec<Diagnostic>,
}

impl NoStdValidator {
    /// Push an E1400 for a heap-allocating construction `expr` describes as `what`.
    fn flag(&mut self, expr: &HirExpr, what: &str) {
        self.diags.push(Diagnostic::error_with_code(
            format!(
                "heap allocation ({what}) is not allowed in a no_std unit: there is \
                 no global allocator in a no_std build. Provide one (staged) or move \
                 this code behind a hosted build."
            ),
            expr.span.clone(),
            "E1400",
        ));
    }
}

impl Visit for NoStdValidator {
    fn visit_item(&mut self, item: &HirItem) {
        // Const initializers that allocate are out of scope for v1 (they would
        // need const-eval anyway); every other item is walked for bodies.
        if !matches!(item, HirItem::Const(_)) {
            walk_item(self, item);
        }
    }

    /// Flag heap-allocating constructions, then descend into every
    /// sub-expression.
    fn visit_expr(&mut self, expr: &HirExpr) {
        let what = match &expr.kind {
            // Unambiguous allocation sites.
            HirExprKind::StringLiteral(_) => Some("string literal"),
            HirExprKind::Interpolation { .. } => Some("string interpolation"),
            HirExprKind::ArrayLiteral(_) => Some("array literal"),
            HirExprKind::ArrayFill { .. } => Some("array fill"),
            HirExprKind::MapLiteral(_) => Some("map literal"),
            // Calls that PRODUCE a heap value (e.g. `Array.new`, `.clone`).
            HirExprKind::MethodCall { .. } if is_heap_alloc_ty(&expr.ty) => {
                Some("heap-typed method result")
            }
            HirExprKind::FnCall { .. } if is_heap_alloc_ty(&expr.ty) => {
                Some("heap-typed call result")
            }
            // `array![…]` / `hash!{…}` produce heap values.
            HirExprKind::MacroCall { .. } if is_heap_alloc_ty(&expr.ty) => Some("collection macro"),
            // Allocates nothing at this node. Listed (no `_`) so a new HIR
            // variant must be triaged for allocation here.
            HirExprKind::MethodCall { .. }
            | HirExprKind::FnCall { .. }
            | HirExprKind::MacroCall { .. }
            | HirExprKind::IntLiteral(_)
            | HirExprKind::FloatLiteral(_)
            | HirExprKind::BoolLiteral(_)
            | HirExprKind::CharLiteral(_)
            | HirExprKind::UnitLiteral
            | HirExprKind::RegexLiteral { .. }
            | HirExprKind::VarRef(_)
            | HirExprKind::FieldAccess { .. }
            | HirExprKind::BinaryOp { .. }
            | HirExprKind::UnaryOp { .. }
            | HirExprKind::Borrow { .. }
            | HirExprKind::Block(..)
            | HirExprKind::UnsafeBlock(..)
            | HirExprKind::If { .. }
            | HirExprKind::Match { .. }
            | HirExprKind::Loop { .. }
            | HirExprKind::While { .. }
            | HirExprKind::For { .. }
            | HirExprKind::Assign { .. }
            | HirExprKind::CompoundAssign { .. }
            | HirExprKind::MultiAssign { .. }
            | HirExprKind::Return(_)
            | HirExprKind::Break(_)
            | HirExprKind::Continue
            | HirExprKind::Next(_)
            | HirExprKind::Closure { .. }
            | HirExprKind::Construct { .. }
            | HirExprKind::EnumVariant { .. }
            | HirExprKind::Tuple(_)
            | HirExprKind::Index { .. }
            | HirExprKind::Cast { .. }
            | HirExprKind::Range { .. }
            | HirExprKind::NullLiteral
            | HirExprKind::Error => None,
        };
        if let Some(what) = what {
            self.flag(expr, what);
        }
        walk_expr(self, expr);
    }
}
