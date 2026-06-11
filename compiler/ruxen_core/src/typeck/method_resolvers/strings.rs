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
//! What REMAINS here after Feature E (String reconcile) — the two genuinely
//! irreducible `String`-surface residuals:
//!   * `remove` — the C symbol `ruxen_string_remove` returns a 16-byte
//!     struct *pointer* (`void*`/I64) carrying BOTH the removed codepoint
//!     (field 0) and the rewritten buffer (field 1); the MIR special-case
//!     reads both fields and stores the new buffer back through the
//!     `&mut String`. The true surface type is `Char` (I32). No single
//!     `.rx` return type expresses both an I64-wide pointer ABI (needed so
//!     the GetField base is not truncated) AND a `Char` surface, so this
//!     arm stays Rust-side: the `.rx` decl is `-> Int` (ABI-faithful,
//!     shadowed) and the resolver reports the true `Char`.
//!   * `push`/`push_str`/`insert`/`insert_str` — these are MUTATION methods
//!     whose MIR special-cases perform the `&mut String` deref/store dance
//!     and return NO value (`Ok(None)`), so their true SURFACE type is
//!     `Unit` (e.g. `def append_bang(s: &var String); s.push(?!); end`
//!     returns nil — pin `48_borrow_var`). But the C symbols return the
//!     fresh `char*` buffer (I64), which the deref/store dance MUST capture,
//!     so the `.rx` decls are `-> String` (ABI-faithful). Declaring `-> nil`
//!     would derive a void import sig, leaving the captured `new_buf`
//!     undefined and breaking the store-back. So surface `Unit` and C
//!     `char*` cannot collapse to one `.rx` return type: the resolver arms
//!     report `Unit` and shadow the ABI-faithful `.rx` `-> String` decls.
//!
//! MIGRATED to `string.rx` (Feature E) — these arms were deleted:
//!   * `to_lower`/`to_upper` — resolve via the bridge to `class String`'s
//!     `-> String`. The C symbols `malloc` a fresh owned buffer, so
//!     `String` is the CORRECT surface. These use the normal call path (a
//!     real dest), so unlike the mutation methods there is no Unit conflict.
//!   * `parse_uint` — declared on `class String`
//!     (`def parse_uint as "ruxen_str_parse_uint" -> Result[USize, Error]`)
//!     and resolves via the bridge.
//!
//! (One-string-type ADR: there is one string type, `String`; a `&String`
//! borrow peels to `String` in `method_home_key`. The old separate `&str`
//! surface — wire-identical at the C ABI — is gone.)

use crate::hir::types::Ty;

use super::resolver::MethodResolver;

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, method| match ty {
            // The residual `String` arms — shadow string.rx (whose decls
            // are ABI-faithful) so the true SURFACE type wins. Run AHEAD of
            // `builtin_bridge`. See the module header for why neither can
            // collapse to a single `.rx` return type.
            Ty::String => matches!(
                method,
                "remove" | "push" | "push_str" | "insert" | "insert_str"
            ),
            _ => false,
        },
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // `remove`'s true surface is `Char` (the removed codepoint);
            // the C symbol returns a 16-byte struct *pointer* (I64) and the
            // `.rx` decl is `-> Int` (ABI-faithful, shadowed here).
            (Ty::String, "remove") => Some(Ty::Char),
            // Mutation methods: the MIR special-cases drive the deref/store
            // dance and return NO value, so the SURFACE is `Unit` (pin
            // `48_borrow_var`). The `.rx` decls are `-> String` (ABI-
            // faithful, capturing the fresh C `char*` buffer), shadowed here.
            (Ty::String, "push")
            | (Ty::String, "push_str")
            | (Ty::String, "insert")
            | (Ty::String, "insert_str") => Some(Ty::Unit),
            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
