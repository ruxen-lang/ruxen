//! The `MethodResolver` struct and the ordered pipeline's tier-spanning
//! resolvers.
//!
//! Phase 5 converts the giant `match (ty, method)` in `mod.rs` into an
//! ORDERED pipeline of resolvers. The dispatcher (`mod.rs`) walks
//! `resolvers()` in precedence order; the first resolver whose `resolve`
//! returns `Some` wins. Precedence is the Vec order — there is exactly
//! one decision, not one-per-arm-position.
//!
//! This file holds the `MethodResolver` definition and the two resolvers
//! that are NOT namespace-specific (declared-method tier 1 and the
//! structural-fallback tail of tier 3). The per-namespace tables live in
//! their own sibling files.

use crate::hir::nodes::HirExpr;
use crate::hir::types::Ty;
use crate::lexer::token::Span;

use super::InferenceEngine;

/// One stage of the ordered method-resolution pipeline. The dispatcher
/// walks `resolvers()` in order; the FIRST resolver whose `resolve`
/// returns `Some` wins. `matches` is a cheap structural pre-filter used
/// both to skip work and (in the golden test) to label which resolver
/// claimed a triple. Precedence is the Vec order.
///
/// Every existing arm is a free computation with no captured environment,
/// so `fn(...)` pointers suffice — no `Box<dyn Fn>`, no allocation.
pub(super) struct MethodResolver {
    /// Cheap pre-filter on receiver+method. MUST be side-effect free.
    pub matches: fn(&Ty, &str) -> bool,
    /// The actual return-type computation. May read/mutate `eng`, inspect
    /// `args`, push diagnostics keyed at `span`, and may return `None` to
    /// fall through to the next resolver even when `matches` was true.
    pub resolve: fn(&mut InferenceEngine<'_>, &Ty, &str, &[HirExpr], &Span) -> Option<Ty>,
}

/// TIER 1 — declared-method resolver (the bug-A2 fix).
///
/// Generalises the legacy `(Ty::Class, "new")` declared-lookup arm
/// (`mod.rs:1214`) into a precedence-FIRST resolver: for a `Class`
/// receiver calling `new`, return the user-declared `new`'s return type
/// (`lookup_class_method_return`). It is scoped to `new` only (the legacy
/// arm only consulted declared methods for `new`; other methods flow to
/// the trait resolver in `collect.rs`, unchanged). When the
/// receiver has no declared `new`, `resolve` returns `None` and the
/// pipeline falls through to the structural fallback's `Some(Self)`.
pub(super) fn declared_method_resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, method| method == "new" && matches!(ty, Ty::Class { .. }),
        resolve: |eng, ty, method, args, _span| {
            let Ty::Class { name, .. } = ty else {
                return None;
            };
            if method != "new" {
                return None;
            }
            eng.lookup_class_method_return(name, "new", args.len())
        },
    }]
}

/// TIER 3 (tail) — structural fallback + the `to_s` derive resolver.
///
/// Two concerns, in precedence order:
///
/// 1. **Constructor fallback** (`new`) — the only remaining structural
///    arm. `.new` yields `Self` and is NOT a derive: every `class`/`struct`
///    has a compiler-generated all-fields constructor (the `None` branch
///    of the legacy `(Ty::Class,"new")` arm — when no declared `new`
///    exists, the constructor yields `Self`).
/// 2. **`to_s`** → `String` when the type derives `Debug` (Displayable).
///    The structural implicit-include rule (`ruby-naming.spec.md` §3.6)
///    makes every aggregate derive `Debug`, so this matches the old blanket
///    `to_s → String`; the universal `to_s` is routed through the
///    interpolation display dispatch at lower time. `to_s` is not a method
///    of `mixin Debug`, so it has no `.rx` declaration to register.
///
/// The other derived methods (`clone`, `eq`, `hash_code`, `default`, `cmp`,
/// `partial_cmp`) are typed from their `.rx` mixin declarations, registered
/// per type by `MixinResolver::register_derived_methods` under the same rule
/// MIR synthesises them by.
pub(super) fn structural_fallback_resolvers() -> Vec<MethodResolver> {
    use crate::resolve::symbols::ty_has_derive_trait;
    vec![
        // Structural fallback — `.new` constructor yields Self.
        MethodResolver {
            matches: |ty, method| {
                method == "new" && matches!(ty, Ty::Class { .. } | Ty::Struct { .. })
            },
            resolve: |_eng, ty, method, _args, _span| match (ty, method) {
                (Ty::Class { .. }, "new") | (Ty::Struct { .. }, "new") => Some(ty.clone()),
                _ => None,
            },
        },
        // `to_s` gated on the `Debug` derive.
        MethodResolver {
            matches: |ty, method| {
                matches!(ty, Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. })
                    && method == "to_s"
            },
            resolve: |eng, ty, _method, _args, _span| {
                ty_has_derive_trait(ty, eng.symbols, "Debug").then_some(Ty::String)
            },
        },
    ]
}
