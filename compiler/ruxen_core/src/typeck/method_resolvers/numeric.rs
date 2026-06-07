//! TIER 3 — scalar / numeric structural RESIDUAL resolvers.
//!
//! After the zero-Rust-stdlib migration (Phase 3, Option C), `Ty::Int` /
//! `Ty::Float` method typing is delegated to their `.rx` method-home
//! classes (`class Int` / `class Float` in `library/std/scalar/src/lib.rx`)
//! via `builtin_bridge`. What REMAINS here (runs AHEAD of the bridge so it
//! shadows the delegation for the residual heads):
//!
//!   * `Ty::Float` `to_string` / `to_s` / `to_i` — ABI-divergent: the
//!     instance-method receiver prepended to the derived FFI sig is a
//!     pointer-sized `Ty::Class { name: "Float" }` → I64, but the C
//!     symbols `ruxen_float_to_string` / `ruxen_float_to_i` take a
//!     `double` (F64). A `class Float` decl would fail the parity guard,
//!     so Float stays a Rust residual. (`Int` migrates because its
//!     I64-class-receiver coincidentally matches `int64_t`.)
//!   * `Ty::Bool` / `Ty::Char` `to_s` / `to_string` — same shape: the
//!     I64 class receiver contradicts the narrower head the C symbol
//!     wants. Stay residual. FOLLOW-UP: teach the receiver-prepend to use
//!     the primitive head (or widen the C receivers) and migrate.
//!   * `Ty::USize` `to_s` / `to_string` — shares `ruxen_int_to_string`
//!     with `Int`, but there is no `class USize` (it is not in the
//!     `primitive_class_ty` set); kept here.
//!
//! Same ABI-divergence rule as `String.remove`. These run AHEAD of
//! `builtin_bridge` so they shadow the `.rx` delegation for these heads.
//!
//! REMOVED (Feature D assessment): the `Ty::Enum` `.weight` arm. It was a
//! vestigial golden-corpus-only artifact with NO real user — the only
//! `enum Priority` fixture (`20_enum_methods.rx`) declares `label`, not
//! `weight`, and resolves it via `lookup_class_method_return` (the
//! declared-method fallback in `infer/expr.rs`'s `Ty::Enum` arm), not via
//! a resolver arm. The hardcoded `Some(Int)` was neither a derive
//! candidate (it has a user-authored body, not a structural derivation)
//! nor a genuine floor; worse, it SHADOWED `lookup_class_method_return`,
//! so a user `def weight -> Float` on any enum would mis-type as `Int`.
//! Real user enum accessors resolve through the declared-method path
//! unchanged — pinned by `217_enum_declared_method.rx` (`weight -> Int`).
//! The golden pin was dropped alongside the arm. (NB: a `Float`-returning
//! enum method hits a PRE-EXISTING, unrelated cranelift width bug in enum
//! method codegen — independent of this resolver change, since the deleted
//! arm only ever matched `weight`, never a `Float` accessor — so the pin
//! uses `Int`. See the final report.)
//!
//! No residual numeric/enum arms remain. `resolvers()` returns an empty
//! pipeline stage so the wiring in `mod.rs::resolvers()` and its slot in
//! the precedence order are unchanged; the next stdlib head that needs a
//! numeric residual lands here without re-threading the pipeline.

use super::resolver::MethodResolver;

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![]
}
