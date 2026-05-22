//! Type and helper mappings.
//!
//! Pure functions that translate between Riven's `Ty` / `CmpOp` and the
//! Cranelift IR equivalents, plus a couple of MIR-side predicates used to
//! steer codegen (string compares, address-taken locals). Split out of the
//! original monolithic `cranelift.rs` for navigability — the contents are
//! otherwise unchanged.

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::types::{self, Type};

use crate::hir::types::Ty;
use crate::mir::nodes::*;

/// Map a Riven `Ty` to a Cranelift IR type.
///
/// Returns `None` for `Unit` / `Never` (no runtime representation in return
/// position), `Some(type)` for everything else.
pub(super) fn ty_to_cranelift(ty: &Ty) -> Option<Type> {
    match ty {
        Ty::Bool => Some(types::I8),
        Ty::Int8 | Ty::UInt8 => Some(types::I8),
        Ty::Int16 | Ty::UInt16 => Some(types::I16),
        Ty::Int32 | Ty::UInt32 | Ty::Char => Some(types::I32),
        Ty::Int | Ty::Int64 | Ty::UInt | Ty::UInt64 | Ty::ISize | Ty::USize => Some(types::I64),
        Ty::Float32 => Some(types::F32),
        Ty::Float | Ty::Float64 => Some(types::F64),

        // All pointer-like / heap types -> I64.
        Ty::String
        | Ty::Str
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
        | Ty::TypeParam { .. }
        | Ty::Infer(_)
        | Ty::Tuple(_)
        | Ty::FixedArray(_, _) => Some(types::I64),

        Ty::Unit | Ty::Never => None,
        Ty::Error => None,
        // T2.02 S6: const-arg markers never reach codegen on their
        // own — they live inside a parent type's generic_args list.
        Ty::ConstArg(_) => None,
    }
}

/// Map a MIR `CmpOp` to a Cranelift `IntCC`.
pub(super) fn cmpop_to_intcc(op: CmpOp) -> IntCC {
    match op {
        CmpOp::Eq => IntCC::Equal,
        CmpOp::NotEq => IntCC::NotEqual,
        CmpOp::Lt => IntCC::SignedLessThan,
        CmpOp::LtEq => IntCC::SignedLessThanOrEqual,
        CmpOp::Gt => IntCC::SignedGreaterThan,
        CmpOp::GtEq => IntCC::SignedGreaterThanOrEqual,
    }
}

pub(super) fn cmpop_to_floatcc(op: CmpOp) -> FloatCC {
    match op {
        CmpOp::Eq => FloatCC::Equal,
        CmpOp::NotEq => FloatCC::NotEqual,
        CmpOp::Lt => FloatCC::LessThan,
        CmpOp::LtEq => FloatCC::LessThanOrEqual,
        CmpOp::Gt => FloatCC::GreaterThan,
        CmpOp::GtEq => FloatCC::GreaterThanOrEqual,
    }
}

/// Check if a MIR value operand is a string-typed local.
///
/// Returns true when the operand references a local whose declared MIR type
/// is `String`, `Str`, or a reference to either. This is used to decide
/// whether a `Compare` instruction should use `strcmp` rather than pointer
/// equality.
pub(super) fn is_string_typed_value(val: &MirValue, func: &MirFunction) -> bool {
    if let MirValue::Use(local_id) = val {
        if let Some(local) = func.locals.get(*local_id as usize) {
            return is_string_mir_ty(&local.ty);
        }
    }
    false
}

/// Check if a MIR type is a string-like type.
///
/// Recognises the canonical primitive forms `Ty::String` / `Ty::Str` AND the
/// bootstrap-class form `Ty::Class { name: "String", .. }` that the resolve
/// pass produces for function parameters / fields whose annotation is
/// `String`.
///
/// Why the second form exists: `register_builtins` (`resolve/stdlib/mod.rs`)
/// inserts `String` as `DefKind::TypeAlias { target: Ty::String }`. The
/// bootstrap merge then anchors the user-side `class String` declaration
/// (`library/std/string/src/lib.rvn`) onto the same DefId so FFI methods
/// (`String.from`, `String.new`, …) hang off it. The class-resolution pass
/// in `resolve/items.rs::resolve_class` rewrites that DefId's `DefKind` from
/// `TypeAlias` to `Class`, after which `resolve_type_expr` returns the
/// `Ty::Class { name: "String", .. }` form for any `s: String` annotation.
/// Inferred locals (`let x = String.from(...)`) still get `Ty::String` from
/// the inference rules; only the annotation path hits the Class form.
///
/// This helper is the codegen-side normalisation: any consumer asking "is
/// this value a string?" gets the right answer regardless of which form the
/// resolve pass produced. Without it, the `Compare` emitter falls back to
/// pointer-equality on `def check(s: String, needle: String)` style code —
/// byte-identical strings compare unequal, silently breaking every URL
/// match / header lookup / response comparison in real server code.
pub(super) fn is_string_mir_ty(ty: &Ty) -> bool {
    match ty {
        Ty::String | Ty::Str => true,
        Ty::Class { name, .. } if name == "String" => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_string_mir_ty(inner),
        _ => false,
    }
}

/// Size estimate for heap allocation.
///
/// For classes and structs, we use a field-count-based heuristic (8 bytes
/// per field, since all fields are stored as 64-bit words). For enums,
/// we allocate tag (8 bytes) + payload. Minimum allocation is 8 bytes
/// for any composite type.
pub(super) fn simple_type_size(ty: &Ty) -> usize {
    match ty {
        Ty::Bool | Ty::Int8 | Ty::UInt8 => 1,
        Ty::Int16 | Ty::UInt16 => 2,
        Ty::Int32 | Ty::UInt32 | Ty::Float32 | Ty::Char => 4,
        Ty::Int
        | Ty::Int64
        | Ty::UInt
        | Ty::UInt64
        | Ty::ISize
        | Ty::USize
        | Ty::Float
        | Ty::Float64 => 8,
        Ty::String => 24,
        Ty::Str => 16,
        Ty::Array(_) => 24,
        Ty::Map(_, _) | Ty::Set(_) => 48,
        Ty::Ref(_)
        | Ty::RefMut(_)
        | Ty::RefLifetime(_, _)
        | Ty::RefMutLifetime(_, _)
        | Ty::RawPtr(_)
        | Ty::RawPtrMut(_)
        | Ty::RawPtrVoid
        | Ty::RawPtrMutVoid => 8,
        Ty::Unit | Ty::Never => 0,
        // Enums: tag (8 bytes aligned) + payload (conservatively 8 bytes per field,
        // with space for the largest variant's payload).
        Ty::Enum { .. } => 32, // tag + up to 3 payload fields
        // Classes and structs MUST carry their precomputed size on the
        // `MirInst::Alloc.size` field — `Lowerer::alloc_size` (in
        // `mir/lower/emit.rs`) walks the parent chain to budget every
        // inherited field correctly. A class allocation that hits this
        // size-estimator fallback indicates a missing call site in the
        // MIR lowerer — surface it loudly rather than silently
        // truncating to 64 bytes (which is wrong for any class with
        // >8 fields and triggers heap corruption at runtime).
        Ty::Class { name, .. } | Ty::Struct { name, .. } => {
            panic!(
                "simple_type_size: class/struct `{}` reached the codegen \
                 size-estimator fallback. MIR Alloc.size MUST be set by \
                 `Lowerer::alloc_size`; the 64-byte fallback was always \
                 a defence-in-depth backstop that silently truncated \
                 classes with >8 fields. Fix the missing precompute at \
                 the MIR lowering site.",
                name
            )
        }
        // Option: tag + payload
        Ty::Option(_) => 16,
        // Result: tag + payload
        Ty::Result(_, _) => 16,
        // Tuples: 8 bytes per element
        Ty::Tuple(elems) => elems.len().max(1) * 8,
        // Arrays.  T2.02 stage 4: `n` is a `ConstExpr`; pre-stage-7
        // codegen only resolves `Lit` values.
        Ty::FixedArray(_, n) => n.as_lit().unwrap_or(0) as usize * 8,
        _ => 8,
    }
}
