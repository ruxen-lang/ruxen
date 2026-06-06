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
//!   * `Ty::Enum` `.weight` (Priority.weight) — a compiler accessor, not a
//!     runtime symbol.
//!
//! Same ABI-divergence rule as `String.remove`. These run AHEAD of
//! `builtin_bridge` so they shadow the `.rx` delegation for these heads.

use crate::hir::types::Ty;

use super::resolver::MethodResolver;

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| matches!(ty, Ty::Enum { .. }),
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // `Enum.weight` (Priority.weight) — a compiler accessor, not a
            // runtime FFI symbol, so it cannot be a `.rx` decl.
            //
            // The scalar conversions (Float/Bool/Char/USize `to_s`/
            // `to_string`/`to_i`) MIGRATED to their `.rx` method-home
            // classes (scalar/src/lib.rx) once `primitive_ffi_receiver_ty`
            // taught the receiver-prepend to use each C symbol's true
            // register class (Float→F64, the rest→I64). They resolve via
            // builtin_bridge now; their residual arms are removed.
            (Ty::Enum { .. }, "weight") => Some(Ty::Int),
            _ => None,
        },
    }]
}
