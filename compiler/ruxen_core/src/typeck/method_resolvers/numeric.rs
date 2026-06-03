//! TIER 3 — scalar / numeric structural resolvers.
//!
//! `Ty::Int` / `Ty::USize` / `Ty::Float` / `Ty::Bool` / `Ty::Char`
//! conversions + the `Ty::Enum` `.weight` accessor, carved verbatim out
//! of the legacy match. Pure arms.

use crate::hir::types::Ty;

use super::resolver::MethodResolver;

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| {
            matches!(
                ty,
                Ty::Int | Ty::USize | Ty::Float | Ty::Bool | Ty::Char | Ty::Enum { .. }
            )
        },
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // Enum weight (Priority.weight)
            (Ty::Enum { .. }, "weight") => Some(Ty::Int),

            // Bool methods
            (Ty::Bool, "to_string") => Some(Ty::String),

            // Int methods
            (Ty::Int, "to_string") => Some(Ty::String),
            (Ty::USize, "to_string") => Some(Ty::String),
            (Ty::Float, "to_string") => Some(Ty::String),

            // Numeric conversions. Ruxen has no implicit Int<->Float coercion
            // (see E0707), so these explicit methods are the supported way to
            // cross the integer/float boundary. `to_f` widens an `Int` to a
            // `Float`; `to_i` truncates a `Float` toward zero to an `Int`.
            (Ty::Int, "to_f") => Some(Ty::Float),
            (Ty::Float, "to_i") => Some(Ty::Int),

            // Universal `to_s` (Ruby convention) on scalar primitives — every
            // value can be rendered to a `String`. Backed by the same
            // `ruxen_*_to_string` runtime helpers as string interpolation
            // (`lang_intrinsics::runtime_name` maps the mangled `<Type>_to_s`
            // names). User-defined class/struct/enum `to_s` is handled in the
            // MIR display-dispatch path, not here.
            (Ty::Int, "to_s") => Some(Ty::String),
            (Ty::USize, "to_s") => Some(Ty::String),
            (Ty::Float, "to_s") => Some(Ty::String),
            (Ty::Bool, "to_s") => Some(Ty::String),
            (Ty::Char, "to_s") => Some(Ty::String),
            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
