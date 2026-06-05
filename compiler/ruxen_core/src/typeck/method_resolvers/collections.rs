//! TIER 3 — collection-shape structural resolvers.
//!
//! `Ty::Array` / `Ty::Map` / `Ty::Set` / `Ty::Option` / `Ty::Result`
//! method typing, carved verbatim out of the legacy match. The `map` /
//! `map_err` arms mint a fresh type var via `eng`; the rest are pure.

use crate::diagnostics::Diagnostic;
use crate::hir::types::Ty;

use super::is_iter_sum_compatible;
use super::resolver::MethodResolver;
use super::InferenceEngine;

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| {
            matches!(
                ty,
                Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_) | Ty::Option(_) | Ty::Result(_, _)
            )
        },
        resolve: |eng: &mut InferenceEngine<'_>, ty, method, _args, _span| match (ty, method) {
            // ── Array RESIDUAL arms (run AHEAD of builtin_bridge) ──────
            // The non-residual Array methods (size/empty?/push/pop/get/
            // first/last/include?/clone/to_a/reverse/sort/join/clear/
            // truncate/swap/insert/remove/extend/dedup/take/drop/chain/
            // to_set/new/with_capacity/capacity/count) were MIGRATED to
            // `library/std/array/src/lib.rx` (`class Array[T]`) and now
            // resolve through `builtin_bridge`. What stays here:
            //
            //   * CLOSURE combinators — inlined in mir/lower/closure_inline;
            //     their return types are closure-inferred (a fresh type var
            //     or an element-derived shape), not a static `.rx` return.
            //   * `zip` — the pair's second element type is read from the
            //     ARG, so it cannot be a fixed declared return.
            //   * `to_h` — the Map key/value types are read from the
            //     receiver's tuple element, likewise arg/receiver-derived.
            //   * `sum` — carries the E0700 non-numeric-element check.
            //   * `get_mut` — `Option[&var T]` aliases `ruxen_vec_get_opt`
            //     with a wire shape that differs from `get`'s `&T`, so a
            //     second `.rx` alias would trip E0722.
            (Ty::Array(elem), "get_mut") => Some(Ty::Option(Box::new(Ty::RefMut(elem.clone())))),
            (Ty::Array(_), "each") => Some(Ty::Unit),
            // Ruby `each_with_index { |element, index| }` — yields the element
            // and its 0-based index. Inlined in closure_inline/each_with_index.
            (Ty::Array(_), "each_with_index") => Some(Ty::Unit),
            (Ty::Array(_), "map") => Some(Ty::Array(Box::new(eng.ctx.fresh_type_var()))),
            // Ruby block combinators (inlined in mir/lower/closure_inline).
            (Ty::Array(elem), "select") => Some(Ty::Array(elem.clone())),
            (Ty::Array(elem), "reject") => Some(Ty::Array(elem.clone())),
            (Ty::Array(_), "reduce") => Some(eng.ctx.fresh_type_var()),
            (Ty::Array(_), "all?") => Some(Ty::Bool),
            (Ty::Array(_), "any?") => Some(Ty::Bool),
            (Ty::Array(elem), "find") => Some(Ty::Option(Box::new(Ty::Ref(elem.clone())))),
            (Ty::Array(_), "index") => Some(Ty::Option(Box::new(Ty::USize))),
            // `partition` is inlined; returns a (matching, rest) tuple.
            (Ty::Array(elem), "partition") => Some(Ty::Tuple(vec![
                Ty::Array(elem.clone()),
                Ty::Array(elem.clone()),
            ])),
            (Ty::Array(elem), "zip") => {
                let other = match _args.first() {
                    Some(a) => match eng.ctx.resolve(&a.ty) {
                        Ty::Array(e) => *e,
                        Ty::Ref(inner) | Ty::RefMut(inner) => match *inner {
                            Ty::Array(e) => *e,
                            o => o,
                        },
                        o => o,
                    },
                    None => eng.ctx.fresh_type_var(),
                };
                Some(Ty::Array(Box::new(Ty::Tuple(vec![*elem.clone(), other]))))
            }
            // `pairs.to_h` builds a Map[K, V] from an Array of (K, V) tuples
            // — the K/V come from the receiver's tuple element.
            (Ty::Array(elem), "to_h") => match elem.as_ref() {
                Ty::Tuple(kv) if kv.len() == 2 => {
                    Some(Ty::Map(Box::new(kv[0].clone()), Box::new(kv[1].clone())))
                }
                _ => Some(Ty::Map(
                    Box::new(eng.ctx.fresh_type_var()),
                    Box::new(eng.ctx.fresh_type_var()),
                )),
            },
            // `sum` integer-sums raw slots — reject non-numeric elements
            // (Ruby's `["a"].sum` errors too) with a typeck-time E0700.
            (Ty::Array(elem), "sum") => {
                let resolved = eng.ctx.resolve(elem);
                if !is_iter_sum_compatible(&resolved) {
                    eng.diagnostics.push(Diagnostic::error_with_code(
                        format!("`sum` requires numeric elements that implement `Add`; `{resolved}` is not numeric"),
                        _span.clone(),
                        "E0700",
                    ));
                    return Some(Ty::Error);
                }
                Some(Ty::Int)
            }
            (Ty::Array(_), "sort_by") => Some(Ty::Unit),
            (Ty::Array(_), "select!") => Some(Ty::Unit),

            // HashMap methods
            (Ty::Map(_, _), "new") => Some(ty.clone()),
            (Ty::Map(_, _), "insert") => Some(Ty::Unit),
            (Ty::Map(_, v), "get") => Some(Ty::Option(Box::new(Ty::Ref(v.clone())))),
            (Ty::Map(_, _), "key?") => Some(Ty::Bool),
            (Ty::Map(_, _), "size") => Some(Ty::USize),
            (Ty::Map(_, _), "empty?") => Some(Ty::Bool),
            // Phase 2 stdlib (#04): full HashMap[K,V] surface.
            (Ty::Map(_, _), "with_capacity") => Some(ty.clone()),
            (Ty::Map(_, v), "remove") => Some(Ty::Option(v.clone())),
            (Ty::Map(_, _), "clear") => Some(Ty::Unit),
            (Ty::Map(k, _), "keys") => Some(Ty::Array(Box::new(Ty::Ref(k.clone())))),
            (Ty::Map(_, v), "values") => Some(Ty::Array(Box::new(Ty::Ref(v.clone())))),
            // Ruby's `hash.to_a` returns `[(K, V)]` pairs (owned tuples,
            // same aliased-heap contract as `zip`), not the bare key list.
            (Ty::Map(k, v), "to_a") => {
                Some(Ty::Array(Box::new(Ty::Tuple(vec![*k.clone(), *v.clone()]))))
            }

            // Set methods MIGRATED to `library/std/set/src/lib.rx`
            // (`class Set[T]`) — every Set method has a real C symbol and
            // a statically substitutable return, so all resolve through
            // `builtin_bridge`; no residual Set arms remain.

            // Option try_op (the ? operator desugars to this)
            (Ty::Option(inner), "try_op") => Some(*inner.clone()),

            // Option methods
            (Ty::Option(inner), "unwrap") => Some(*inner.clone()),
            (Ty::Option(inner), "expect") => Some(*inner.clone()),
            (Ty::Option(inner), "unwrap_or") => Some(*inner.clone()),
            (Ty::Option(inner), "unwrap_or_else") => Some(*inner.clone()),
            (Ty::Option(_), "map") => Some(Ty::Option(Box::new(eng.ctx.fresh_type_var()))),
            (Ty::Option(inner), "ok_or") => Some(Ty::Result(inner.clone(), Box::new(Ty::Error))),
            // Ruby predicate spellings: `nil?` (was `is_none`) and `present?`
            // (was `is_some`). The Rust `is_some`/`is_none` are removed.
            (Ty::Option(_), "nil?") => Some(Ty::Bool),
            (Ty::Option(_), "present?") => Some(Ty::Bool),

            // Result try_op (the ? operator desugars to this)
            (Ty::Result(ok, _), "try_op") => Some(*ok.clone()),

            // Result methods
            (Ty::Result(ok, _), "unwrap") => Some(*ok.clone()),
            (Ty::Result(ok, _), "expect") => Some(*ok.clone()),
            (Ty::Result(ok, _), "unwrap_or") => Some(*ok.clone()),
            (Ty::Result(ok, _), "unwrap_or_else") => Some(*ok.clone()),
            (Ty::Result(_, _), "map") => Some(Ty::Result(
                Box::new(eng.ctx.fresh_type_var()),
                Box::new(Ty::Error),
            )),
            (Ty::Result(_, err), "map_err") => {
                Some(Ty::Result(Box::new(eng.ctx.fresh_type_var()), err.clone()))
            }
            // Ruby predicate spellings: `ok?` (was `is_ok`) / `err?` (was
            // `is_err`). The Rust `is_ok`/`is_err` are removed.
            (Ty::Result(_, _), "ok?") => Some(Ty::Bool),
            (Ty::Result(_, _), "err?") => Some(Ty::Bool),

            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
