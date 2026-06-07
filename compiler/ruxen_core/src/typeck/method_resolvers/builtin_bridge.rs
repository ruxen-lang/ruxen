//! Builtin-head method-resolution DELEGATOR (zero-Rust-stdlib, Phase B,
//! Option C).
//!
//! For builtin `Ty` heads whose methods have been migrated to their `.rx`
//! method-home class (`String` so far; `Array`/`Set`/`Map`/scalars as
//! their steps land), this resolver carries ZERO hardcoded method
//! knowledge: it delegates to `InferenceEngine::bridge_builtin_method`,
//! which resolves the method from the `.rx` class via
//! `lookup_method_with_args` + generic substitution.
//!
//! Why a resolver (and not the post-fallback trait lookup at
//! `resolve_method_call` line 82): running here, inside the resolver
//! pipeline (`builtin_method_type`, consulted at line 77 BEFORE the
//! `obj_ty.is_infer()` fallback), preserves the inference fixpoint
//! ordering the old hardcoded arms relied on — fixing the interpolation
//! regression a line-82 lookup hit (`#{s.size}` mangling `?T_size`).
//!
//! Residuals that must NOT delegate (handled by their own resolver arms,
//! which run AHEAD of this one): the ABI-divergent `String.remove`
//! (Char/I32 surface vs C void*/I64), `String.clone` (E0722 alias of
//! `from`), `String.to_s` (structural-head), and the Problem-3
//! closure/effectful/Send-check arms (concurrency / bufio).

use crate::hir::types::Ty;

use super::resolver::MethodResolver;

/// Builtin heads currently delegated to their `.rx` method-home. Grows as
/// each migration step lands (Map/scalars next). The receiver shape that
/// reaches this resolver is the un-reffed head (the same as the `String`
/// arm relied on); `method_home_key` and `substitute_generics_in_return`
/// peel any reference layers downstream when the bridge resolves.
fn is_delegated_head(ty: &Ty) -> bool {
    // Feature A: `Float`/`Bool`/`Char`/`USize` now delegate their
    // `to_s`/`to_string`/`to_i` to their `.rx` method-home classes
    // (scalar/src/lib.rx). The receiver-prepend uses each symbol's true
    // register class (`primitive_ffi_receiver_ty`: `Float`→F64, the rest
    // →I64), so the derived FFI width matches the C symbol and the parity
    // guard passes. Their numeric.rs residual arms are removed.
    //
    // `Ty::Str` (`&str`) IS delegated: `method_home_key` routes it to
    // `class String` (there is no `class str`), and a `&str` receiver is
    // a pointer-sized I64 — wire-compatible with the `ruxen_string_*`
    // symbols those decls bind. Both `&str` and `&String` cross the C ABI
    // as one pointer, so the receiver-prepend is ABI-faithful. The few
    // `&str` surfaces that genuinely DIFFER from `class String`'s return
    // (`to_lower`/`to_upper` yield `str` not `String`, `parse_uint` has
    // no `String` counterpart, `to_s`) stay as residual arms in
    // `strings.rs`, which run AHEAD of this bridge and shadow it.
    // `Option`/`Result` delegate their NON-closure methods (unwrap/
    // expect/unwrap_or/nil?/present?/ok_or/ok?/err?/ok/err) to their
    // builtin enums (option_result/src/lib.rx). Their closure /
    // operator residuals (`map`/`map_err`/`unwrap_or_else`/`try_op`)
    // stay in collections.rs and run AHEAD of this bridge, shadowing the
    // delegation for those names.
    matches!(
        ty,
        Ty::String
            | Ty::Str
            | Ty::Array(_)
            | Ty::Set(_)
            | Ty::Map(_, _)
            | Ty::Int
            | Ty::Float
            | Ty::Bool
            | Ty::Char
            | Ty::USize
            | Ty::Option(_)
            | Ty::Result(_, _)
    )
}

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| is_delegated_head(ty),
        resolve: |eng, ty, method, args, span| eng.bridge_builtin_method(ty, method, args, span),
    }]
}
