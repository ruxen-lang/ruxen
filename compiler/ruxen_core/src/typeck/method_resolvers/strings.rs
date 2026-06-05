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

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, method| match ty {
            // Residual `String` arms that shadow string.rx — they run
            // AHEAD of `builtin_bridge` so they win for these names.
            Ty::String => matches!(
                method,
                "remove" | "to_s" | "push" | "push_str" | "insert" | "insert_str"
            ),
            // `&str` routes to `class String` via the bridge
            // (`method_home_key: Ty::Str → "String"`); only the surfaces
            // that genuinely DIFFER from `class String`'s declared return
            // stay here, running AHEAD of the bridge to shadow it:
            //   * `to_lower`/`to_upper` — `&str` yields `str`, the `.rx`
            //     decl yields `String`.
            //   * `parse_uint` — no `class String` counterpart.
            //   * `to_s` — `class String` declares only `to_string`.
            Ty::Str => matches!(method, "to_lower" | "to_upper" | "parse_uint" | "to_s"),
            _ => false,
        },
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // ── String residual arms (shadow string.rx) ──────────────
            (Ty::String, "remove") => Some(Ty::Char),
            // `clone` MIGRATED to string.rx: with E0722 relaxed to a
            // wire-level compare, its `ruxen_string_from` alias (whose
            // implicit `&self` is wire-identical to `from`'s explicit
            // `&String` param) is now admitted and resolves via the
            // bridge.
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

            // ── Ty::Str (&str) RESIDUAL arms — shadow the bridge ──────
            // The exact-match `&str` surfaces (size/empty?/trim/
            // trim_start/trim_end/chars/lines/split/splitn/bytes/as_str/
            // include?/starts_with/ends_with/find/replace/to_string/
            // parse_int/parse_float) MIGRATED to `class String` via the
            // bridge (`method_home_key: Ty::Str → "String"`); their
            // returns are ABI-identical to the shared `ruxen_string_*`
            // symbols. Only the genuinely-divergent surfaces stay:
            //   * `to_lower`/`to_upper` — `&str` yields `str` (the C
            //     symbol returns a borrowed slice into the source); the
            //     `.rx` `class String` decl yields an owned `String`.
            //   * `parse_uint` — no `class String` counterpart symbol.
            //   * `to_s` — `class String` declares only `to_string`; the
            //     structural `to_s` fallback matches Class/Struct/Enum,
            //     not the `Ty::Str` primitive head.
            (Ty::Str, "to_lower") => Some(Ty::Str),
            (Ty::Str, "to_upper") => Some(Ty::Str),
            (Ty::Str, "parse_uint") => Some(Ty::Result(Box::new(Ty::USize), Box::new(Ty::Error))),
            (Ty::Str, "to_s") => Some(Ty::String),
            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
