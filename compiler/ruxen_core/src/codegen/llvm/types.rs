//! LLVM's view of the shared lowering policy (`mir::abi`): the machine
//! class to LLVM type map and function types. The rules
//! themselves live in `mir::abi`.

use inkwell::context::Context;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType};
use inkwell::AddressSpace;

use crate::mir::abi::{AbiClass, FnAbi};

/// The LLVM type of a machine class. `Ptr` is the target's pointer, so it
/// is 32 bits on wasm32.
pub fn llvm_type<'ctx>(class: AbiClass, context: &'ctx Context) -> BasicTypeEnum<'ctx> {
    match class {
        AbiClass::I8 => context.i8_type().into(),
        AbiClass::I16 => context.i16_type().into(),
        AbiClass::I32 => context.i32_type().into(),
        AbiClass::I64 => context.i64_type().into(),
        AbiClass::F32 => context.f32_type().into(),
        AbiClass::F64 => context.f64_type().into(),
        AbiClass::Ptr => context.ptr_type(AddressSpace::default()).into(),
    }
}

/// The LLVM function type of a machine signature.
pub fn llvm_fn_type<'ctx>(abi: &FnAbi, context: &'ctx Context) -> FunctionType<'ctx> {
    let params: Vec<BasicMetadataTypeEnum> = abi
        .param_classes()
        .map(|class| llvm_type(class, context).into())
        .collect();
    match abi.ret {
        Some(ret) => llvm_type(ret, context).fn_type(&params, abi.variadic),
        None => context.void_type().fn_type(&params, abi.variadic),
    }
}
