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
    matches!(ty, Ty::String | Ty::Array(_) | Ty::Set(_))
}

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| is_delegated_head(ty),
        resolve: |eng, ty, method, args, _span| eng.bridge_builtin_method(ty, method, args),
    }]
}
