//! TIER 3 — collection-shape structural resolvers.
//!
//! `Ty::Array` / `Ty::Map` / `Ty::Set` / `Ty::Option` / `Ty::Result`
//! method typing, carved verbatim out of the legacy match. The `map` /
//! `map_err` arms mint a fresh type var via `eng`; the rest are pure.

use crate::hir::types::Ty;

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
            //
            // `get_mut` / `get_var` MIGRATED to `array/src/lib.rx`: with
            // E0722 relaxed to a wire-level compare (`&T` and `&var T`
            // are wire-identical pointers), the second/third aliases of
            // `ruxen_vec_get_opt` are now admitted, so they resolve via
            // the bridge against `class Array[T]`'s declared
            // `Option[&var T]` return.
            (Ty::Array(_), "each") => Some(Ty::Unit),
            // `map` / `select` / `reject` / `all?` / `any?` / `partition` /
            // `each_with_index` / `find` / `index` / `reduce` / `sort_by`
            // MIGRATED to real `.rx` bodies over `each` (or `swap`+indexed
            // reads for `sort_by`) (Feature C, array/src/lib.rx) and resolve
            // through the builtin bridge.
            // `zip` MIGRATED to a generic `.rx` decl
            // (`zip[U](other: Array[U]) -> Array[(T, U)]`) that resolves
            // through the bridge; the `ruxen_vec_zip` FFI alias still emits
            // the real call. The result element `(T, U)` is now expressible
            // as a static declared return, so no resolver arm is needed.
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
            // `sum` MIGRATED to the receiver-element bound seam (Task OP):
            // its E0700 non-numeric-element check is now the `.rx` bound
            // `class Array[T]`'s `def sum -> Int where T: Add`, enforced in
            // `bridge_builtin_method` against the receiver's concrete
            // element via `check_generic_param_bounds`. The return type
            // (`Int`) comes from the laundered `.rx` decl through the
            // bridge. No arm here.
            //
            // `sort_by` MIGRATED to a real `.rx` body over `swap` + indexed
            // reads (Feature C); it resolves through the builtin bridge.
            (Ty::Array(_), "select!") => Some(Ty::Unit),

            // Hash (Map) methods MIGRATED to `library/std/map/src/lib.rx`
            // (`class Hash[K, V]`) — every Map method has a real C symbol
            // and a statically substitutable return, so all resolve
            // through `builtin_bridge`; no residual Map arms remain.

            // Set methods MIGRATED to `library/std/set/src/lib.rx`
            // (`class Set[T]`) — every Set method has a real C symbol and
            // a statically substitutable return, so all resolve through
            // `builtin_bridge`; no residual Set arms remain.

            // ── Option / Result RESIDUAL arms (ahead of the bridge) ───
            // The NON-closure Option/Result methods (unwrap/expect/
            // unwrap_or/nil?/present? and Result unwrap/expect/unwrap_or/
            // ok?/err?/ok/err) MIGRATED to the builtin `enum Option[T]` /
            // `enum Result[T, E]` (option_result/src/lib.rx) and resolve
            // through `builtin_bridge` with element substitution. What
            // STAYS here, running AHEAD of the bridge:
            //   * `try_op` — the `?` operator desugaring (no surface
            //     method / C symbol; a compiler intrinsic).
            //   * `ok_or` — the err type is read from the ARG (like
            //     `Array.zip`), not expressible as a static `.rx` return.
            //
            // `map` / `map_err` MIGRATED (Task H) to method-level generic
            // harvesting through the bridge: the `.rx` decls
            // `def map[U](f: any Fn[Fn(T) -> U]) -> Option[U]` /
            // `-> Result[U, E]` and `def map_err[F](...) -> Result[T, F]`
            // carry the transformed type as a method-level generic, and
            // `bridge_builtin_method` → `harvest_and_subst_generics` binds
            // it from the closure argument's inferred return type (the same
            // path Array `map` and `zip[U]` already use). The arms below
            // that minted a fresh type var + relied on `infer_combinator_block`
            // to unify it are retired. `unwrap_or_else` likewise MIGRATED to
            // `.rx` bodies (its return is the static success element `T`).
            (Ty::Option(inner), "try_op") => Some(*inner.clone()),
            (Ty::Option(inner), "ok_or") => Some(Ty::Result(inner.clone(), Box::new(Ty::Error))),

            (Ty::Result(ok, _), "try_op") => Some(*ok.clone()),

            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
