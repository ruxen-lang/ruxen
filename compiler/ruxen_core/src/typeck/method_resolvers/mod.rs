//! Per-namespace method-resolver dispatch.
//!
//! Phase D of #06.75 carved this out of `typeck/infer.rs` (where it
//! lived as the 774-LOC `InferenceEngine::builtin_method_type` method,
//! a single `match (recv_ty, method_name) { … }` table).
//!
//! Future follow-up (tracked separately): split this single
//! `builtin_method_type` function into per-namespace files
//! (`primitives.rs`, `string.rs`, `array.rs`, `option.rs`, `result.rs`,
//! `iter.rs`, `io.rs`, `fs.rs`, `time.rs`, …) and have each contribute a
//! `pub fn resolvers() -> Vec<MethodResolver>` that the dispatcher walks.
//! The internal arm groupings below already mark the cut lines.

use crate::diagnostics::Diagnostic;
use crate::hir::nodes::*;
use crate::hir::types::Ty;
use crate::lexer::token::Span;

use super::infer::{is_bufio_inner_supported, is_iter_sum_compatible, InferenceEngine};

pub(super) fn builtin_method_type(
    eng: &mut InferenceEngine<'_>,
    ty: &Ty,
    method: &str,
    args: &[HirExpr],
    span: &Span,
) -> Option<Ty> {
    match (ty, method) {
        // String methods
        (Ty::String, "clone") => Some(Ty::String),
        (Ty::String, "len") => Some(Ty::USize),
        (Ty::String, "is_empty") => Some(Ty::Bool),
        (Ty::String, "push_str") => Some(Ty::Unit),
        (Ty::String, "trim") => Some(Ty::Str),
        (Ty::String, "to_lower") => Some(Ty::String),
        (Ty::String, "to_upper") => Some(Ty::String),
        (Ty::String, "chars") => Some(Ty::Array(Box::new(Ty::Char))),
        // Phase 2 stdlib batch 2 (#02): split returns owned Vec[String]
        // (per the v1 rule: iterator producers return Vec, not lazy
        // SplitIter, until prompt 05 ships the lazy iterator story).
        (Ty::String, "split") => Some(Ty::Array(Box::new(Ty::String))),
        (Ty::String, "push") => Some(Ty::Unit),
        (Ty::String, "as_str") => Some(Ty::Str),
        (Ty::String, "from") => Some(Ty::String),
        (Ty::String, "contains") => Some(Ty::Bool),
        (Ty::String, "starts_with") => Some(Ty::Bool),
        (Ty::String, "ends_with") => Some(Ty::Bool),
        (Ty::String, "repeat") => Some(Ty::String),
        (Ty::String, "lines") => Some(Ty::Array(Box::new(Ty::String))),
        (Ty::String, "replace") => Some(Ty::String),
        // Phase 2 stdlib (#02).
        (Ty::String, "new") => Some(Ty::String),
        (Ty::String, "with_capacity") => Some(Ty::String),
        (Ty::String, "to_string") => Some(Ty::String),
        (Ty::String, "bytes") => Some(Ty::Array(Box::new(Ty::UInt8))),
        (Ty::String, "trim_start") => Some(Ty::Str),
        (Ty::String, "trim_end") => Some(Ty::Str),
        (Ty::String, "find") => Some(Ty::Option(Box::new(Ty::USize))),
        (Ty::String, "splitn") => Some(Ty::Array(Box::new(Ty::String))),
        (Ty::String, "clear") => Some(Ty::Unit),
        (Ty::String, "truncate") => Some(Ty::Unit),
        (Ty::String, "insert") => Some(Ty::Unit),
        (Ty::String, "insert_str") => Some(Ty::Unit),
        (Ty::String, "remove") => Some(Ty::Char),
        (Ty::String, "parse_int") => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(InferenceEngine::class_ty("ParseIntError", vec![])),
        )),
        (Ty::String, "parse_float") => Some(Ty::Result(
            Box::new(Ty::Float),
            Box::new(InferenceEngine::class_ty("ParseFloatError", vec![])),
        )),
        (Ty::String, "into_bytes") => Some(Ty::Array(Box::new(Ty::UInt8))),
        (Ty::Str, "len") => Some(Ty::USize),
        (Ty::Str, "is_empty") => Some(Ty::Bool),
        (Ty::Str, "trim") => Some(Ty::Str),
        (Ty::Str, "to_lower") => Some(Ty::Str),
        (Ty::Str, "to_upper") => Some(Ty::Str),
        (Ty::Str, "chars") => Some(Ty::Array(Box::new(Ty::Char))),
        // String#split returns Array<String> in Ruby — always. Both
        // owned-`String` and borrowed-`&str` receivers should produce
        // the same surface type. The historical `SplitIter` class
        // shape on the `&str` arm was a Rust-style lazy iterator that
        // didn't expose `.get(i)` / `.len()`, leaving callers stuck
        // (every multipart/header parser hits this). Unifying to
        // Array<String> matches Ruby and removes the footgun. Pin:
        // `docs/rondo_v1_blockers.md` B13.
        (Ty::Str, "split") => Some(Ty::Array(Box::new(Ty::String))),
        (Ty::Str, "parse_uint") => Some(Ty::Result(Box::new(Ty::USize), Box::new(Ty::Error))),
        (Ty::Str, "as_str") => Some(Ty::Str),
        (Ty::Str, "contains") => Some(Ty::Bool),
        (Ty::Str, "starts_with") => Some(Ty::Bool),
        (Ty::Str, "ends_with") => Some(Ty::Bool),
        (Ty::Str, "lines") => Some(Ty::Array(Box::new(Ty::String))),
        (Ty::Str, "replace") => Some(Ty::String),
        (Ty::Str, "to_string") => Some(Ty::String),
        (Ty::Str, "bytes") => Some(Ty::Array(Box::new(Ty::UInt8))),
        (Ty::Str, "trim_start") => Some(Ty::Str),
        (Ty::Str, "trim_end") => Some(Ty::Str),
        (Ty::Str, "find") => Some(Ty::Option(Box::new(Ty::USize))),
        (Ty::Str, "splitn") => Some(Ty::Array(Box::new(Ty::String))),
        (Ty::Str, "parse_int") => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(InferenceEngine::class_ty("ParseIntError", vec![])),
        )),
        (Ty::Str, "parse_float") => Some(Ty::Result(
            Box::new(Ty::Float),
            Box::new(InferenceEngine::class_ty("ParseFloatError", vec![])),
        )),
        // ParseIntError / ParseFloatError accessors.
        (Ty::Class { name, .. }, "message")
            if name == "ParseIntError" || name == "ParseFloatError" =>
        {
            Some(Ty::String)
        }

        // Vec methods
        (Ty::Array(_), "len") => Some(Ty::USize),
        (Ty::Array(_), "is_empty") => Some(Ty::Bool),
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
        (Ty::Array(_), "contains") => Some(Ty::Bool),
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
        (Ty::String, "from_iter") => Some(Ty::String),

        // HashMap methods
        (Ty::Map(_, _), "new") => Some(ty.clone()),
        (Ty::Map(_, _), "from_iter") => Some(ty.clone()),
        (Ty::Map(_, _), "insert") => Some(Ty::Unit),
        (Ty::Map(_, v), "get") => Some(Ty::Option(Box::new(Ty::Ref(v.clone())))),
        (Ty::Map(_, _), "contains_key") => Some(Ty::Bool),
        (Ty::Map(_, _), "len") => Some(Ty::USize),
        (Ty::Map(_, _), "is_empty") => Some(Ty::Bool),
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
        (Ty::Set(_), "contains") => Some(Ty::Bool),
        (Ty::Set(_), "len") => Some(Ty::USize),
        (Ty::Set(_), "is_empty") => Some(Ty::Bool),
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

        (Ty::Class { name, generic_args }, "await") if name == "Future" => {
            generic_args.first().cloned()
        }
        (Ty::Class { name, .. }, "spawn") if name == "Thread" => {
            // Spec B6 (send_sync_enforcement.spec.md) — `Thread.spawn`
            // rejects closures whose captures don't satisfy Send (or
            // Sync, for by-ref captures per B7). The check fires only
            // at this construction site; closures used in
            // `Array.each` / `Array.map` / … are unaffected.
            if let Some(arg) = args.first() {
                if let HirExprKind::Closure {
                    captures, is_move, ..
                } = &arg.kind
                {
                    for cap in captures {
                        // The capture's stored `ty` is recorded at
                        // resolve time (control_flow.rs:323) — for
                        // un-annotated `let` bindings that's
                        // `Ty::Infer(_)`. Re-fetch the variable's
                        // current type from the symbol table; typeck
                        // updates it in-place when inferring `let`
                        // bindings (see `update_ty` in
                        // resolve/symbols.rs). Fall back to the
                        // capture's stored ty if no def is registered.
                        let cap_ty = eng
                            .symbols
                            .def_ty(cap.def_id)
                            .map(|t| eng.ctx.resolve(&t))
                            .unwrap_or_else(|| eng.ctx.resolve(&cap.ty));
                        // Class-typed values are moved by default at
                        // the Ruxen level (no `&` was written), even
                        // if the recorded `by_move` flag is false (the
                        // resolver only sets it when an explicit `move`
                        // keyword precedes the closure body). Treat
                        // a non-Copy class capture as by-move for the
                        // Send check; primitives are Copy and Send so
                        // the branch doesn't matter for them.
                        let by_move = cap.by_move
                            || *is_move
                            || matches!(
                                cap_ty,
                                Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
                            );
                        let satisfied = if by_move {
                            cap_ty.is_send_with(eng.symbols)
                        } else {
                            // B7 — by-ref capture requires `&T: Send`,
                            // which means `T: Sync`. The Sync auto-derive
                            // walks fields, so a user class without
                            // `include Sync` is still rejected when its
                            // field set isn't all Sync. We use the
                            // existing `is_sync_with` here (the strict
                            // rule on Sync is left as v2 polish).
                            cap_ty.is_sync_with(eng.symbols)
                        };
                        if !satisfied {
                            let note = if by_move {
                                format!(
                                    "captured value `{}` of type `{}` is not `Send`. \
                                     Add `include Send` to the type if it is safe to share across threads.",
                                    cap.name, cap_ty
                                )
                            } else {
                                format!(
                                    "captured value `{}` is held by reference; the closure \
                                     requires `&{}: Send`, which means `{}` must implement `Sync`.",
                                    cap.name, cap_ty, cap_ty
                                )
                            };
                            eng.diagnostics.push(Diagnostic::error_with_code(
                                note,
                                arg.span.clone(),
                                "E1100",
                            ));
                        }
                    }
                }
            }
            let output = args
                .first()
                .and_then(|arg| InferenceEngine::callable_return_ty(&arg.ty))
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            Some(InferenceEngine::class_ty("JoinHandle", vec![output]))
        }
        (Ty::Class { name, .. }, "current") if name == "Thread" => {
            Some(InferenceEngine::class_ty("Thread", vec![]))
        }
        (Ty::Class { name, .. }, "sleep") if name == "Thread" => Some(Ty::Unit),
        (Ty::Class { name, .. }, "yield_now") if name == "Thread" => Some(Ty::Unit),
        (Ty::Class { name, generic_args }, "join") if name == "JoinHandle" => {
            let output = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(InferenceEngine::result_ty(output, "ThreadPanic"))
        }
        (Ty::Class { name, generic_args }, "join!") if name == "JoinHandle" => {
            generic_args.first().cloned()
        }
        (Ty::Class { name, .. }, "thread_id") if name == "JoinHandle" => {
            Some(InferenceEngine::class_ty("ThreadId", vec![]))
        }
        (Ty::Class { name, .. }, "id") if name == "Thread" => {
            Some(InferenceEngine::class_ty("ThreadId", vec![]))
        }
        (Ty::Class { name, .. }, "name") if name == "Thread" => {
            Some(InferenceEngine::option_ty(Ty::String))
        }
        (Ty::Class { name, .. }, "message") if name == "ThreadPanic" => Some(Ty::String),
        (Ty::Class { name, .. }, "new") if name == "Mutex" => {
            let inner = args
                .first()
                .map(|arg| arg.ty.clone())
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            // Spec B3 (send_sync_enforcement.spec.md) — Mutex.new
            // requires the payload to be Send. `Mutex[T]` itself is
            // declared without a `T: Send` bound (sync.rx line 118)
            // so the regular bound-checker can't catch this; the check
            // fires at the construction site.
            let inner_resolved = eng.ctx.resolve(&inner);
            if let Some(arg) = args.first() {
                if !inner_resolved.is_send_with(eng.symbols) {
                    eng.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "cannot construct `Mutex[{}]` — payload type `{}` is not `Send`. \
                             Add `include Send` to the class if it is safe to share across threads.",
                            inner_resolved, inner_resolved
                        ),
                        arg.span.clone(),
                        "E1101",
                    ));
                }
            }
            Some(InferenceEngine::class_ty("Mutex", vec![inner]))
        }
        (Ty::Class { name, generic_args }, "lock") if name == "Mutex" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(InferenceEngine::result_ty(
                InferenceEngine::class_ty("MutexGuard", vec![inner]),
                "PoisonError",
            ))
        }
        (Ty::Class { name, generic_args }, "lock!") if name == "Mutex" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(InferenceEngine::class_ty("MutexGuard", vec![inner]))
        }
        (Ty::Class { name, generic_args }, "try_lock") if name == "Mutex" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(InferenceEngine::option_ty(InferenceEngine::class_ty(
                "MutexGuard",
                vec![inner],
            )))
        }
        (Ty::Class { name, generic_args }, "into_inner") if name == "Mutex" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(InferenceEngine::result_ty(inner, "PoisonError"))
        }
        (Ty::Class { name, generic_args }, "deref")
            if name == "MutexGuard" || name == "Arc" || name == "SharedSync" =>
        {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(Ty::Ref(Box::new(inner)))
        }
        (Ty::Class { name, generic_args }, "deref_mut") if name == "MutexGuard" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(Ty::RefMut(Box::new(inner)))
        }
        // ruby-naming.spec.md §10a: Arc → SharedSync. Internal name
        // kept as alias; new code uses SharedSync.
        (Ty::Class { name, generic_args }, "deref_var") if name == "MutexGuard" => {
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(Ty::RefMut(Box::new(inner)))
        }
        (Ty::Class { name, .. }, "new") if name == "Arc" || name == "SharedSync" => {
            let inner = args
                .first()
                .map(|arg| arg.ty.clone())
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            // Spec B4 (send_sync_enforcement.spec.md) — SharedSync.new
            // requires the payload to be Send (the wrapper itself
            // doesn't permit mutable sharing, so Sync isn't required of
            // T; only the cross-thread move). `SharedSync[T]` is
            // declared without a `T: Send` bound (sync.rx line 142)
            // so the regular bound-checker can't catch this.
            let inner_resolved = eng.ctx.resolve(&inner);
            if let Some(arg) = args.first() {
                if !inner_resolved.is_send_with(eng.symbols) {
                    eng.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "cannot construct `{}[{}]` — payload type `{}` is not `Send`. \
                             Add `include Send` to the class if it is safe to share across threads.",
                            name, inner_resolved, inner_resolved
                        ),
                        arg.span.clone(),
                        "E1102",
                    ));
                }
            }
            Some(InferenceEngine::class_ty(name, vec![inner]))
        }
        (Ty::Class { name, .. }, "clone") if name == "Arc" || name == "SharedSync" => {
            Some(ty.clone())
        }
        (Ty::Class { name, .. }, "strong_count") if name == "Arc" || name == "SharedSync" => {
            Some(Ty::USize)
        }
        (Ty::Class { name, .. }, "weak_count") if name == "Arc" || name == "SharedSync" => {
            Some(Ty::USize)
        }

        // Phase 2 #06.A3: `std::fmt::Formatter` write surface.
        // `write_str(&str)` and `write_char(Char)` both return
        // `Result[(), FmtError]` — caller chooses to propagate
        // via `?` or match. Phase D wires the runtime semantics;
        // here we only register the typeck contract so user
        // `impl Display` bodies can call `f.write_str("x")` etc.
        // without typeck rejecting the unknown method.
        (Ty::Class { name, .. }, "write_str") if name == "Formatter" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Class {
                name: "FmtError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "write_char") if name == "Formatter" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Class {
                name: "FmtError".to_string(),
                generic_args: vec![],
            }),
        )),
        // `len()` returns the current byte count of the accumulated
        // buffer — mirrors `ruxen_fmt_formatter_len` (returns int64_t).
        (Ty::Class { name, .. }, "len") if name == "Formatter" => Some(Ty::Int),
        // Read-only spec accessors that Phase D will use when
        // formatting widths / precision / fill. Optional types
        // because `"#{x}"` (no spec) leaves them all None.
        (Ty::Class { name, .. }, "width") if name == "Formatter" => {
            Some(Ty::Option(Box::new(Ty::USize)))
        }
        (Ty::Class { name, .. }, "precision") if name == "Formatter" => {
            Some(Ty::Option(Box::new(Ty::USize)))
        }
        (Ty::Class { name, .. }, "align") if name == "Formatter" => Some(Ty::Char),
        (Ty::Class { name, .. }, "fill") if name == "Formatter" => Some(Ty::Char),

        // Iterator-like methods on any "Iter" class
        //
        // Phase 2 stdlib (#05): the Iterator surface lives at the MIR
        // layer — `vec.iter` returns a `*Iter` class which is a
        // run-time no-op pass-through (`ruxen_iter_to_vec`). Eager
        // terminators that don't take a closure (`sum`, `count`,
        // `first`, `last`, `contains`, `reverse`, `clone`) route to
        // the same `ruxen_vec_*` helpers their `Vec` counterparts use,
        // so all that's missing is the type-check entry.
        (Ty::Class { name, .. }, "filter") if name.ends_with("Iter") => Some(ty.clone()),
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
        (Ty::Class { name, .. }, "position") if name.ends_with("Iter") => {
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
        (Ty::Class { name, .. }, "fold") if name.ends_with("Iter") => {
            if let Some(init) = args.first() {
                Some(eng.ctx.resolve(&init.ty))
            } else {
                Some(eng.ctx.fresh_type_var())
            }
        }
        (Ty::Class { name, .. }, "all") if name.ends_with("Iter") => Some(Ty::Bool),
        (Ty::Class { name, .. }, "any") if name.ends_with("Iter") => Some(Ty::Bool),
        // `take(n)` / `skip(n)` are lazy combinators — they return
        // a same-shape iter wrapper. v1 ships eager-materialising
        // runtime helpers (`ruxen_vec_take` / `ruxen_vec_skip`)
        // that hand back a fresh `RuxenVec*`, so the surface type
        // stays the receiver's iter class for chaining.
        (Ty::Class { name, .. }, "take") if name.ends_with("Iter") => Some(ty.clone()),
        (Ty::Class { name, .. }, "skip") if name.ends_with("Iter") => Some(ty.clone()),
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
        (Ty::Class { name, .. }, "read_line") if name == "Stdin" => Some(Ty::Result(
            Box::new(Ty::String),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "read_to_string") if name == "Stdin" => Some(Ty::Result(
            Box::new(Ty::String),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        // Phase 2 stdlib (#06.2): `Stdin.lines()` returns
        // `Vec[Result[String, IoError]]`. v1 simplification of
        // Rust's `BufRead::lines` iterator — every line is read
        // up front (see `ruxen_stdin_lines` in runtime.c). On
        // read failure the vec holds a single Err element.
        (Ty::Class { name, .. }, "lines") if name == "Stdin" => {
            Some(Ty::Array(Box::new(Ty::Result(
                Box::new(Ty::String),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            ))))
        }
        (Ty::Class { name, .. }, "write_str") if name == "Stdout" || name == "Stderr" => {
            Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            ))
        }
        (Ty::Class { name, .. }, "flush") if name == "Stdout" || name == "Stderr" => {
            Some(Ty::Result(
                Box::new(Ty::Unit),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            ))
        }
        // Phase 2 stdlib (#06.1): Stdout / Stderr convenience methods
        // that swallow errors and return `Unit`. Mirror Rust's
        // `print!` / `println!` / `eprint!` / `eprintln!` macros at
        // method-shape level. Use `write_str` + `match` if you need
        // the IoError back.
        (Ty::Class { name, .. }, "print") if name == "Stdout" => Some(Ty::Unit),
        (Ty::Class { name, .. }, "println") if name == "Stdout" => Some(Ty::Unit),
        (Ty::Class { name, .. }, "eprint") if name == "Stderr" => Some(Ty::Unit),
        (Ty::Class { name, .. }, "eprintln") if name == "Stderr" => Some(Ty::Unit),
        // Phase 2 stdlib (#06): std::fs::Metadata accessors.
        // Backed by `ruxen_metadata_*` runtime fns reading from
        // the flat 24-byte heap struct produced by
        // `ruxen_fs_metadata`. `modified` is a UNIX timestamp in
        // seconds (Int), matching `std.time.unix_ns / 1_000_000_000`.
        (Ty::Class { name, .. }, "len") if name == "Metadata" => Some(Ty::Int),
        (Ty::Class { name, .. }, "modified") if name == "Metadata" => Some(Ty::Int),
        (Ty::Class { name, .. }, "is_file") if name == "Metadata" => Some(Ty::Bool),
        (Ty::Class { name, .. }, "is_dir") if name == "Metadata" => Some(Ty::Bool),
        (Ty::Class { name, .. }, "is_symlink") if name == "Metadata" => Some(Ty::Bool),
        // Phase 2 stdlib (#06): std::process::Command builder.
        // `.arg/.args/.env/.current_dir` return Self (same handle,
        // mutate-in-place — the source local is tainted by the
        // method-call default in `compute_dealloc_safe_locals` so
        // double-free is avoided in chained-let bindings).
        // `.status` / `.output` consume self and return Result.
        (Ty::Class { name, .. }, "arg") if name == "Command" => Some(ty.clone()),
        (Ty::Class { name, .. }, "args") if name == "Command" => Some(ty.clone()),
        (Ty::Class { name, .. }, "env") if name == "Command" => Some(ty.clone()),
        (Ty::Class { name, .. }, "current_dir") if name == "Command" => Some(ty.clone()),
        (Ty::Class { name, .. }, "status") if name == "Command" => Some(Ty::Result(
            Box::new(InferenceEngine::class_ty("ExitStatus", vec![])),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "output") if name == "Command" => Some(Ty::Result(
            Box::new(InferenceEngine::class_ty("Output", vec![])),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        // ExitStatus accessors.
        (Ty::Class { name, .. }, "code") if name == "ExitStatus" => Some(Ty::Int),
        (Ty::Class { name, .. }, "success") if name == "ExitStatus" => Some(Ty::Bool),
        // Output accessors. `.status` returns a fresh ExitStatus
        // (cloned in the runtime so the Output can be dropped
        // independently).
        (Ty::Class { name, .. }, "status") if name == "Output" => {
            Some(InferenceEngine::class_ty("ExitStatus", vec![]))
        }
        (Ty::Class { name, .. }, "stdout") if name == "Output" => Some(Ty::String),
        (Ty::Class { name, .. }, "stderr") if name == "Output" => Some(Ty::String),
        // Phase 2 stdlib (#06.5 T2): std::io::File static-style
        // constructors. Receiver type is `File` (the class identifier
        // promoted to a type via resolve::IdentifierKind promotion).
        // All return `Result[File, IoError]`.
        (Ty::Class { name, .. }, "open") if name == "File" => Some(Ty::Result(
            Box::new(InferenceEngine::class_ty("File", vec![])),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "create") if name == "File" => Some(Ty::Result(
            Box::new(InferenceEngine::class_ty("File", vec![])),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "append") if name == "File" => Some(Ty::Result(
            Box::new(InferenceEngine::class_ty("File", vec![])),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "open_options") if name == "File" => Some(Ty::Result(
            Box::new(InferenceEngine::class_ty("File", vec![])),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        // Phase 2 stdlib (#06.5 T2): std::io::File instance methods.
        // Every path that can fail returns `Result[_, IoError]`. The
        // io_error_ty helper would be cleaner but inferring it here
        // matches the existing Command-arm style above.
        (Ty::Class { name, .. }, "read") if name == "File" => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "read_to_string") if name == "File" => Some(Ty::Result(
            Box::new(Ty::String),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "read_all") if name == "File" => Some(Ty::Result(
            Box::new(Ty::Array(Box::new(Ty::Int))),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "write") if name == "File" => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "write_all") if name == "File" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "write_str") if name == "File" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "flush") if name == "File" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "seek") if name == "File" => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "metadata") if name == "File" => Some(Ty::Result(
            Box::new(InferenceEngine::class_ty("Metadata", vec![])),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "close") if name == "File" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        // OpenOptions builder methods — each returns Self.
        (Ty::Class { name, .. }, "read") if name == "OpenOptions" => Some(ty.clone()),
        (Ty::Class { name, .. }, "write") if name == "OpenOptions" => Some(ty.clone()),
        (Ty::Class { name, .. }, "append") if name == "OpenOptions" => Some(ty.clone()),
        (Ty::Class { name, .. }, "truncate") if name == "OpenOptions" => Some(ty.clone()),
        (Ty::Class { name, .. }, "create") if name == "OpenOptions" => Some(ty.clone()),
        (Ty::Class { name, .. }, "create_new") if name == "OpenOptions" => Some(ty.clone()),
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
        // Phase 2 stdlib (#06.5 T5): std::net::TcpListener surface.
        // Every fallible op returns `Result[_, IoError]`. Static
        // constructor `bind` (Ty::Class receiver promoted from the
        // class identifier by the resolver) returns
        // `Result[TcpListener, IoError]`.
        (Ty::Class { name, .. }, "bind") if name == "TcpListener" => Some(Ty::Result(
            Box::new(InferenceEngine::class_ty("TcpListener", vec![])),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "accept") if name == "TcpListener" => Some(Ty::Result(
            Box::new(InferenceEngine::class_ty("TcpStream", vec![])),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "local_addr") if name == "TcpListener" => Some(Ty::Result(
            Box::new(Ty::String),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "set_nonblocking") if name == "TcpListener" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "close") if name == "TcpListener" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        // Phase 2 stdlib (#06.5 T5): std::net::TcpStream surface.
        (Ty::Class { name, .. }, "connect") if name == "TcpStream" => Some(Ty::Result(
            Box::new(InferenceEngine::class_ty("TcpStream", vec![])),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "read") if name == "TcpStream" => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "write") if name == "TcpStream" => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "peer_addr") if name == "TcpStream" => Some(Ty::Result(
            Box::new(Ty::String),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "shutdown") if name == "TcpStream" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "close") if name == "TcpStream" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        // Phase 2 #06.5 T5 additions: socket read/write timeouts.
        // Both take a `&Duration` and return Result[(), IoError].
        (Ty::Class { name, .. }, "set_read_timeout") if name == "TcpStream" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "set_write_timeout") if name == "TcpStream" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        // Phase 2 #06.5 T6: `std::io::BufReader[R]` / `BufWriter[W]`
        // surface. Static-style constructors (`new` / `with_capacity`)
        // are dispatched through the collection-ctor fast path in
        // mir/lower/expr/method_call.rs alongside File / TcpStream.
        // The inner type R / W is restricted to the closed set
        // {File, TcpStream} — anything else is E0714 here.
        //
        // For `with_capacity(cap: Int, inner: R)` the inner is args[1],
        // for `new(inner: R)` it's args[0]. We pick the right slot
        // below.
        (Ty::Class { name, .. }, "new") if name == "BufReader" => {
            let inner = args
                .first()
                .map(|arg| eng.ctx.resolve(&arg.ty))
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            if !is_bufio_inner_supported(&inner) {
                eng.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`BufReader.new` requires inner type to be `File` or `TcpStream`; got `{inner}`"
                    ),
                    span.clone(),
                    "E0714",
                ));
                return Some(Ty::Error);
            }
            Some(InferenceEngine::class_ty("BufReader", vec![inner]))
        }
        (Ty::Class { name, .. }, "with_capacity") if name == "BufReader" => {
            let inner = args
                .get(1)
                .map(|arg| eng.ctx.resolve(&arg.ty))
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            if !is_bufio_inner_supported(&inner) {
                eng.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`BufReader.with_capacity` requires inner type to be `File` or `TcpStream`; got `{inner}`"
                    ),
                    span.clone(),
                    "E0714",
                ));
                return Some(Ty::Error);
            }
            Some(InferenceEngine::class_ty("BufReader", vec![inner]))
        }
        (Ty::Class { name, generic_args }, "read_line") if name == "BufReader" => {
            let _ = generic_args; // shape-only; runtime ignores type param
            Some(Ty::Result(
                Box::new(InferenceEngine::option_ty(Ty::String)),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            ))
        }
        (Ty::Class { name, .. }, "read") if name == "BufReader" => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, generic_args }, "into_inner") if name == "BufReader" => {
            // Surrender the inner — return R directly (not wrapped).
            Some(generic_args.first().cloned().unwrap_or(Ty::Error))
        }
        (Ty::Class { name, .. }, "new") if name == "BufWriter" => {
            let inner = args
                .first()
                .map(|arg| eng.ctx.resolve(&arg.ty))
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            if !is_bufio_inner_supported(&inner) {
                eng.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`BufWriter.new` requires inner type to be `File` or `TcpStream`; got `{inner}`"
                    ),
                    span.clone(),
                    "E0714",
                ));
                return Some(Ty::Error);
            }
            Some(InferenceEngine::class_ty("BufWriter", vec![inner]))
        }
        (Ty::Class { name, .. }, "with_capacity") if name == "BufWriter" => {
            let inner = args
                .get(1)
                .map(|arg| eng.ctx.resolve(&arg.ty))
                .unwrap_or_else(|| eng.ctx.fresh_type_var());
            if !is_bufio_inner_supported(&inner) {
                eng.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`BufWriter.with_capacity` requires inner type to be `File` or `TcpStream`; got `{inner}`"
                    ),
                    span.clone(),
                    "E0714",
                ));
                return Some(Ty::Error);
            }
            Some(InferenceEngine::class_ty("BufWriter", vec![inner]))
        }
        (Ty::Class { name, .. }, "write") if name == "BufWriter" => Some(Ty::Result(
            Box::new(Ty::Int),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "write_all") if name == "BufWriter" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "write_str") if name == "BufWriter" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, .. }, "flush") if name == "BufWriter" => Some(Ty::Result(
            Box::new(Ty::Unit),
            Box::new(Ty::Enum {
                name: "IoError".to_string(),
                generic_args: vec![],
            }),
        )),
        (Ty::Class { name, generic_args }, "into_inner") if name == "BufWriter" => {
            // Result[W, IoError] — flush failure surfaces here.
            let inner = generic_args.first().cloned().unwrap_or(Ty::Error);
            Some(Ty::Result(
                Box::new(inner),
                Box::new(Ty::Enum {
                    name: "IoError".to_string(),
                    generic_args: vec![],
                }),
            ))
        }
        // Phase 2 #06.5: `IoError` is a tagged enum, not a class.
        // `.message() -> String` dispatches on tag in the runtime
        // (see `ruxen_io_error_get_message` in runtime.c).
        (Ty::Enum { name, .. }, "message") if name == "IoError" => Some(Ty::String),
        // Phase 2 #06.5 T1: `.kind() -> IoErrorKind` returns the
        // discriminant as a sibling 20-unit-variant enum. Lets
        // user code branch on the variant tag without binding the
        // payload. Wired through `ruxen_io_error_kind`.
        (Ty::Enum { name, .. }, "kind") if name == "IoError" => Some(Ty::Enum {
            name: "IoErrorKind".to_string(),
            generic_args: vec![],
        }),
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

        // Enum weight (Priority.weight)
        (Ty::Enum { .. }, "weight") => Some(Ty::Int),

        // Bool methods
        (Ty::Bool, "to_string") => Some(Ty::String),

        // Int methods
        (Ty::Int, "to_string") => Some(Ty::String),
        (Ty::USize, "to_string") => Some(Ty::String),
        (Ty::Float, "to_string") => Some(Ty::String),

        // Generic class methods
        (Ty::Class { .. }, "new") => Some(ty.clone()),
        (Ty::Class { .. }, "clone") => Some(ty.clone()),

        // Struct constructors and clone (structs have `.new` generated by
        // the compiler, and `.clone` is available via derive Clone).
        (Ty::Struct { .. }, "new") => Some(ty.clone()),
        (Ty::Struct { .. }, "clone") => Some(ty.clone()),

        // ruby-naming.spec.md §3.6: enums get `.clone` whenever
        // every variant field structurally supports Clone (which
        // is the implicit-include condition).
        (Ty::Enum { .. }, "clone") => Some(ty.clone()),

        // ruby-naming.spec.md §3.6: Default — implicit when every
        // field has a default value. Spec includes Struct, Class,
        // and Enum; for enums Default is conservative (no canonical
        // variant) so we only synthesise it for Struct and Class
        // here.
        (Ty::Struct { .. }, "default") | (Ty::Class { .. }, "default") => Some(ty.clone()),

        // NOTE: six wildcard catch-all arms here (`to_display`, `summary`,
        // `is_actionable`, `is_done`, `serialize`, `message`) used to
        // claim `Ty::String` / `Ty::Bool` for ANY receiver type. They
        // were quality-review §1.3 / §4 — domain-named ones (`is_done`,
        // `is_actionable`) leaked from the sample_program.rx fixture;
        // `to_display`/`summary` were a fallback for an earlier era when
        // mixin-bound dispatch on `&T where T: Mixin` didn't reach the
        // hardcoded signature registry. With the ref-peel fix in
        // `lookup_on_type_param_bounds` (commit b840862) and the proper
        // return types on the Showable/Summarizable mixins, normal
        // dispatch handles them. Real receiver types resolve through
        // `lookup_method` / `lookup_method_on_bounds` above; a typo on
        // an unrelated value (`42.summary`) now surfaces as the
        // intended "unknown method" diagnostic instead of typechecking.
        _ => None,
    }
}
