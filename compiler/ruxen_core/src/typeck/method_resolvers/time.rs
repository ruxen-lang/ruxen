//! TIER 2 — time namespace resolvers.
//!
//! `Duration` / `Instant` constructors + accessors + named arithmetic,
//! carved verbatim out of the legacy match.

use crate::hir::types::Ty;

use super::resolver::MethodResolver;
use super::InferenceEngine;

const NAMES: &[&str] = &["Duration", "Instant"];

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| {
            matches!(ty, Ty::Class { name, .. } if NAMES.contains(&name.as_str()))
        },
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // Phase 2 stdlib (#06.5 T4): Duration static-style
            // constructors. Receiver type-name resolves to `Duration`
            // (class identifier promoted to its Ty by the resolver).
            // Each `from_*` takes `Int` and returns `Duration`.
            (Ty::Class { name, .. }, "from_secs")
            | (Ty::Class { name, .. }, "from_millis")
            | (Ty::Class { name, .. }, "from_micros")
            | (Ty::Class { name, .. }, "from_nanos")
                if name == "Duration" =>
            {
                Some(InferenceEngine::class_ty("Duration", vec![]))
            }
            // Duration instance accessors — integer division.
            (Ty::Class { name, .. }, "as_secs")
            | (Ty::Class { name, .. }, "as_millis")
            | (Ty::Class { name, .. }, "as_micros")
            | (Ty::Class { name, .. }, "as_nanos")
                if name == "Duration" =>
            {
                Some(Ty::Int)
            }
            // Duration named arithmetic methods. The `+`/`-` operator
            // path also routes here (see mir/lower/expr/binops.rs);
            // `.add()` / `.sub()` are the explicit named surface,
            // load-bearing when the binop site isn't statically
            // resolvable (e.g. generic over Duration).
            (Ty::Class { name, .. }, "add") if name == "Duration" => {
                Some(InferenceEngine::class_ty("Duration", vec![]))
            }
            (Ty::Class { name, .. }, "sub") if name == "Duration" => {
                Some(InferenceEngine::class_ty("Duration", vec![]))
            }
            // Phase 2 stdlib (#06.5 T4): Instant.now / elapsed /
            // duration_since. CLOCK_MONOTONIC under the hood.
            (Ty::Class { name, .. }, "now") if name == "Instant" => {
                Some(InferenceEngine::class_ty("Instant", vec![]))
            }
            (Ty::Class { name, .. }, "elapsed") if name == "Instant" => {
                Some(InferenceEngine::class_ty("Duration", vec![]))
            }
            (Ty::Class { name, .. }, "duration_since") if name == "Instant" => {
                Some(InferenceEngine::class_ty("Duration", vec![]))
            }
            // `.sub()` as the named alias for `Instant - Instant`.
            (Ty::Class { name, .. }, "sub") if name == "Instant" => {
                Some(InferenceEngine::class_ty("Duration", vec![]))
            }
            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
