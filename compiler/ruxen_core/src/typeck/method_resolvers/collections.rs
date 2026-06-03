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
            // Vec methods
            (Ty::Array(_), "size") => Some(Ty::USize),
            (Ty::Array(_), "empty?") => Some(Ty::Bool),
            (Ty::Array(_), "push") => Some(Ty::Unit),
            (Ty::Array(elem), "pop") => Some(Ty::Option(elem.clone())),
            (Ty::Array(elem), "get") => Some(Ty::Option(Box::new(Ty::Ref(elem.clone())))),
            (Ty::Array(elem), "get_mut") => Some(Ty::Option(Box::new(Ty::RefMut(elem.clone())))),
            (Ty::Array(elem), "iter") => Some(Ty::Class {
                name: "VecIter".to_string(),
                generic_args: vec![*elem.clone()],
            }),
            (Ty::Array(elem), "into_iter") => Some(Ty::Class {
                name: "VecIntoIter".to_string(),
                generic_args: vec![*elem.clone()],
            }),
            (Ty::Array(_), "each") => Some(Ty::Unit),
            (Ty::Array(_), "map") => Some(Ty::Array(Box::new(eng.ctx.fresh_type_var()))),
            (Ty::Array(elem), "filter") => Some(Ty::Array(elem.clone())),
            (Ty::Array(elem), "find") => Some(Ty::Option(Box::new(Ty::Ref(elem.clone())))),
            (Ty::Array(_), "position") => Some(Ty::Option(Box::new(Ty::USize))),
            (Ty::Array(_), "to_vec") => Some(ty.clone()),
            (Ty::Array(_), "new") => Some(ty.clone()),
            (Ty::Array(_), "sum") => Some(Ty::Int),
            (Ty::Array(_), "count") => Some(Ty::USize),
            (Ty::Array(_), "reverse") => Some(ty.clone()),
            (Ty::Array(elem), "first") => Some(Ty::Option(elem.clone())),
            (Ty::Array(elem), "last") => Some(Ty::Option(elem.clone())),
            (Ty::Array(_), "clone") => Some(ty.clone()),
            (Ty::Array(_), "include?") => Some(Ty::Bool),
            (Ty::Array(_), "sort") => Some(ty.clone()),
            (Ty::Array(_), "join") => Some(Ty::String),
            // Phase 2 stdlib batch 1 (#03).
            (Ty::Array(_), "with_capacity") => Some(ty.clone()),
            (Ty::Array(_), "capacity") => Some(Ty::USize),
            (Ty::Array(_), "clear") => Some(Ty::Unit),
            (Ty::Array(_), "truncate") => Some(Ty::Unit),
            (Ty::Array(_), "swap") => Some(Ty::Unit),
            (Ty::Array(_), "insert") => Some(Ty::Unit),
            (Ty::Array(elem), "remove") => Some(*elem.clone()),
            (Ty::Array(_), "extend") => Some(Ty::Unit),
            (Ty::Array(_), "iter_mut") => Some(ty.clone()),
            (Ty::Array(_), "as_slice") => Some(ty.clone()),
            // Phase 2 stdlib batch 2 (#03).
            (Ty::Array(_), "from_iter") => Some(ty.clone()),
            (Ty::Array(_), "dedup") => Some(Ty::Unit),
            (Ty::Array(_), "sort_by") => Some(Ty::Unit),
            (Ty::Array(_), "retain") => Some(Ty::Unit),

            // HashMap methods
            (Ty::Map(_, _), "new") => Some(ty.clone()),
            (Ty::Map(_, _), "from_iter") => Some(ty.clone()),
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
            (Ty::Map(k, _), "iter") => Some(Ty::Array(Box::new(Ty::Ref(k.clone())))),

            // Set methods
            (Ty::Set(_), "new") => Some(ty.clone()),
            (Ty::Set(_), "from_iter") => Some(ty.clone()),
            (Ty::Set(_), "insert") => Some(Ty::Unit),
            (Ty::Set(_), "include?") => Some(Ty::Bool),
            (Ty::Set(_), "size") => Some(Ty::USize),
            (Ty::Set(_), "empty?") => Some(Ty::Bool),
            // Phase 2 stdlib (#04): full HashSet[T] surface.
            (Ty::Set(_), "with_capacity") => Some(ty.clone()),
            (Ty::Set(_), "remove") => Some(Ty::Bool),
            (Ty::Set(_), "clear") => Some(Ty::Unit),
            (Ty::Set(t), "iter") => Some(Ty::Array(Box::new(Ty::Ref(t.clone())))),
            (Ty::Set(_), "union") => Some(ty.clone()),
            (Ty::Set(_), "intersection") => Some(ty.clone()),
            (Ty::Set(_), "difference") => Some(ty.clone()),

            // Option try_op (the ? operator desugars to this)
            (Ty::Option(inner), "try_op") => Some(*inner.clone()),

            // Option methods
            (Ty::Option(inner), "unwrap") => Some(*inner.clone()),
            (Ty::Option(inner), "expect") => Some(*inner.clone()),
            (Ty::Option(inner), "unwrap_or") => Some(*inner.clone()),
            (Ty::Option(inner), "unwrap_or_else") => Some(*inner.clone()),
            (Ty::Option(_), "map") => Some(Ty::Option(Box::new(eng.ctx.fresh_type_var()))),
            (Ty::Option(inner), "ok_or") => Some(Ty::Result(inner.clone(), Box::new(Ty::Error))),
            (Ty::Option(_), "is_some") => Some(Ty::Bool),
            (Ty::Option(_), "is_none") => Some(Ty::Bool),

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
            (Ty::Result(_, _), "is_ok") => Some(Ty::Bool),
            (Ty::Result(_, _), "is_err") => Some(Ty::Bool),

            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
