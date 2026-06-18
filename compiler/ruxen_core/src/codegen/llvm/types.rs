//! LLVM type mapping — converts Ruxen types to LLVM IR types.

use inkwell::context::Context;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum, FunctionType};
use inkwell::AddressSpace;

use crate::hir::types::Ty;
use crate::mir::nodes::MirFunction;

/// Map a Ruxen type to an LLVM basic type.
///
/// Returns `None` for types with no runtime representation (Unit, Never, Error).
pub fn ty_to_llvm<'ctx>(ty: &Ty, context: &'ctx Context) -> Option<BasicTypeEnum<'ctx>> {
    match ty {
        // Bool is i8, NOT i1 -- matches Cranelift backend and C ABI
        Ty::Bool => Some(context.i8_type().into()),

        // Integer types
        Ty::Int8 | Ty::UInt8 => Some(context.i8_type().into()),
        Ty::Int16 | Ty::UInt16 => Some(context.i16_type().into()),
        Ty::Int32 | Ty::UInt32 | Ty::Char => Some(context.i32_type().into()),
        Ty::Int | Ty::Int64 | Ty::UInt | Ty::UInt64 | Ty::ISize | Ty::USize => {
            Some(context.i64_type().into())
        }

        // A type-erased generic value crosses the ABI as an int64 slot (NOT a
        // pointer): the C runtime takes/returns it as `int64_t`, and the Cranelift
        // backend maps `TypeParam` to I64 likewise. On 64-bit targets a pointer is
        // also i64 so the prior `ptr` mapping happened to work, but on wasm32 a
        // pointer is i32 — an ABI mismatch with the runtime's int64 slots (e.g.
        // `ruxen_vec_push`'s item). Map to i64 to match the slot ABI on every
        // target. Pointer values flowing through a `T` slot are int/ptr-converted
        // at the boundary (see `coerce_value`). (tier 4.09)
        Ty::TypeParam { .. } => Some(context.i64_type().into()),

        // Float types
        Ty::Float32 => Some(context.f32_type().into()),
        Ty::Float | Ty::Float64 => Some(context.f64_type().into()),

        // All pointer/heap-allocated types -> opaque ptr
        Ty::String
        | Ty::Array(_)
        | Ty::Map(_, _)
        | Ty::Set(_)
        | Ty::Ref(_)
        | Ty::RefMut(_)
        | Ty::RefLifetime(_, _)
        | Ty::RefMutLifetime(_, _)
        | Ty::RawPtr(_)
        | Ty::RawPtrMut(_)
        | Ty::RawPtrVoid
        | Ty::RawPtrMutVoid
        | Ty::Option(_)
        | Ty::Result(_, _)
        | Ty::Class { .. }
        | Ty::Struct { .. }
        | Ty::Enum { .. }
        | Ty::Fn { .. }
        | Ty::FnMut { .. }
        | Ty::FnOnce { .. }
        | Ty::AnyMixin(_)
        | Ty::SomeMixin(_)
        | Ty::Alias { .. }
        | Ty::Newtype { .. }
        | Ty::Infer(_)
        | Ty::Tuple(_)
        | Ty::FixedArray(_, _) => Some(context.ptr_type(AddressSpace::default()).into()),

        // No runtime representation
        Ty::Unit | Ty::Never | Ty::Error => None,
        // T2.02 S6: const-arg markers never reach codegen on their own —
        // they live inside a parent type's generic_args list. Mirrors
        // `cranelift/helpers.rs::ty_to_cranelift`.
        Ty::ConstArg(_) => None,
    }
}

/// Build an LLVM function type from a MIR function signature.
pub fn build_function_type<'ctx>(func: &MirFunction, context: &'ctx Context) -> FunctionType<'ctx> {
    // Special case: main returns i32
    if func.name == "main" {
        let ptr_ty = context.ptr_type(AddressSpace::default());
        let params: &[BasicMetadataTypeEnum] = &[context.i32_type().into(), ptr_ty.into()];
        return context.i32_type().fn_type(params, false);
    }

    let param_types: Vec<BasicMetadataTypeEnum> = func
        .params
        .iter()
        .filter_map(|&pid| ty_to_llvm(&func.locals[pid as usize].ty, context).map(|t| t.into()))
        .collect();

    match ty_to_llvm(&func.return_ty, context) {
        Some(ret_ty) => ret_ty.fn_type(&param_types, false),
        None => context.void_type().fn_type(&param_types, false),
    }
}
