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

/// The stdlib `Class` names that own a NAMED `new` arm performing
/// payload-inference and/or a Send/E0714 construction check
/// (`mod.rs` arms at `Mutex.new` / `Arc`+`SharedSync.new` /
/// `BufReader.new` / `BufWriter.new`). The declared-method tier (tier 1)
/// MUST skip these names for `new`: their named arm is the authoritative
/// constructor typing (E1101/E1102/E0714 + arg-based payload), and it
/// must keep winning over `lookup_class_method_return` exactly as it did
/// when the named arm sat positionally before the generic
/// `(Ty::Class, "new")` declared-lookup arm (legacy `mod.rs:1214`).
///
/// This is the empirically-forced refinement of the plan's
/// `STDLIB_TYPE_NAMES` idea (Task 2 probe): the real stdlib `Mutex`
/// declares its own FFI `new`, so an unscoped tier-1-first would override
/// the named arm. Scoping the skip to exactly the names with a named
/// `new` arm is the minimal rule that (a) preserves stdlib behaviour and
/// (b) lets a user class with any OTHER stdlib-shaped name (e.g. `File`,
/// `TcpStream`, `Command`) honour its declared `new` — fixing bug A2.
const STDLIB_NEW_ARM_NAMES: &[&str] = &["Mutex", "Arc", "SharedSync", "BufReader", "BufWriter"];

/// TIER 1 — declared-method resolver (the bug-A2 fix).
///
/// Generalises the legacy `(Ty::Class, "new")` declared-lookup arm
/// (`mod.rs:1214`) into a precedence-FIRST resolver: for a `Class`
/// receiver calling `new`, return the user-declared `new`'s return type
/// (`lookup_class_method_return`). It is scoped to `new` only (the legacy
/// arm only consulted declared methods for `new`; other methods flow to
/// the trait resolver in `collect.rs`, unchanged) and skips
/// `STDLIB_NEW_ARM_NAMES` (whose named arms must keep winning). When the
/// receiver has no declared `new`, `resolve` returns `None` and the
/// pipeline falls through to the structural fallback's `Some(Self)`.
pub(super) fn declared_method_resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, method| {
            method == "new"
                && matches!(ty, Ty::Class { name, .. } if !STDLIB_NEW_ARM_NAMES.contains(&name.as_str()))
        },
        resolve: |eng, ty, method, _args, _span| {
            let Ty::Class { name, .. } = ty else {
                return None;
            };
            // `new` only, user/other receivers only (see `matches`).
            if method != "new" || STDLIB_NEW_ARM_NAMES.contains(&name.as_str()) {
                return None;
            }
            eng.lookup_class_method_return(name, "new")
        },
    }]
}

/// TIER 3 (tail) — structural fallback resolvers.
///
/// The generic `to_s` / `clone` / `new` / `default` arms for
/// `Class`/`Struct`/`Enum` receivers (legacy `mod.rs:1196–1235`),
/// transcribed verbatim. Precedence-LAST: a named stdlib resolver (tier
/// 2) and the declared tier (tier 1) both win over these. The `new` arm
/// here is the `None` branch of the legacy `(Ty::Class,"new")` arm — when
/// no declared `new` exists, the constructor yields `Self`.
pub(super) fn structural_fallback_resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, method| {
            matches!(ty, Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. })
                && matches!(method, "to_s" | "clone" | "new" | "default")
        },
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            (Ty::Class { .. }, "to_s") => Some(Ty::String),
            (Ty::Struct { .. }, "to_s") => Some(Ty::String),
            (Ty::Enum { .. }, "to_s") => Some(Ty::String),

            // Generic `.new` — constructor yields Self. (The declared-`new`
            // override is tier 1; this is the legacy arm's `None` branch.)
            (Ty::Class { .. }, "new") => Some(ty.clone()),
            (Ty::Class { .. }, "clone") => Some(ty.clone()),

            // Structs have a compiler-generated `.new` and derive `Clone`.
            (Ty::Struct { .. }, "new") => Some(ty.clone()),
            (Ty::Struct { .. }, "clone") => Some(ty.clone()),

            // Enums get `.clone` under the implicit-include condition.
            (Ty::Enum { .. }, "clone") => Some(ty.clone()),

            // Default — implicit for Struct/Class (enums excluded).
            (Ty::Struct { .. }, "default") | (Ty::Class { .. }, "default") => Some(ty.clone()),

            _ => None,
        },
    }]
}
