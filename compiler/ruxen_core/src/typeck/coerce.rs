//! Auto-deref helper for the Ruxen type checker. Coercion rules live in
//! `unify::can_coerce`.

use crate::hir::context::TypeContext;
use crate::hir::types::Ty;

/// Apply auto-deref chain: follow references until reaching a non-reference type.
/// Returns the deref chain depth and the final type.
pub fn auto_deref(ty: &Ty, ctx: &TypeContext) -> (usize, Ty) {
    let resolved = ctx.resolve(ty);
    let mut current = resolved;
    let mut depth = 0;
    loop {
        match &current {
            Ty::Ref(inner) | Ty::RefMut(inner) => {
                current = ctx.resolve(inner);
                depth += 1;
            }
            Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => {
                current = ctx.resolve(inner);
                depth += 1;
            }
            _ => break,
        }
    }
    (depth, current)
}
