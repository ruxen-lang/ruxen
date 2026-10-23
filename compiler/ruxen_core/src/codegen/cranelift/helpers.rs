//! Cranelift's view of the shared lowering policy (`mir::abi`): the
//! machine class to Cranelift type map, its inverse, signatures, and the
//! `CmpOp` condition codes. The rules themselves live in `mir::abi`.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::types::{self, Type};
use cranelift_codegen::ir::{AbiParam, Signature};
use cranelift_codegen::isa::CallConv;

use crate::hir::types::Ty;
use crate::mir::abi::{AbiClass, FnAbi};
use crate::mir::nodes::CmpOp;

/// The Cranelift type of a machine class. Cranelift targets are 64-bit
/// native only, so a pointer is an `I64`.
pub fn clif_type(class: AbiClass) -> Type {
    match class {
        AbiClass::I8 => types::I8,
        AbiClass::I16 => types::I16,
        AbiClass::I32 => types::I32,
        AbiClass::I64 | AbiClass::Ptr => types::I64,
        AbiClass::F32 => types::F32,
        AbiClass::F64 => types::F64,
    }
}

/// The machine class of a Cranelift value type. Never `Ptr`: on this
/// backend a pointer is an `I64`.
pub fn clif_class(ty: Type) -> AbiClass {
    match ty {
        types::I8 => AbiClass::I8,
        types::I16 => AbiClass::I16,
        types::I32 => AbiClass::I32,
        types::F32 => AbiClass::F32,
        types::F64 => AbiClass::F64,
        // Lowering creates only the types `clif_type` returns, so every
        // other value type is the `I64` word.
        _ => AbiClass::I64,
    }
}

/// Map a Ruxen `Ty` to a Cranelift IR type; `None` for a type with no
/// runtime representation (`Unit` / `Never`).
pub fn ty_to_cranelift(ty: &Ty) -> Option<Type> {
    AbiClass::of(ty).map(clif_type)
}

/// The Cranelift signature of a machine signature.
pub fn clif_signature(call_conv: CallConv, abi: &FnAbi) -> Signature {
    let mut sig = Signature::new(call_conv);
    sig.params
        .extend(abi.param_classes().map(|c| AbiParam::new(clif_type(c))));
    sig.returns
        .extend(abi.ret.map(|c| AbiParam::new(clif_type(c))));
    sig
}

/// Map a MIR `CmpOp` to a Cranelift `IntCC`.
pub fn cmpop_to_intcc(op: CmpOp) -> IntCC {
    match op {
        CmpOp::Eq => IntCC::Equal,
        CmpOp::NotEq => IntCC::NotEqual,
        CmpOp::Lt => IntCC::SignedLessThan,
        CmpOp::LtEq => IntCC::SignedLessThanOrEqual,
        CmpOp::Gt => IntCC::SignedGreaterThan,
        CmpOp::GtEq => IntCC::SignedGreaterThanOrEqual,
    }
}

/// Map a MIR `CmpOp` to a Cranelift `FloatCC`.
pub fn cmpop_to_floatcc(op: CmpOp) -> FloatCC {
    match op {
        CmpOp::Eq => FloatCC::Equal,
        CmpOp::NotEq => FloatCC::NotEqual,
        CmpOp::Lt => FloatCC::LessThan,
        CmpOp::LtEq => FloatCC::LessThanOrEqual,
        CmpOp::Gt => FloatCC::GreaterThan,
        CmpOp::GtEq => FloatCC::GreaterThanOrEqual,
    }
}
