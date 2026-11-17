//! The one lowering policy every backend reads (F-29).
//!
//! Cranelift, LLVM and the REPL JIT used to carry their own copy of each
//! rule here, and the copies drifted (a negative `Int32` argument was
//! sign-extended by Cranelift and zero-extended by LLVM; LLVM's float to
//! integer conversion was not saturating). Each rule now lives once, as a
//! DECISION over backend-neutral [`AbiClass`]es and declared `Ty`s, at the
//! MIR level: `mir::coercion` applies the decisions once after lowering and
//! records each as a `MirInst::Convert`, so a backend only maps a class to
//! its native type and emits the operation decided here.
//!
//! - [`AbiClass`]: the machine class of a `Ty` (`AbiClass::of`) and of an
//!   operand ([`value_class`]).
//! - [`FnAbi`]: a function's parameter and return classes, entry ABI included.
//! - [`CalleeIndex`]: every callee's [`FnAbi`] by each name a call can spell.
//! - [`Mode`] / [`coercion`]: how a value crosses from one class to another,
//!   with the per-site mode decided by [`assign_mode`], [`call_arg_mode`],
//!   [`return_mode`] and [`unify_operands`], and an instruction's result
//!   class and landing mode by [`produced`].

use std::collections::HashMap;

use super::lang_intrinsics::runtime_name;
use super::nodes::{FfiFuncDecl, Literal};
use super::nodes::{LocalId, MirFunction, MirInst, MirProgram, MirValue};
use crate::hir::types::Ty;

use super::residual_abi::RESIDUAL_RUNTIME_FNS;

/// The machine class of a value: what a backend lowers to one native type.
/// `Ptr` and `I64` are both 64 bits on Cranelift's native targets (which
/// map both to `I64`) but differ on wasm32, where LLVM lowers `Ptr` to a
/// 32-bit pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbiClass {
    /// C `int8_t` (`Bool`, `Int8`, `UInt8`).
    I8,
    /// C `int16_t`.
    I16,
    /// C `int32_t` (`Int32`, `UInt32`, `Char`).
    I32,
    /// C `int64_t`, including erased uniform-word slots (a generic `T`).
    I64,
    /// C `float`.
    F32,
    /// C `double`.
    F64,
    /// Any heap handle or C pointer.
    Ptr,
}

impl AbiClass {
    /// The class of a declared `Ty`, or `None` when it has no runtime
    /// representation (`Unit`, `Never`, `Error`, a const-generic marker).
    ///
    /// Bool is `I8`, not `i1`, matching the C ABI. A type-erased generic
    /// `T` is an `I64` uniform word, never a pointer: the C runtime takes
    /// and returns it as `int64_t`, which matters where a pointer is 32 bits.
    pub fn of(ty: &Ty) -> Option<AbiClass> {
        match ty {
            Ty::Bool | Ty::Int8 | Ty::UInt8 => Some(AbiClass::I8),
            Ty::Int16 | Ty::UInt16 => Some(AbiClass::I16),
            Ty::Int32 | Ty::UInt32 | Ty::Char => Some(AbiClass::I32),
            Ty::Int | Ty::Int64 | Ty::UInt | Ty::UInt64 | Ty::ISize | Ty::USize => {
                Some(AbiClass::I64)
            }
            Ty::TypeParam { .. } => Some(AbiClass::I64),
            Ty::Float32 => Some(AbiClass::F32),
            Ty::Float | Ty::Float64 => Some(AbiClass::F64),
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
            | Ty::FixedArray(_, _) => Some(AbiClass::Ptr),
            // A const-generic marker lives only inside a parent type's
            // generic arguments and never reaches codegen on its own.
            Ty::Unit | Ty::Never | Ty::Error | Ty::ConstArg(_) => None,
        }
    }

    /// True for `F32` / `F64`.
    pub fn is_float(self) -> bool {
        matches!(self, AbiClass::F32 | AbiClass::F64)
    }

    /// True for the integer classes (`Ptr` excluded).
    pub fn is_int(self) -> bool {
        matches!(
            self,
            AbiClass::I8 | AbiClass::I16 | AbiClass::I32 | AbiClass::I64
        )
    }

    /// Bit width of an integer or float class; `None` for `Ptr`, whose
    /// width is the target's.
    pub fn bits(self) -> Option<u32> {
        match self {
            AbiClass::I8 => Some(8),
            AbiClass::I16 => Some(16),
            AbiClass::I32 | AbiClass::F32 => Some(32),
            AbiClass::I64 | AbiClass::F64 => Some(64),
            AbiClass::Ptr => None,
        }
    }

    /// A type of this class, for a temporary that holds a converted value.
    pub fn ty(self) -> Ty {
        match self {
            AbiClass::I8 => Ty::Int8,
            AbiClass::I16 => Ty::Int16,
            AbiClass::I32 => Ty::Int32,
            AbiClass::I64 => Ty::Int,
            AbiClass::F32 => Ty::Float32,
            AbiClass::F64 => Ty::Float,
            AbiClass::Ptr => Ty::RawPtrVoid,
        }
    }

    /// The integer class as wide as a float class (its bit pattern's home).
    fn same_width_int(self) -> AbiClass {
        if self == AbiClass::F32 {
            AbiClass::I32
        } else {
            AbiClass::I64
        }
    }
}

// ─── The erased uniform word (B01) ───────────────────────────────────────────
//
// Ruxen has ONE storage class for a value whose static type codegen cannot
// see: a 64-bit **uniform word**. It appears at six boundaries:
//
//   1. a generic-collection slot — `RuxenVec` and the hash/set tables store
//      `int64_t` elements regardless of `T`;
//   2. an address-taken local's stack slot — uniformly 8 bytes wide so a
//      `&var` can hand out a stable pointer;
//   3. a parameter or return whose declared `Ty` is opaque (`TypeParam`,
//      a class/ref/option/pointer, …);
//   4. the parameters and return of a closure reached through an erased
//      `Fn` box — the generic body that invokes it was lowered ONCE with the
//      element type abstract, so it can only pass words;
//   5. a scalar method home's `self`, typed as the FFI shell
//      `Class{"Float"}` rather than as `Float`;
//   6. an `Assign` (or `Copy`/`Move`/`Ref`) into a word-typed local.
//
// **ABI decision:** a `Float` occupying a uniform word travels as its
// IEEE-754 **bit pattern**, and is recovered by the inverse reinterpretation.
// It is NEVER `fcvt`-converted. Getting only one side of that would be worse
// than getting neither, so every crossing is symmetric: bitcast on write,
// bitcast on read. `Float32` occupies the low 32 bits of the word, its own
// 32-bit pattern zero-extended.
//
// Integer-class types (`Bool`/i8, `Char`/i32, `Int32`, …) keep the existing
// width-extension: they already share the integer register class with the
// word, which is precisely why only `Float` was corrupted.
//
// The one crossing that is NOT an erasure is Ruxen's implicit numeric
// conversion into a **declared** scalar parameter (`def f(x: Float)` called
// as `f(3)`, `def g(x: Int)` called as `g(2.5)`). Those must stay `fcvt`.
// `is_erased_word_ty` is what tells the two apart, from the DECLARED `Ty` —
// the machine class alone cannot, because a declared `Int` slot and an
// erased word are both `I64`.

/// True when a declared Ruxen type is stored as an OPAQUE 64-bit uniform
/// word rather than as a value of its own numeric class.
///
/// Every primitive scalar (the integer family, the float family, `Bool`,
/// `Char`) is concrete: its declared type IS its representation, so a
/// mismatch against it is a real numeric conversion. Everything else —
/// `TypeParam`, classes, refs, options, collections, function values,
/// pointers — is an opaque word, so a mismatch against it is erasure and
/// must be bit-reinterpreted.
///
/// `Alias` / `Newtype` are deliberately on the word side: they already lower
/// to a word today, so treating them as concrete would change their ABI.
pub fn is_erased_word_ty(ty: &Ty) -> bool {
    !matches!(
        ty,
        Ty::Bool
            | Ty::Int8
            | Ty::UInt8
            | Ty::Int16
            | Ty::UInt16
            | Ty::Int32
            | Ty::UInt32
            | Ty::Char
            | Ty::Int
            | Ty::Int64
            | Ty::UInt
            | Ty::UInt64
            | Ty::ISize
            | Ty::USize
            | Ty::Float
            | Ty::Float32
            | Ty::Float64
    )
}

/// True when a declared Ruxen type is a float, i.e. the only type whose
/// uniform-word representation needs a REGISTER-CLASS change (SSE/vector →
/// integer) rather than a width extension. Used by closure ABI erasure in
/// MIR lowering.
pub fn is_float_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Float | Ty::Float32 | Ty::Float64)
}

/// True when a declared integer type is signed: it sign-extends when
/// widened, and is the target interpretation of a float conversion.
pub fn is_signed_ty(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Int8 | Ty::Int16 | Ty::Int32 | Ty::Int | Ty::Int64 | Ty::ISize | Ty::Char
    )
}

/// Whether a MIR operand is a signed value: an integer or char literal, or
/// a local whose declared type [`is_signed_ty`].
pub fn operand_is_signed(arg: &MirValue, func: &MirFunction) -> bool {
    match arg {
        MirValue::Literal(Literal::Int(_)) | MirValue::Literal(Literal::Char(_)) => true,
        MirValue::Literal(_) | MirValue::Unit => false,
        MirValue::Use(id) => local_ty(func, *id).is_some_and(is_signed_ty),
    }
}

/// True when a MIR operand's DECLARED type is an erased uniform word.
/// Literals are never erased: an integer literal beside a float operand is
/// Ruxen's implicit numeric promotion, not a bit pattern.
pub fn operand_is_erased_word(arg: &MirValue, func: &MirFunction) -> bool {
    match arg {
        MirValue::Use(id) => local_ty(func, *id).is_some_and(is_erased_word_ty),
        _ => false,
    }
}

/// True when a MIR operand is a string-typed local (`Ty::String` or the
/// annotation form `Class{"String"}`, see `Ty::is_string_form`), so a
/// `Compare` on it calls the runtime string comparison instead of
/// comparing pointers.
pub fn is_string_operand(arg: &MirValue, func: &MirFunction) -> bool {
    match arg {
        MirValue::Use(id) => local_ty(func, *id).is_some_and(Ty::is_string_form),
        _ => false,
    }
}

/// The class a backend gives a MIR operand: a local holds its declared
/// type's class (the `I64` word for a type with none, such as `Unit`); an
/// integer literal is an `I64`, a float literal an `F64`, a `Bool` an `I8`,
/// a `Char` an `I32`, a string literal a `Ptr`, and `()` an `I64` zero.
pub fn value_class(value: &MirValue, func: &MirFunction) -> AbiClass {
    match value {
        MirValue::Use(id) => local_ty(func, *id)
            .and_then(AbiClass::of)
            .unwrap_or(AbiClass::I64),
        MirValue::Literal(Literal::Int(_)) | MirValue::Unit => AbiClass::I64,
        MirValue::Literal(Literal::Float(_)) => AbiClass::F64,
        MirValue::Literal(Literal::Bool(_)) => AbiClass::I8,
        MirValue::Literal(Literal::Char(_)) => AbiClass::I32,
        MirValue::Literal(Literal::String(_)) => AbiClass::Ptr,
    }
}

fn local_ty(func: &MirFunction, id: LocalId) -> Option<&Ty> {
    func.locals.get(id as usize).map(|l| &l.ty)
}

/// Size estimate for a heap allocation whose MIR `Alloc.size` is 0.
///
/// Classes and structs MUST carry their precomputed size (`Lowerer::alloc_size`
/// walks the parent chain to budget every inherited field); reaching this
/// estimator with one is a missing precompute at the MIR lowering site, so
/// it panics rather than silently truncate the allocation.
pub fn simple_type_size(ty: &Ty) -> usize {
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
        // Tag (8 bytes aligned) + up to 3 payload words.
        Ty::Enum { .. } => 32,
        Ty::Class { name, .. } | Ty::Struct { name, .. } => panic!(
            "simple_type_size: class/struct `{name}` reached the codegen size-estimator \
             fallback. MIR Alloc.size MUST be set by `Lowerer::alloc_size`; a fallback \
             would silently truncate classes with more than 8 fields."
        ),
        // Tag + payload.
        Ty::Option(_) | Ty::Result(_, _) => 16,
        Ty::Tuple(elems) => elems.len().max(1) * 8,
        // `n` is a `ConstExpr`; only a literal length resolves here.
        Ty::FixedArray(_, n) => n.as_lit().unwrap_or(0) as usize * 8,
        _ => 8,
    }
}

// ─── Value coercion ──────────────────────────────────────────────────────────

/// How a value crosses into a slot of another class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A numeric conversion. `signed` is the SOURCE's signedness for an
    /// integer widening or an integer to float conversion, and the
    /// TARGET's for a float to integer conversion.
    Numeric {
        /// See the variant docs for which side it describes.
        signed: bool,
    },
    /// An erased uniform-word crossing (B01): a float moves by its bit
    /// pattern, never by a numeric conversion.
    Word,
}

impl Mode {
    /// A numeric conversion that zero-extends.
    pub const UNSIGNED: Mode = Mode::Numeric { signed: false };
}

/// The one operation a backend emits to move a value of one class into
/// another, decided by [`coercion`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coercion {
    /// Same class: nothing to emit.
    Same,
    /// Integer to a narrower integer.
    Truncate,
    /// Integer to a wider integer.
    Extend {
        /// Sign-extend (`true`) or zero-extend.
        signed: bool,
    },
    /// `F32` to `F64`.
    FloatPromote,
    /// `F64` to `F32`.
    FloatDemote,
    /// Integer to float.
    IntToFloat {
        /// Read the integer as signed.
        signed: bool,
    },
    /// Float to integer, SATURATING on every backend: an out-of-range value
    /// clamps to the target's bounds and NaN becomes 0.
    FloatToInt {
        /// Convert to a signed (`true`) or unsigned integer.
        signed: bool,
    },
    /// Integer to pointer; a narrower integer widens first.
    IntToPtr {
        /// Sign-extend (`true`) or zero-extend a narrower integer.
        signed: bool,
    },
    /// Pointer to integer.
    PtrToInt,
    /// Reinterpret a float as the integer class `int` of its own width,
    /// then coerce that integer to the target ([`Mode::UNSIGNED`]).
    FloatToBits {
        /// The float's same-width integer class.
        int: AbiClass,
    },
    /// Coerce the value to the integer class `int` ([`Mode::UNSIGNED`]),
    /// then reinterpret those bits as the target float.
    BitsToFloat {
        /// The target float's same-width integer class.
        int: AbiClass,
    },
    /// No conversion exists between the classes: the value passes unchanged.
    Keep,
}

/// The coercion of a `from`-class value into a `to`-class slot under `mode`.
pub fn coercion(from: AbiClass, to: AbiClass, mode: Mode) -> Coercion {
    if from == to {
        return Coercion::Same;
    }
    if mode == Mode::Word {
        if from.is_float() && !to.is_float() {
            return Coercion::FloatToBits {
                int: from.same_width_int(),
            };
        }
        if to.is_float() && !from.is_float() {
            return Coercion::BitsToFloat {
                int: to.same_width_int(),
            };
        }
    }
    let signed = matches!(mode, Mode::Numeric { signed: true });
    let narrows = from.bits() > to.bits();
    if from.is_int() && to.is_int() {
        if narrows {
            Coercion::Truncate
        } else {
            Coercion::Extend { signed }
        }
    } else if from.is_float() && to.is_float() {
        if narrows {
            Coercion::FloatDemote
        } else {
            Coercion::FloatPromote
        }
    } else if from.is_int() && to.is_float() {
        Coercion::IntToFloat { signed }
    } else if from.is_float() && to.is_int() {
        Coercion::FloatToInt { signed }
    } else if from.is_int() && to == AbiClass::Ptr {
        Coercion::IntToPtr { signed }
    } else if from == AbiClass::Ptr && to.is_int() {
        Coercion::PtrToInt
    } else {
        Coercion::Keep
    }
}

/// The mode of `Assign { dest, value }`. `Assign` is both the home of
/// Ruxen's numeric `as` cast and the general value-into-local move, told
/// apart by the DECLARED types: a cast has a concrete scalar on both sides,
/// a crossing has an opaque word on one (assigning to a `&var Float`
/// parameter, whose local is `RefMut(Float)`). An integer to float cast
/// reads the source's signedness; a float to integer cast the destination's.
pub fn assign_mode(
    value: &MirValue,
    dest: LocalId,
    func: &MirFunction,
    from: AbiClass,
    to: AbiClass,
) -> Mode {
    let dest_ty = local_ty(func, dest);
    if dest_ty.is_some_and(is_erased_word_ty) || operand_is_erased_word(value, func) {
        Mode::Word
    } else if from.is_int() && to.is_float() {
        Mode::Numeric {
            signed: operand_is_signed(value, func),
        }
    } else if from.is_float() && to.is_int() {
        Mode::Numeric {
            signed: dest_ty.is_some_and(is_signed_ty),
        }
    } else {
        Mode::UNSIGNED
    }
}

/// The mode of passing `arg` (of class `from`) to a parameter of class `to`.
/// `param` is the callee's declared parameter, `None` for a callee no
/// declaration describes.
///
/// A float into an integer or pointer slot is an erasure when the slot is a
/// declared uniform word (`def push as "ruxen_vec_push"(x: T)`), or when no
/// declaration says otherwise: the reinterpretation keeps `[1.5].push` a
/// `1.5`. Into a declared concrete integer (`def f(x: Int)` called as
/// `f(2.5)`) it is Ruxen's implicit numeric conversion, signed as the
/// parameter is. Every other crossing extends or converts by the argument's
/// own signedness, so a negative `Int32` widens to a negative `Int`.
pub fn call_arg_mode(
    arg: &MirValue,
    func: &MirFunction,
    from: AbiClass,
    to: AbiClass,
    param: Option<ParamAbi>,
) -> Mode {
    if from.is_float() && !to.is_float() {
        match param {
            Some(p) if !p.word => Mode::Numeric { signed: p.signed },
            _ => Mode::Word,
        }
    } else {
        Mode::Numeric {
            signed: operand_is_signed(arg, func),
        }
    }
}

/// The mode of returning a value from `func`: a generic body declared
/// `-> T` returns through an erased word, so a float result is
/// reinterpreted; a declared numeric return keeps the numeric path.
pub fn return_mode(func: &MirFunction) -> Mode {
    if is_erased_word_ty(&func.return_ty) {
        Mode::Word
    } else {
        Mode::UNSIGNED
    }
}

/// Which operand of a binary operation [`unify_operands`] converts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unify {
    /// Coerce the right operand to the left's class; the left's is common.
    RhsToLhs(Mode),
    /// Coerce the left operand to the right's class; the right's is common.
    LhsToRhs(Mode),
}

/// Bring a binary operation's operands to one class. Normally the left
/// operand's class wins and the right converts numerically (`0.0 + n`).
/// When exactly one side is a float and the other's DECLARED type is an
/// erased word, that word holds the float's bit pattern (inside
/// `class Float`, `self` is the FFI shell `Class{"Float"}`), so it is
/// reinterpreted as the float, whichever side it is on.
pub fn unify_operands(
    l: AbiClass,
    r: AbiClass,
    lhs: &MirValue,
    rhs: &MirValue,
    func: &MirFunction,
) -> Unify {
    if l.is_float() && !r.is_float() && operand_is_erased_word(rhs, func) {
        Unify::RhsToLhs(Mode::Word)
    } else if r.is_float() && !l.is_float() && operand_is_erased_word(lhs, func) {
        Unify::LhsToRhs(Mode::Word)
    } else {
        Unify::RhsToLhs(Mode::UNSIGNED)
    }
}

/// The class of the value `inst` produces and the mode it lands in its
/// `dest` under; `None` when `inst` defines nothing, calls a callee with no
/// result, or produces `dest`'s own class by construction (a `GetField` or
/// `StackAlloc` typed by `dest`, a `CallIndirect` whose signature returns
/// `dest`'s class, an `Assign` or `Convert`).
///
/// An operation result lands numerically: an arithmetic or bitwise `BinOp`,
/// a `Negate` or a `Not` produces its (unified) operand's class, a pointer
/// operand being its `I64` address; a comparison `BinOp` and a `Compare`
/// produce the `I8` `Bool`; `GetTag` the `I32` tag; an address (`Alloc`,
/// `StringLiteral`, `FuncAddr`, `DataAddr`, `FieldAddr`, `GetPayload`,
/// `RefMut`) a `Ptr`.
///
/// A direct call's result has the callee's return class (the `I64` word for
/// a callee no declaration describes), and `Copy`, `Move`, `Ref`,
/// `FloatToBits` and `BitsToFloat` produce their source's class; these land
/// as an erased-word crossing (B01). A call result never carries an
/// implicit numeric conversion, since MIR gives its `dest` the callee's
/// declared return and a promotion is a separate `Assign`, so a mismatch
/// means an erased return slot; a value moved between locals of different
/// classes is a word crossing too.
pub fn produced(
    inst: &MirInst,
    func: &MirFunction,
    callees: &CalleeIndex,
) -> Option<(AbiClass, Mode)> {
    let address = |class: AbiClass| {
        if class == AbiClass::Ptr {
            AbiClass::I64
        } else {
            class
        }
    };
    let src_class = |src: LocalId| value_class(&MirValue::Use(src), func);
    let class = match inst {
        MirInst::Compare { .. } => AbiClass::I8,
        MirInst::BinOp { op, .. } if op.is_comparison() => AbiClass::I8,
        MirInst::BinOp { lhs: operand, .. }
        | MirInst::Negate { operand, .. }
        | MirInst::Not { operand, .. } => address(value_class(operand, func)),
        MirInst::GetTag { .. } => AbiClass::I32,
        MirInst::Alloc { .. }
        | MirInst::StringLiteral { .. }
        | MirInst::FuncAddr { .. }
        | MirInst::DataAddr { .. }
        | MirInst::FieldAddr { .. }
        | MirInst::GetPayload { .. }
        | MirInst::RefMut { .. } => AbiClass::Ptr,
        MirInst::Call {
            dest: Some(_),
            callee,
            ..
        } => {
            // An unresolvable callee is the backend's error to report.
            let name = runtime_name(callee).ok()?;
            let ret = callees.get(name).map_or(Some(AbiClass::I64), |c| c.abi.ret);
            return ret.map(|class| (class, Mode::Word));
        }
        MirInst::Copy { src, .. }
        | MirInst::Move { src, .. }
        | MirInst::Ref { src, .. }
        | MirInst::FloatToBits { src, .. }
        | MirInst::BitsToFloat { src, .. } => return Some((src_class(*src), Mode::Word)),
        _ => return None,
    };
    Some((class, Mode::UNSIGNED))
}

// ─── Signatures and callees ──────────────────────────────────────────────────

/// One parameter slot of a callee.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamAbi {
    /// Its machine class.
    pub class: AbiClass,
    /// The declared type is an erased uniform word ([`is_erased_word_ty`]).
    pub word: bool,
    /// The declared type is a signed integer ([`is_signed_ty`]).
    pub signed: bool,
}

impl ParamAbi {
    /// The slot of a declared parameter type; `None` when it has no runtime
    /// representation, so a signature skips it.
    fn of(ty: &Ty) -> Option<ParamAbi> {
        AbiClass::of(ty).map(|class| ParamAbi {
            class,
            word: is_erased_word_ty(ty),
            signed: is_signed_ty(ty),
        })
    }
}

/// A function's machine signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnAbi {
    /// Parameter slots; zero-sized parameters have none.
    pub params: Vec<ParamAbi>,
    /// Return class; `None` for no value.
    pub ret: Option<AbiClass>,
    /// Takes C varargs after `params`.
    pub variadic: bool,
}

impl FnAbi {
    /// The signature of a MIR function. The program entry (`is_entry`)
    /// returns the C `main` exit status, an `I32`, whatever its body returns.
    pub fn of_function(func: &MirFunction) -> FnAbi {
        let ret = if func.is_entry {
            Some(AbiClass::I32)
        } else {
            AbiClass::of(&func.return_ty)
        };
        FnAbi {
            params: func
                .params
                .iter()
                .filter_map(|&id| local_ty(func, id).and_then(ParamAbi::of))
                .collect(),
            ret,
            variadic: false,
        }
    }

    /// The signature of a `lib`-declared function, derived from its
    /// declared types.
    pub fn of_ffi(decl: &FfiFuncDecl) -> FnAbi {
        FnAbi {
            params: decl.param_types.iter().filter_map(ParamAbi::of).collect(),
            ret: decl.return_type.as_ref().and_then(AbiClass::of),
            variadic: decl.is_variadic,
        }
    }

    /// The classes of the parameter slots, in order.
    pub fn param_classes(&self) -> impl Iterator<Item = AbiClass> + '_ {
        self.params.iter().map(|p| p.class)
    }
}

/// Whether a callee is linked in or defined by the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalleeKind {
    /// Declared with import linkage: a `lib` function or a runtime symbol
    /// the compiler emits itself (`residual_abi`).
    Import,
    /// A MIR function of the program.
    Defined,
}

/// One callee: the symbol a call links against and its signature.
#[derive(Clone, Debug)]
pub struct Callee {
    /// The symbol to declare and call (an FFI's C symbol).
    pub link_name: String,
    /// Its signature.
    pub abi: FnAbi,
    /// Import or program-defined.
    pub kind: CalleeKind,
}

/// Every callee a program can call, by every name a MIR call can spell, built
/// once per program. An FFI function answers to its C symbol, its Ruxen name
/// and its lib-qualified alias (`LibM_sin`); a MIR function to its name; a
/// compiler-emitted runtime symbol (`residual_abi`) to its name unless a
/// `lib` block declares it, in which case the declared ABI wins.
#[derive(Clone, Debug)]
pub struct CalleeIndex {
    callees: Vec<Callee>,
    by_name: HashMap<String, usize>,
}

impl Default for CalleeIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl CalleeIndex {
    /// An index of the compiler-emitted runtime symbols only: a program's
    /// placeholder until `for_program` indexes it.
    pub fn new() -> CalleeIndex {
        let mut index = CalleeIndex {
            callees: Vec::with_capacity(RESIDUAL_RUNTIME_FNS.len()),
            by_name: HashMap::with_capacity(RESIDUAL_RUNTIME_FNS.len()),
        };
        index.add_residual();
        index
    }

    /// The index of `program`: its FFI declarations, its functions, and the
    /// compiler-emitted runtime symbols no `lib` block declares.
    pub fn for_program(program: &MirProgram) -> CalleeIndex {
        let mut index = CalleeIndex {
            callees: Vec::new(),
            by_name: HashMap::new(),
        };
        for lib in &program.ffi_libs {
            for decl in &lib.functions {
                // The first declaration of a C symbol wins; a conflicting
                // redeclaration is rejected upstream (E0722).
                let id = match index.by_name.get(&decl.name) {
                    Some(&id) => id,
                    None => index.push(Callee {
                        link_name: decl.name.clone(),
                        abi: FnAbi::of_ffi(decl),
                        kind: CalleeKind::Import,
                    }),
                };
                index.alias(&decl.ruxen_name, id);
                if !lib.name.is_empty() {
                    index.alias(&format!("{}_{}", lib.name, decl.name), id);
                }
            }
        }
        for func in &program.functions {
            index.insert_function(func);
        }
        index.add_residual();
        index
    }

    /// Add (or replace) a MIR function under its name.
    fn insert_function(&mut self, func: &MirFunction) {
        let id = self.push(Callee {
            link_name: func.name.clone(),
            abi: FnAbi::of_function(func),
            kind: CalleeKind::Defined,
        });
        self.by_name.insert(func.name.clone(), id);
    }

    /// The callee a call to `name` reaches.
    pub fn get(&self, name: &str) -> Option<&Callee> {
        self.by_name.get(name).map(|&id| &self.callees[id])
    }

    /// Every import, once per symbol, FFI declarations first.
    pub fn imports(&self) -> impl Iterator<Item = &Callee> {
        self.callees.iter().filter(|c| c.kind == CalleeKind::Import)
    }

    fn push(&mut self, callee: Callee) -> usize {
        let id = self.callees.len();
        self.by_name.entry(callee.link_name.clone()).or_insert(id);
        self.callees.push(callee);
        id
    }

    fn alias(&mut self, name: &str, id: usize) {
        self.by_name.entry(name.to_string()).or_insert(id);
    }

    fn add_residual(&mut self) {
        for r in RESIDUAL_RUNTIME_FNS {
            if self.by_name.contains_key(r.name) {
                continue;
            }
            self.push(Callee {
                link_name: r.name.to_string(),
                abi: FnAbi {
                    // The residual's slots are uniform words by construction.
                    params: r
                        .params
                        .iter()
                        .map(|&class| ParamAbi {
                            class,
                            word: true,
                            signed: false,
                        })
                        .collect(),
                    ret: r.ret,
                    variadic: false,
                },
                kind: CalleeKind::Import,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_narrow_int_argument_sign_extends() {
        assert_eq!(
            coercion(AbiClass::I32, AbiClass::I64, Mode::Numeric { signed: true }),
            Coercion::Extend { signed: true }
        );
    }

    #[test]
    fn float_into_word_moves_bits_and_into_int_converts() {
        assert_eq!(
            coercion(AbiClass::F64, AbiClass::I64, Mode::Word),
            Coercion::FloatToBits { int: AbiClass::I64 }
        );
        assert_eq!(
            coercion(AbiClass::F32, AbiClass::Ptr, Mode::Word),
            Coercion::FloatToBits { int: AbiClass::I32 }
        );
        assert_eq!(
            coercion(AbiClass::Ptr, AbiClass::F64, Mode::Word),
            Coercion::BitsToFloat { int: AbiClass::I64 }
        );
        assert_eq!(
            coercion(AbiClass::F64, AbiClass::I64, Mode::Numeric { signed: true }),
            Coercion::FloatToInt { signed: true }
        );
    }

    #[test]
    fn pointer_crossings() {
        assert_eq!(
            coercion(AbiClass::I64, AbiClass::Ptr, Mode::UNSIGNED),
            Coercion::IntToPtr { signed: false }
        );
        // A negative `Int32` receiver into a `Ptr`-class scalar shell
        // `self` keeps its sign.
        assert_eq!(
            coercion(AbiClass::I32, AbiClass::Ptr, Mode::Numeric { signed: true }),
            Coercion::IntToPtr { signed: true }
        );
        assert_eq!(
            coercion(AbiClass::Ptr, AbiClass::I8, Mode::Word),
            Coercion::PtrToInt
        );
    }
}
