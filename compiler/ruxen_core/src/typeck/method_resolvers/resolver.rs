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
//! their own sibling files. During the migration, a temporary
//! `legacy_resolvers()` wraps the still-undivided legacy match so the
//! dispatcher runs end-to-end before any arm is carved out.

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

/// TEMPORARY (removed by the final migration task): wraps the entire
/// legacy match as one resolver so the dispatcher runs end-to-end before
/// any arm is carved out. `matches` is always-true; `resolve` is the old
/// match body (renamed `legacy_builtin_method_type`).
pub(super) fn legacy_resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |_, _| true,
        resolve: super::legacy_builtin_method_type,
    }]
}
