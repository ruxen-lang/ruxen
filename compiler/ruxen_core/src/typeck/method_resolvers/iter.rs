//! TIER 3 — iterator-combinator resolvers.
//!
//! The `name.ends_with("Iter")` / `name.ends_with("IntoIter")` combinator
//! arms (`filter`/`map`/`find`/`sum`/`fold`/`zip`/`collect_vec`/`to_vec`/
//! `enumerate`/`partition`/…), carved verbatim out of the legacy match.
//! Includes the E0700 non-numeric `sum` rejection (early
//! `return Some(Ty::Error)`). The single `to_vec` arm handles both the
//! generic Item-typed Iters and the `SplitIter` (&str segment) special
//! case, so it lives here as one arm rather than split across namespaces.
//!
//! These resolvers key on the receiver class NAME SHAPE (suffix), not a
//! fixed name set — `vec.iter` / `set.iter` / `str.split` etc. all
//! produce a `*Iter` class.

use crate::diagnostics::Diagnostic;
use crate::hir::types::Ty;

use super::is_iter_sum_compatible;
use super::resolver::MethodResolver;

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| matches!(ty, Ty::Class { name, .. } if name.ends_with("Iter")),
        resolve,
    }]
}

fn resolve(
    eng: &mut super::InferenceEngine<'_>,
    ty: &Ty,
    method: &str,
    args: &[crate::hir::nodes::HirExpr],
    span: &crate::lexer::token::Span,
) -> Option<Ty> {
    match (ty, method) {
        // Iterator-like methods on any "Iter" class
        //
        // Phase 2 stdlib (#05): the Iterator surface lives at the MIR
        // layer — `vec.iter` returns a `*Iter` class which is a
        // run-time no-op pass-through (`ruxen_iter_to_vec`). Eager
        // terminators that don't take a closure (`sum`, `count`,
        // `first`, `last`, `contains`, `reverse`, `clone`) route to
        // the same `ruxen_vec_*` helpers their `Vec` counterparts use,
        // so all that's missing is the type-check entry.
        (Ty::Class { name, .. }, "select") if name.ends_with("Iter") => Some(ty.clone()),
        (Ty::Class { name, .. }, "map") if name.ends_with("Iter") => Some(Ty::Class {
            name: name.clone(),
            generic_args: vec![eng.ctx.fresh_type_var()],
        }),
        (Ty::Class { name, generic_args }, "find") if name.ends_with("Iter") => {
            let elem = generic_args
                .first()
                .cloned()
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            Some(Ty::Option(Box::new(Ty::Ref(Box::new(elem)))))
        }
        (Ty::Class { name, .. }, "index") if name.ends_with("Iter") => {
            Some(Ty::Option(Box::new(Ty::USize)))
        }
        (Ty::Class { name, generic_args }, "sum") if name.ends_with("Iter") => {
            // Sum returns the element type for numeric Items. The
            // runtime path is `ruxen_vec_sum` which integer-sums
            // raw 64-bit slots — calling it on a non-`Add` element
            // type produces nonsensical bytes-as-int sums (e.g.
            // `Vec[String].iter.sum` would add string pointers).
            //
            // Phase 2 stdlib (#05 batch 3): reject non-`Add` Items
            // up front. The v1 trait machinery only models a few
            // built-in numeric types as `Add`; surface the rejection
            // here rather than at the runtime layer so users get a
            // typeck-time error with a real source span. Inferred
            // element types (`Ty::Infer`) and the type-error sentinel
            // pass through silently — they will be either pinned by
            // a downstream constraint (still numeric) or already
            // surfaced as a separate diagnostic.
            let elem = generic_args.first().cloned().unwrap_or(Ty::Int);
            let resolved = eng.ctx.resolve(&elem);
            if !is_iter_sum_compatible(&resolved) {
                eng.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`sum` requires an iterator whose Item implements `Add`; \
                             `{resolved}` is not numeric"
                    ),
                    span.clone(),
                    "E0700",
                ));
                return Some(Ty::Error);
            }
            Some(resolved)
        }
        (Ty::Class { name, .. }, "count") if name.ends_with("Iter") => Some(Ty::USize),
        // Phase 2 stdlib (#05 batch 2): closure-taking eager
        // terminators. `fold` returns the accumulator type — for
        // v1 we surface a fresh inference variable that the
        // closure-body unification will pin to the real type
        // (the MIR inliner reads `args[0].ty` to seed the seed).
        // `all` / `any` always return Bool. Inlining happens at
        // MIR; the runtime never sees a `VecIter_fold` call.
        (Ty::Class { name, .. }, "reduce") if name.ends_with("Iter") => {
            if let Some(init) = args.first() {
                Some(eng.ctx.resolve(&init.ty))
            } else {
                Some(eng.ctx.fresh_type_var())
            }
        }
        (Ty::Class { name, .. }, "all?") if name.ends_with("Iter") => Some(Ty::Bool),
        (Ty::Class { name, .. }, "any?") if name.ends_with("Iter") => Some(Ty::Bool),
        // `take(n)` / `skip(n)` are lazy combinators — they return
        // a same-shape iter wrapper. v1 ships eager-materialising
        // runtime helpers (`ruxen_vec_take` / `ruxen_vec_skip`)
        // that hand back a fresh `RuxenVec*`, so the surface type
        // stays the receiver's iter class for chaining.
        (Ty::Class { name, .. }, "take") if name.ends_with("Iter") => Some(ty.clone()),
        (Ty::Class { name, .. }, "drop") if name.ends_with("Iter") => Some(ty.clone()),
        // Phase 2 stdlib (#05 batch 3): `chain(other)` returns the
        // same iter shape (concatenation preserves Item type).
        // `zip(other)` returns an iter whose Item is the pair
        // `(Self.Item, Other.Item)` — for v1 we surface a fresh
        // `*Iter[(T, U)]` so downstream `.count` and `.collect_vec`
        // see the right element type.  `collect_vec` is the v1
        // type-specific shorthand for `collect[Vec[T]]` — it
        // materialises the iter into a `Vec[T]`.
        (Ty::Class { name, .. }, "chain") if name.ends_with("Iter") => Some(ty.clone()),
        (Ty::Class { name, generic_args }, "zip") if name.ends_with("Iter") => {
            let self_item = generic_args
                .first()
                .cloned()
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            let other_item = match args.first() {
                Some(arg) => match eng.ctx.resolve(&arg.ty) {
                    Ty::Class { generic_args, .. } => generic_args
                        .first()
                        .cloned()
                        .unwrap_or_else(|| eng.ctx.fresh_type_var()),
                    Ty::Array(elem) => *elem,
                    other => other,
                },
                None => eng.ctx.fresh_type_var(),
            };
            Some(Ty::Class {
                name: name.clone(),
                generic_args: vec![Ty::Tuple(vec![self_item, other_item])],
            })
        }
        (Ty::Class { name, generic_args }, "collect_vec") if name.ends_with("Iter") => {
            let elem = generic_args
                .first()
                .cloned()
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            Some(Ty::Array(Box::new(elem)))
        }
        (Ty::Class { name, generic_args }, "to_vec") if name.ends_with("Iter") => {
            let elem = if name == "SplitIter" {
                // SplitIter yields &str segments
                Ty::Str
            } else {
                generic_args.first().cloned().unwrap_or(Ty::Error)
            };
            Some(Ty::Array(Box::new(elem)))
        }
        (Ty::Class { name, .. }, "enumerate")
            if name.ends_with("Iter") || name.ends_with("IntoIter") =>
        {
            Some(ty.clone())
        }
        (Ty::Class { name, generic_args }, "partition") if name.ends_with("Iter") => {
            let elem = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(Ty::Tuple(vec![
                Ty::Array(Box::new(elem.clone())),
                Ty::Array(Box::new(elem)),
            ]))
        }
        // Within-namespace fallthrough (not a cross-cutting catch-all).
        _ => None,
    }
}
