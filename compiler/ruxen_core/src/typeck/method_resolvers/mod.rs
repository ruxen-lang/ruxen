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

mod concurrency;
mod fmt;
mod resolver;

use resolver::MethodResolver;

/// The method-resolution entry point. Signature UNCHANGED — callers in
/// `infer/collect.rs:333` and `infer/expr.rs` stay byte-identical. Walks
/// the ordered resolver pipeline; the first resolver to return `Some`
/// wins. The pipeline (one ordered `Vec`) is the single precedence
/// decision, replacing the smear of ~290 arm positions.
pub(super) fn builtin_method_type(
    eng: &mut InferenceEngine<'_>,
    ty: &Ty,
    method: &str,
    args: &[HirExpr],
    span: &Span,
) -> Option<Ty> {
    for r in resolvers() {
        if (r.matches)(ty, method) {
            if let Some(ret) = (r.resolve)(eng, ty, method, args, span) {
                return Some(ret);
            }
        }
    }
    // The single, deliberate "nothing claimed it" — was the legacy
    // match's trailing `_ => None`.
    None
}

/// The ONE precedence decision. During the migration this delegates to a
/// single legacy-wrapping resolver; each migration task carves a
/// namespace out of the legacy match into its own slot here, at the
/// correct precedence position.
fn resolvers() -> Vec<MethodResolver> {
    let mut v = Vec::new();
    v.extend(resolver::declared_method_resolvers()); // TIER 1 — fixes A2
    v.extend(concurrency::resolvers()); // TIER 2 — named stdlib
    v.extend(fmt::resolvers());
    v.extend(resolver::legacy_resolvers()); // TIER 2 (remaining named stdlib, still legacy-wrapped)
    v.extend(resolver::structural_fallback_resolvers()); // TIER 3 tail
    v
}

/// LEGACY — the original `match (ty, method)`. Arms are carved out of
/// here into per-namespace resolvers over Tasks 3–10; once empty it is
/// deleted. Reached only through the `legacy_resolvers()` wrapper.
fn legacy_builtin_method_type(
    eng: &mut InferenceEngine<'_>,
    ty: &Ty,
    method: &str,
    args: &[HirExpr],
    span: &Span,
) -> Option<Ty> {
    match (ty, method) {
        // String methods
        (Ty::String, "clone") => Some(Ty::String),
        (Ty::String, "size") => Some(Ty::USize),
        (Ty::String, "empty?") => Some(Ty::Bool),
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
        (Ty::String, "include?") => Some(Ty::Bool),
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
        (Ty::Str, "size") => Some(Ty::USize),
        (Ty::Str, "empty?") => Some(Ty::Bool),
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
        (Ty::Str, "include?") => Some(Ty::Bool),
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
        (Ty::String, "from_iter") => Some(Ty::String),

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

        // NOTE: the concurrency arms (Future/Thread/JoinHandle/ThreadPanic/
        // Mutex/MutexGuard/Arc/SharedSync, incl. E1100/E1101/E1102) moved to
        // `concurrency::resolvers()` (tier 2). See Phase 5 Task 4.

        // NOTE: the Formatter arms moved to `fmt::resolvers()` (tier 2).
        // See Phase 5 Task 5.

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
        (Ty::Class { name, .. }, "size") if name == "Metadata" => Some(Ty::Int),
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
        (Ty::String, "to_s") => Some(Ty::String),
        (Ty::Str, "to_s") => Some(Ty::String),
        // `to_s` on user-defined types also yields a `String`. The MIR
        // method-call lowering routes it through the same display dispatch
        // as `"#{obj}"` (unless the type defines its own `to_s`, which
        // wins). A user `to_s` conventionally returns `String` too, so
        // reporting `String` here is correct in both cases.
        // NOTE: the generic `to_s`/`clone`/`new`/`default` arms for
        // `Class`/`Struct`/`Enum` receivers moved to
        // `resolver::structural_fallback_resolvers` (tier 3 tail), and the
        // declared-`new` override moved to `resolver::declared_method_resolvers`
        // (tier 1 — the bug-A2 fix). See Phase 5 Task 3.

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

#[cfg(test)]
mod golden {
    //! Golden parity corpus for `builtin_method_type` (Phase 5).
    //!
    //! This module is the migration oracle: it captures the CURRENT
    //! `match (ty, method)` answer (the `Option<Ty>` return AND any
    //! diagnostic codes pushed) for one representative triple per arm,
    //! freezes it in a committed snapshot, and asserts every later
    //! migration task reproduces it byte-for-byte. The corpus uses
    //! stdlib-named receivers with an EMPTY symbol table (no user-declared
    //! methods), so it pins stdlib/structural behaviour only — bug A2
    //! (user class shadowing a stdlib name) is deliberately NOT exercised
    //! here and is pinned separately.
    //!
    //! Record mode (regenerate the snapshot from the current code):
    //!   `RECORD_GOLDEN=1 cargo test -p ruxen_core --lib \
    //!        method_resolvers::golden::golden_parity -- --nocapture`
    //! Assert mode (default): the test compares against the committed
    //! snapshot and fails on any divergence.

    use super::*;
    use crate::hir::context::TypeContext;
    use crate::hir::nodes::{HirExpr, HirExprKind};
    use crate::hir::types::Ty;
    use crate::lexer::token::Span;
    use crate::resolve::symbols::SymbolTable;
    use crate::typeck::infer::InferenceEngine;
    use crate::typeck::mixins::MixinResolver;

    fn span() -> Span {
        Span {
            start: 0,
            end: 0,
            line: 1,
            column: 1,
        }
    }

    /// An argument expression whose `.ty` is what the effectful arms read
    /// (e.g. `Mutex.new` reads `args[0].ty`). The `kind` is irrelevant to
    /// the resolvers except `Thread.spawn`'s closure-capture scan, which
    /// is exercised with a dedicated closure-free arg below (a non-closure
    /// arg simply skips the E1100 capture loop).
    fn arg(ty: Ty) -> HirExpr {
        HirExpr {
            kind: HirExprKind::UnitLiteral,
            ty,
            span: span(),
        }
    }

    fn class(name: &str, generic_args: Vec<Ty>) -> Ty {
        Ty::Class {
            name: name.to_string(),
            generic_args,
        }
    }

    fn enum_ty(name: &str) -> Ty {
        Ty::Enum {
            name: name.to_string(),
            generic_args: vec![],
        }
    }

    fn struct_ty(name: &str) -> Ty {
        Ty::Struct {
            name: name.to_string(),
            generic_args: vec![],
        }
    }

    /// One corpus triple: receiver, method, and the args the (effectful)
    /// arm inspects. Most arms ignore args, so the common case is `&[]`.
    struct Case {
        recv: Ty,
        method: &'static str,
        args: Vec<HirExpr>,
    }

    fn c(recv: Ty, method: &'static str) -> Case {
        Case {
            recv,
            method,
            args: vec![],
        }
    }

    fn c_args(recv: Ty, method: &'static str, args: Vec<HirExpr>) -> Case {
        Case { recv, method, args }
    }

    /// The full corpus — one entry per distinct `(Ty-head, guard, method)`
    /// arm in `mod.rs`. Effectful arms carry the args they read. Built
    /// programmatically per receiver group; transcribed from the live
    /// arms, then cross-checked for completeness against the source's
    /// method-literal set by `golden_covers_every_method`.
    fn corpus() -> Vec<Case> {
        let mut v: Vec<Case> = Vec::new();

        // ── Ty::String structural ──────────────────────────────────
        for m in [
            "clone", "size", "empty?", "push_str", "trim", "to_lower", "to_upper", "chars",
            "split", "push", "as_str", "from", "include?", "starts_with", "ends_with", "repeat",
            "lines", "replace", "new", "with_capacity", "to_string", "bytes", "trim_start",
            "trim_end", "find", "splitn", "clear", "truncate", "insert", "insert_str", "remove",
            "parse_int", "parse_float", "into_bytes", "from_iter", "to_s",
        ] {
            v.push(c(Ty::String, m));
        }

        // ── Ty::Str structural ─────────────────────────────────────
        for m in [
            "size", "empty?", "trim", "to_lower", "to_upper", "chars", "split", "parse_uint",
            "as_str", "include?", "starts_with", "ends_with", "lines", "replace", "to_string",
            "bytes", "trim_start", "trim_end", "find", "splitn", "parse_int", "parse_float",
            "to_s",
        ] {
            v.push(c(Ty::Str, m));
        }

        // ── ParseIntError / ParseFloatError ────────────────────────
        v.push(c(class("ParseIntError", vec![]), "message"));
        v.push(c(class("ParseFloatError", vec![]), "message"));

        // ── Ty::Array structural ───────────────────────────────────
        let arr = || Ty::Array(Box::new(Ty::Int));
        for m in [
            "size", "empty?", "push", "pop", "get", "get_mut", "iter", "into_iter", "each", "map",
            "filter", "find", "position", "to_vec", "new", "sum", "count", "reverse", "first",
            "last", "clone", "include?", "sort", "join", "with_capacity", "capacity", "clear",
            "truncate", "swap", "insert", "remove", "extend", "iter_mut", "as_slice", "from_iter",
            "dedup", "sort_by", "retain",
        ] {
            v.push(c(arr(), m));
        }

        // ── Ty::Map structural ─────────────────────────────────────
        let map = || Ty::Map(Box::new(Ty::String), Box::new(Ty::Int));
        for m in [
            "new", "from_iter", "insert", "get", "key?", "size", "empty?", "with_capacity",
            "remove", "clear", "keys", "values", "iter",
        ] {
            v.push(c(map(), m));
        }

        // ── Ty::Set structural ─────────────────────────────────────
        let set = || Ty::Set(Box::new(Ty::Int));
        for m in [
            "new", "from_iter", "insert", "include?", "size", "empty?", "with_capacity", "remove",
            "clear", "iter", "union", "intersection", "difference",
        ] {
            v.push(c(set(), m));
        }

        // ── Ty::Option structural ──────────────────────────────────
        let opt = || Ty::Option(Box::new(Ty::Int));
        for m in [
            "try_op", "unwrap", "expect", "unwrap_or", "unwrap_or_else", "map", "ok_or", "is_some",
            "is_none",
        ] {
            v.push(c(opt(), m));
        }

        // ── Ty::Result structural ──────────────────────────────────
        let res = || Ty::Result(Box::new(Ty::Int), Box::new(Ty::String));
        for m in [
            "try_op", "unwrap", "expect", "unwrap_or", "unwrap_or_else", "map", "map_err", "is_ok",
            "is_err",
        ] {
            v.push(c(res(), m));
        }

        // ── concurrency (TIER 2) ───────────────────────────────────
        v.push(c(class("Future", vec![Ty::Int]), "await"));
        // Thread.spawn with a non-closure arg (skips the E1100 loop, hits
        // the fresh-var output fallback).
        v.push(c_args(class("Thread", vec![]), "spawn", vec![arg(Ty::Int)]));
        v.push(c(class("Thread", vec![]), "current"));
        v.push(c(class("Thread", vec![]), "sleep"));
        v.push(c(class("Thread", vec![]), "yield_now"));
        v.push(c(class("Thread", vec![]), "id"));
        v.push(c(class("Thread", vec![]), "name"));
        v.push(c(class("JoinHandle", vec![Ty::Int]), "join"));
        v.push(c(class("JoinHandle", vec![Ty::Int]), "join!"));
        v.push(c(class("JoinHandle", vec![Ty::Int]), "thread_id"));
        v.push(c(class("ThreadPanic", vec![]), "message"));
        // Mutex.new with a Send payload (Int) — no E1101.
        v.push(c_args(class("Mutex", vec![]), "new", vec![arg(Ty::Int)]));
        v.push(c(class("Mutex", vec![Ty::Int]), "lock"));
        v.push(c(class("Mutex", vec![Ty::Int]), "lock!"));
        v.push(c(class("Mutex", vec![Ty::Int]), "try_lock"));
        v.push(c(class("Mutex", vec![Ty::Int]), "into_inner"));
        v.push(c(class("MutexGuard", vec![Ty::Int]), "deref"));
        v.push(c(class("MutexGuard", vec![Ty::Int]), "deref_mut"));
        v.push(c(class("MutexGuard", vec![Ty::Int]), "deref_var"));
        // Arc / SharedSync.new with a Send payload (Int) — no E1102.
        v.push(c_args(class("Arc", vec![]), "new", vec![arg(Ty::Int)]));
        v.push(c_args(class("SharedSync", vec![]), "new", vec![arg(Ty::Int)]));
        v.push(c(class("Arc", vec![Ty::Int]), "deref"));
        v.push(c(class("Arc", vec![Ty::Int]), "clone"));
        v.push(c(class("Arc", vec![Ty::Int]), "strong_count"));
        v.push(c(class("Arc", vec![Ty::Int]), "weak_count"));

        // ── fmt (TIER 2) ───────────────────────────────────────────
        for m in [
            "write_str", "write_char", "size", "width", "precision", "align", "fill",
        ] {
            v.push(c(class("Formatter", vec![]), m));
        }

        // ── *Iter combinators (TIER 2-ish, name.ends_with("Iter")) ──
        let veciter = || class("VecIter", vec![Ty::Int]);
        for m in [
            "filter", "map", "find", "position", "sum", "count", "fold", "all", "any", "take",
            "skip", "chain", "zip", "collect_vec", "to_vec", "enumerate", "partition",
        ] {
            v.push(c(veciter(), m));
        }
        // SplitIter.to_vec yields &str segments (distinct branch).
        v.push(c(class("SplitIter", vec![]), "to_vec"));

        // ── io: Stdin / Stdout / Stderr / IoError ──────────────────
        for m in ["read_line", "read_to_string", "lines"] {
            v.push(c(class("Stdin", vec![]), m));
        }
        for m in ["write_str", "flush", "print", "println"] {
            v.push(c(class("Stdout", vec![]), m));
        }
        for m in ["write_str", "flush", "eprint", "eprintln"] {
            v.push(c(class("Stderr", vec![]), m));
        }
        v.push(c(enum_ty("IoError"), "message"));
        v.push(c(enum_ty("IoError"), "kind"));

        // ── fs: Metadata / File / OpenOptions ──────────────────────
        for m in ["size", "modified", "is_file", "is_dir", "is_symlink"] {
            v.push(c(class("Metadata", vec![]), m));
        }
        for m in [
            "open", "create", "append", "open_options", "read", "read_to_string", "read_all",
            "write", "write_all", "write_str", "flush", "seek", "metadata", "close",
        ] {
            v.push(c(class("File", vec![]), m));
        }
        for m in [
            "read", "write", "append", "truncate", "create", "create_new",
        ] {
            v.push(c(class("OpenOptions", vec![]), m));
        }

        // ── process: Command / ExitStatus / Output ─────────────────
        for m in ["arg", "args", "env", "current_dir", "status", "output"] {
            v.push(c(class("Command", vec![]), m));
        }
        for m in ["code", "success"] {
            v.push(c(class("ExitStatus", vec![]), m));
        }
        for m in ["status", "stdout", "stderr"] {
            v.push(c(class("Output", vec![]), m));
        }

        // ── time: Duration / Instant ───────────────────────────────
        for m in [
            "from_secs", "from_millis", "from_micros", "from_nanos", "as_secs", "as_millis",
            "as_micros", "as_nanos", "add", "sub",
        ] {
            v.push(c(class("Duration", vec![]), m));
        }
        for m in ["now", "elapsed", "duration_since", "sub"] {
            v.push(c(class("Instant", vec![]), m));
        }

        // ── net: TcpListener / TcpStream ───────────────────────────
        for m in [
            "bind", "accept", "local_addr", "set_nonblocking", "close",
        ] {
            v.push(c(class("TcpListener", vec![]), m));
        }
        for m in [
            "connect", "read", "write", "peer_addr", "shutdown", "close", "set_read_timeout",
            "set_write_timeout",
        ] {
            v.push(c(class("TcpStream", vec![]), m));
        }

        // ── BufReader / BufWriter (effectful E0714) ────────────────
        // `new` with a supported inner (File) — no diagnostic.
        v.push(c_args(
            class("BufReader", vec![]),
            "new",
            vec![arg(class("File", vec![]))],
        ));
        // `with_capacity(cap, inner)` reads args[1].
        v.push(c_args(
            class("BufReader", vec![]),
            "with_capacity",
            vec![arg(Ty::Int), arg(class("File", vec![]))],
        ));
        v.push(c(class("BufReader", vec![class("File", vec![])]), "read_line"));
        v.push(c(class("BufReader", vec![class("File", vec![])]), "read"));
        v.push(c(class("BufReader", vec![class("File", vec![])]), "into_inner"));
        v.push(c_args(
            class("BufWriter", vec![]),
            "new",
            vec![arg(class("File", vec![]))],
        ));
        v.push(c_args(
            class("BufWriter", vec![]),
            "with_capacity",
            vec![arg(Ty::Int), arg(class("File", vec![]))],
        ));
        for m in ["write", "write_all", "write_str", "flush", "into_inner"] {
            v.push(c(class("BufWriter", vec![class("File", vec![])]), m));
        }

        // ── Enum / numeric / scalar structural (TIER 3) ────────────
        v.push(c(enum_ty("Priority"), "weight"));
        v.push(c(Ty::Bool, "to_string"));
        v.push(c(Ty::Int, "to_string"));
        v.push(c(Ty::USize, "to_string"));
        v.push(c(Ty::Float, "to_string"));
        v.push(c(Ty::Int, "to_f"));
        v.push(c(Ty::Float, "to_i"));
        v.push(c(Ty::Int, "to_s"));
        v.push(c(Ty::USize, "to_s"));
        v.push(c(Ty::Float, "to_s"));
        v.push(c(Ty::Bool, "to_s"));
        v.push(c(Ty::Char, "to_s"));
        // generic to_s / clone / new / default fallbacks
        v.push(c(class("Widget", vec![]), "to_s"));
        v.push(c(struct_ty("Point"), "to_s"));
        v.push(c(enum_ty("Color"), "to_s"));
        v.push(c(class("Widget", vec![]), "new"));
        v.push(c(class("Widget", vec![]), "clone"));
        v.push(c(struct_ty("Point"), "new"));
        v.push(c(struct_ty("Point"), "clone"));
        v.push(c(enum_ty("Color"), "clone"));
        v.push(c(struct_ty("Point"), "default"));
        v.push(c(class("Widget", vec![]), "default"));

        // ── Effectful-arm DIAGNOSTIC pins ──────────────────────────
        // These exercise the early-return + pushed-diagnostic branches
        // of the effectful arms, so the oracle captures the error code
        // AND the `Some(Ty::Error)` return. With an empty symbol table an
        // unknown class is non-Send, so `Mutex.new(NotSend)` fires E1101.
        v.push(c_args(
            class("Mutex", vec![]),
            "new",
            vec![arg(class("NotSend", vec![]))],
        ));
        v.push(c_args(
            class("SharedSync", vec![]),
            "new",
            vec![arg(class("NotSend", vec![]))],
        ));
        v.push(c_args(class("Arc", vec![]), "new", vec![arg(class("NotSend", vec![]))]));
        // BufReader / BufWriter with an unsupported inner (Int) → E0714.
        v.push(c_args(class("BufReader", vec![]), "new", vec![arg(Ty::Int)]));
        v.push(c_args(
            class("BufReader", vec![]),
            "with_capacity",
            vec![arg(Ty::Int), arg(Ty::Int)],
        ));
        v.push(c_args(class("BufWriter", vec![]), "new", vec![arg(Ty::Int)]));
        v.push(c_args(
            class("BufWriter", vec![]),
            "with_capacity",
            vec![arg(Ty::Int), arg(Ty::Int)],
        ));
        // *Iter.sum on a non-numeric element type → E0700.
        v.push(c(class("VecIter", vec![Ty::String]), "sum"));

        v
    }

    /// Build a minimal real engine with an EMPTY symbol table. With no
    /// user-declared methods, `lookup_class_method_return` returns `None`,
    /// so the declared-`new` arm (mod.rs:1173) falls through to the
    /// stdlib/structural answer — exactly the oracle we want to pin.
    fn run_case(case: &Case) -> (Option<Ty>, Vec<String>) {
        let mut ctx = TypeContext::new();
        let mut symbols = SymbolTable::new();
        let traits = MixinResolver::new();
        let mut eng = InferenceEngine::new(&mut ctx, &mut symbols, &traits);
        let ret = builtin_method_type(&mut eng, &case.recv, case.method, &case.args, &span());
        let codes: Vec<String> = eng
            .diagnostics
            .iter()
            .filter_map(|d| d.code.clone())
            .collect();
        (ret, codes)
    }

    fn render(case: &Case, ret: &Option<Ty>, codes: &[String]) -> String {
        format!(
            "{:?} :: {} => {:?} | diag={:?}",
            case.recv, case.method, ret, codes
        )
    }

    fn snapshot_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/method_resolver_golden.snapshot")
    }

    /// The parity oracle. In record mode (`RECORD_GOLDEN=1`) it writes the
    /// snapshot; otherwise it asserts each case matches the committed line.
    #[test]
    fn golden_parity() {
        let cases = corpus();
        let lines: Vec<String> = cases
            .iter()
            .map(|case| {
                let (ret, codes) = run_case(case);
                render(case, &ret, &codes)
            })
            .collect();

        if std::env::var("RECORD_GOLDEN").is_ok() {
            std::fs::write(snapshot_path(), format!("{}\n", lines.join("\n")))
                .expect("write golden snapshot");
            eprintln!("recorded {} golden lines", lines.len());
            return;
        }

        let expected = std::fs::read_to_string(snapshot_path())
            .expect("golden snapshot missing — run with RECORD_GOLDEN=1 first");
        let expected_lines: Vec<&str> = expected.lines().collect();
        assert_eq!(
            lines.len(),
            expected_lines.len(),
            "corpus size ({}) != snapshot size ({}); re-record if you added arms",
            lines.len(),
            expected_lines.len()
        );
        for (i, (got, want)) in lines.iter().zip(expected_lines.iter()).enumerate() {
            assert_eq!(
                got, want,
                "golden parity divergence at corpus index {i}:\n  got:  {got}\n  want: {want}"
            );
        }
    }

    /// Task 2: the dispatcher (`resolvers()` walked by `builtin_method_type`)
    /// must reproduce the legacy match's answer for the whole corpus. Since
    /// `builtin_method_type` IS the dispatcher after Task 2, this asserts the
    /// dispatcher is behaviour-identical to the legacy match it wraps, and
    /// that the resolver pipeline is actually assembled (non-empty).
    #[test]
    fn dispatcher_matches_legacy_for_whole_corpus() {
        assert!(
            !super::resolvers().is_empty(),
            "dispatcher must assemble at least one resolver"
        );
        // Re-run the corpus through the public dispatcher entry and compare
        // to the frozen oracle — identical assertion to `golden_parity`,
        // named to document that the dispatcher path is what's exercised.
        let cases = corpus();
        let expected = std::fs::read_to_string(snapshot_path())
            .expect("golden snapshot missing — run with RECORD_GOLDEN=1 first");
        let expected_lines: Vec<&str> = expected.lines().collect();
        for (i, case) in cases.iter().enumerate() {
            let (ret, codes) = run_case(case);
            assert_eq!(
                render(case, &ret, &codes),
                expected_lines[i],
                "dispatcher diverged from legacy at corpus index {i}"
            );
        }
    }

    /// Task 2 — bug-A2 precedence DECISION (decided empirically, per plan).
    ///
    /// The plan posed: is the real stdlib `Mutex.new` a builtin arm only
    /// (no symbol-table `DefKind::Method`)? If so, tier-1-first would be
    /// safe. This test records the empirical answer: it is NOT — the
    /// stdlib `class Mutex[T]` in `library/std/sync/src/mutex.rx` declares
    /// `def self.new as "ruxen_mutex_new"(initial: T) -> Mutex[T]`, so
    /// `lookup_class_method_return("Mutex", "new")` returns `Some(Mutex[T])`.
    ///
    /// CONSEQUENCE (load-bearing for Task 3): tier-1-first is UNSAFE — a
    /// declared-method resolver running before the named-stdlib arms would
    /// return the unsubstituted `Mutex[T]` instead of the named arm's
    /// payload-inferred `Mutex[<args[0].ty>]` + E1101 Send check, changing
    /// stdlib behaviour and breaking the golden corpus. Therefore tier 1
    /// (declared-method) MUST be scoped to USER-DEFINED receivers only: it
    /// skips any receiver whose name is in the stdlib type-name set
    /// (`resolver::STDLIB_TYPE_NAMES`). This test pins that precondition so
    /// a future change to the stdlib surface that removed the declared
    /// `new` would re-open the tier-1-first option deliberately.
    #[test]
    fn stdlib_mutex_new_is_a_declared_method_so_tier1_is_user_scoped() {
        let mut bootstrap_diagnostics = Vec::new();
        let bootstrap_packages =
            crate::resolve::bootstrap::run_bootstrap_with_package_names(&mut bootstrap_diagnostics);
        // A trivial user program; we only need the stdlib symbols loaded.
        let src = "def main\nend\n";
        let mut lx = crate::lexer::Lexer::new(src);
        let toks = lx.tokenize().expect("lex");
        let mut p = crate::parser::Parser::new(toks);
        let prog = p.parse().expect("parse");
        let resolver = crate::resolve::Resolver::new();
        let result = resolver.resolve_with_bootstrap_packages(&prog, &bootstrap_packages);

        let mut ctx = result.type_context;
        let mut symbols = result.symbols;
        let traits = MixinResolver::new();
        let eng = InferenceEngine::new(&mut ctx, &mut symbols, &traits);
        let mutex_new = eng.lookup_class_method_return("Mutex", "new");
        assert!(
            matches!(mutex_new, Some(Ty::Class { ref name, .. }) if name == "Mutex"),
            "stdlib Mutex declares its own `new` (FFI) returning Mutex[T]; \
             tier-1 must therefore be scoped to user-defined receivers only. \
             got {mutex_new:?}"
        );
    }

    /// Completeness backstop: every method-name literal that appears in a
    /// `mod.rs` arm pattern must be exercised by at least one corpus case.
    /// This turns "I forgot to transcribe an arm" from a silent gap into a
    /// test failure.
    #[test]
    fn golden_covers_every_method() {
        let src = include_str!("mod.rs");
        let mut source_methods: std::collections::BTreeSet<String> = Default::default();
        for line in src.lines() {
            let s = line.trim_start();
            if s.starts_with("(Ty::") || s.starts_with("| (Ty::") {
                // method literals: `, "name")` inside the tuple pattern.
                let mut rest = line;
                while let Some(pos) = rest.find(", \"") {
                    let after = &rest[pos + 3..];
                    if let Some(end) = after.find("\")") {
                        source_methods.insert(after[..end].to_string());
                        rest = &after[end + 2..];
                    } else {
                        break;
                    }
                }
            }
        }
        let covered: std::collections::BTreeSet<String> =
            corpus().iter().map(|c| c.method.to_string()).collect();
        let missing: Vec<&String> = source_methods.difference(&covered).collect();
        assert!(
            missing.is_empty(),
            "corpus is missing source-arm methods: {missing:?}"
        );
    }
}
