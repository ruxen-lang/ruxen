//! Cranelift view of the compiler-emitted runtime ABI residual.
//!
//! The residual itself lives in the backend-neutral `codegen::residual_abi`
//! table, shared with the LLVM backend; this module only maps its `AbiTy`
//! slots to Cranelift types. Every `.rx`-declared FFI symbol takes its ABI
//! from codegen Pass-0 (`ty_to_cranelift` over the declared `Ty`s) and never
//! reaches this lookup.
//!
//! Consumers:
//!   * `cranelift/emit.rs::coerce_call_args`, as a FALLBACK after the
//!     derived `user_fn_param_tys`.
//!   * `cranelift/emit.rs`, for the runtime calls it emits itself
//!     (`ruxen_alloc`, string `==` and `<=>`).
//!   * `cranelift/translation_env.rs::get_or_declare_func`, for a residual
//!     callee emitted mid-body.

use cranelift_codegen::ir::types::{self, Type};

use crate::codegen::residual_abi::{residual_fn, AbiTy};

/// Cranelift `(param_types, optional_return_type)` of a compiler-emitted
/// runtime function, or `None` when `name` is not in the residual table.
/// Pointers cross as `I64` (Cranelift targets are 64-bit native only).
pub fn compiler_internal_signature(name: &str) -> Option<(Vec<Type>, Option<Type>)> {
    let r = residual_fn(name)?;
    Some((
        r.params.iter().copied().map(abi_ty_to_cranelift).collect(),
        r.ret.map(abi_ty_to_cranelift),
    ))
}

fn abi_ty_to_cranelift(ty: AbiTy) -> Type {
    match ty {
        AbiTy::I8 => types::I8,
        AbiTy::I32 => types::I32,
        AbiTy::I64 | AbiTy::Ptr => types::I64,
        AbiTy::F64 => types::F64,
    }
}
