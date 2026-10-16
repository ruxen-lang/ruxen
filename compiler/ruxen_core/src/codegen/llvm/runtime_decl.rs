//! LLVM declarations for the compiler-emitted runtime ABI residual.
//!
//! The residual itself lives in the backend-neutral `codegen::residual_abi`
//! table, shared with the Cranelift backend; this module only declares its
//! entries. `emit::compile_program` declares every `.rx`-declared FFI import
//! first, from its derived signature (`ty_to_llvm`, which keeps a generic
//! `T` slot at `i64` on wasm32), so the per-name `get_function` guard below
//! never overrides a derived declaration.

use inkwell::context::Context;
use inkwell::module::{Linkage, Module};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum};
use inkwell::AddressSpace;

use crate::codegen::residual_abi::{AbiTy, RESIDUAL_RUNTIME_FNS};

/// Declare every residual runtime function not already in `module`, with
/// External linkage. Idempotent.
pub fn declare_runtime_functions<'ctx>(module: &Module<'ctx>, context: &'ctx Context) {
    for r in RESIDUAL_RUNTIME_FNS {
        if module.get_function(r.name).is_some() {
            continue;
        }
        let params: Vec<BasicMetadataTypeEnum> = r
            .params
            .iter()
            .map(|&t| abi_ty_to_llvm(t, context).into())
            .collect();
        let fn_ty = match r.ret {
            Some(ret) => abi_ty_to_llvm(ret, context).fn_type(&params, false),
            None => context.void_type().fn_type(&params, false),
        };
        module.add_function(r.name, fn_ty, Some(Linkage::External));
    }
}

fn abi_ty_to_llvm<'ctx>(ty: AbiTy, context: &'ctx Context) -> BasicTypeEnum<'ctx> {
    match ty {
        AbiTy::I8 => context.i8_type().into(),
        AbiTy::I32 => context.i32_type().into(),
        AbiTy::I64 => context.i64_type().into(),
        AbiTy::F64 => context.f64_type().into(),
        AbiTy::Ptr => context.ptr_type(AddressSpace::default()).into(),
    }
}
