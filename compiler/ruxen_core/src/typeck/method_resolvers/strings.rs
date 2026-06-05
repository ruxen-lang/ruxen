//! TIER 3 — string-shape structural resolvers (residual).
//!
//! After the zero-Rust-stdlib migration (Phase B / M3, Option C), the bulk
//! of `Ty::String` method typing is delegated to `library/std/string/src/
//! string.rx` via `builtin_bridge` (which calls
//! `InferenceEngine::bridge_builtin_method`). `String` is now a
//! `DefKind::Class` (method-home) whose `.rx` decls were corrected to
//! their true surface types (`size -> USize`, `find -> Option[USize]`,
//! `trim -> str`, `parse_int -> Result[Int, ParseIntError]`, …). The
//! `ParseIntError` / `ParseFloatError` `.message` accessors likewise
//! resolve from their `.rx` classes. Those arms were deleted.
//!
//! What REMAINS here (runs AHEAD of `builtin_bridge` in the pipeline):
//!   1. Three `String` residual arms that `string.rx` cannot declare:
//!      * `remove` — ABI divergence: true surface is `Char` (I32) but the
//!        C symbol `ruxen_string_remove` returns `void*` (I64); a `-> Char`
//!        `.rx` decl would derive a width contradicting the C ABI.
//!        FOLLOW-UP bug: reconcile, then migrate.
//!      * `clone` — aliases the SAME C symbol as `String.from`
//!        (`ruxen_string_from`); E0722 rejects a second `.rx` alias for one
//!        c_symbol with an instance-method shape (see lang_intrinsics doc).
//!      * `to_s` — the structural `to_s` fallback matches only
//!        `Ty::Class/Struct/Enum`, not the `Ty::String` primitive head.
//!   2. `Ty::Str` (`&str`) methods — there is no `class str`. The bridge's
//!      `method_home_key` routes `Ty::Str` → `class String`, but a few
//!      `&str` surfaces differ (`trim -> str`) and `parse_uint` has no
//!      String counterpart, so `&str` typing stays here as the source of
//!      truth. Every return's width matches the shared C symbol.

use crate::hir::types::Ty;

use super::resolver::MethodResolver;
use super::InferenceEngine;

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, method| match ty {
            // Residual `String` arms that shadow string.rx — they run
            // AHEAD of `builtin_bridge` so they win for these names.
            Ty::String => matches!(
                method,
                "remove" | "clone" | "to_s" | "push" | "push_str" | "insert" | "insert_str"
            ),
            // `&str` has no stdlib class — all its methods stay here.
            Ty::Str => true,
            _ => false,
        },
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // ── String residual arms (shadow string.rx) ──────────────
            (Ty::String, "remove") => Some(Ty::Char),
            (Ty::String, "clone") => Some(Ty::String),
            (Ty::String, "to_s") => Some(Ty::String),
            // Mutation methods: the C symbols return `char*` (the new
            // buffer, I64), so the `.rx` decl is `-> String` (ABI-
            // faithful) — but the SURFACE type is mutation-style `Unit`
            // (the value is discarded; `def f(s: &var String); s.push(c);
            // end` returns nil). Declaring `-> nil` in `.rx` would derive a
            // void return contradicting the C `char*`, so these stay Rust
            // residuals returning `Unit`, shadowing the ABI-faithful `.rx`
            // decl. FOLLOW-UP: reconcile surface-vs-C return, then migrate.
            (Ty::String, "push")
            | (Ty::String, "push_str")
            | (Ty::String, "insert")
            | (Ty::String, "insert_str") => Some(Ty::Unit),

            // ── Ty::Str (&str) methods — no `class str` to bridge to ──
            (Ty::Str, "size") => Some(Ty::USize),
            (Ty::Str, "empty?") => Some(Ty::Bool),
            (Ty::Str, "trim") => Some(Ty::Str),
            (Ty::Str, "to_lower") => Some(Ty::Str),
            (Ty::Str, "to_upper") => Some(Ty::Str),
            (Ty::Str, "chars") => Some(Ty::Array(Box::new(Ty::Char))),
            // String#split returns Array<String> in Ruby — always. Both
            // owned-`String` and borrowed-`&str` receivers produce the
            // same surface type (pin: `docs/rondo_v1_blockers.md` B13).
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
            (Ty::Str, "to_s") => Some(Ty::String),
            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
