//! The instruction-level contract every backend implements (F-29 P4).
//!
//! [`crate::codegen::lower::lower_function`] walks a MIR function once and
//! emits it through an [`IrBuilder`]; a backend supplies only the machine
//! operations below. Every decision about WHAT to emit (which operation a
//! `BinOp` is, how a string comparison runs, where a field lives, how a
//! zero-sized parameter binds) is the walk's; HOW a backend spells it (its
//! value types, its storage for a local, a missing machine instruction) is
//! the builder's. The walk is generic, so each backend gets its own
//! monomorphised copy: no dynamic dispatch per instruction.
//!
//! Values carry the class MIR gave them (`mir::abi::value_class`); an
//! operation never receives operands of two classes, since every crossing
//! is an explicit `MirInst::Convert` emitted through [`IrBuilder::coerce`].

use crate::mir::abi::{AbiClass, Coercion};
use crate::mir::nodes::{CmpOp, LocalId};

/// An integer (or pointer-word) operation; signed where it matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntOp {
    Add,
    Sub,
    Mul,
    /// Signed division.
    Div,
    /// Signed (truncated) remainder.
    Rem,
    And,
    Or,
    Xor,
    Shl,
    /// Arithmetic (sign-propagating) right shift.
    Shr,
}

/// A float operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatOp {
    Add,
    Sub,
    Mul,
    Div,
    /// The truncated remainder (C `fmod`, LLVM `frem`).
    Rem,
}

/// The machine operations one MIR function lowers to, for one backend.
///
/// A builder is created per function, positioned so the walk can declare
/// its locals; the walk then creates the blocks, binds the parameters,
/// enters the body and emits every block in order.
pub trait IrBuilder {
    /// An SSA value of the backend.
    type Value: Copy;
    /// A basic block of the backend.
    type Block: Copy;
    /// A callable function reference.
    type Func: Copy;

    // ── Frame ────────────────────────────────────────────────────────────

    /// A new, empty block.
    fn create_block(&mut self) -> Self::Block;
    /// Declare local `id` (declared in id order) of machine class `class`.
    /// `address_taken`: a `MirInst::RefMut` takes its address, so
    /// [`Self::local_addr`] must work for it for the whole frame.
    fn declare_local(&mut self, id: LocalId, class: AbiClass, address_taken: bool);
    /// The incoming parameter values, one per sized parameter, readable
    /// from the frame's start (`entry` is the body's first block).
    fn entry_params(&mut self, entry: Self::Block) -> Result<Vec<Self::Value>, String>;
    /// Continue into the body's first block once parameters are bound.
    fn enter_body(&mut self, entry: Self::Block) -> Result<(), String>;
    /// Emit into `block` from here on.
    fn switch_to_block(&mut self, block: Self::Block);
    /// Whether [`Self::set_location`] records anything, so the walk skips
    /// computing locations for a backend that keeps none.
    fn tracks_locations(&self) -> bool {
        false
    }
    /// The source line and column of what is emitted next.
    fn set_location(&mut self, _line: u32, _column: u32) {}

    // ── Locals ───────────────────────────────────────────────────────────

    /// Store `value` (of the local's class) into local `id`.
    fn def_local(&mut self, id: LocalId, value: Self::Value) -> Result<(), String>;
    /// The current value of local `id`, of its class.
    fn use_local(&mut self, id: LocalId) -> Result<Self::Value, String>;
    /// The address of local `id`'s storage (declared `address_taken`).
    fn local_addr(&mut self, id: LocalId) -> Result<Self::Value, String>;

    // ── Constants ────────────────────────────────────────────────────────

    /// An integer constant of integer class `class`.
    fn iconst(&mut self, class: AbiClass, value: i64) -> Self::Value;
    /// An `F64` constant.
    fn f64const(&mut self, value: f64) -> Self::Value;
    /// The zero (null, `0.0`) of `class`.
    fn zero(&mut self, class: AbiClass) -> Self::Value;

    // ── Operations ───────────────────────────────────────────────────────

    /// `lhs op rhs` on two integers (or pointer words) of one class.
    fn int_op(&mut self, op: IntOp, lhs: Self::Value, rhs: Self::Value) -> Self::Value;
    /// `lhs op rhs` on two floats of one class.
    fn float_op(
        &mut self,
        op: FloatOp,
        lhs: Self::Value,
        rhs: Self::Value,
    ) -> Result<Self::Value, String>;
    /// A signed integer (or address) comparison, as an `I8` Bool.
    fn int_compare(&mut self, op: CmpOp, lhs: Self::Value, rhs: Self::Value) -> Self::Value;
    /// A float comparison (`==` ordered, `!=` unordered), as an `I8` Bool.
    fn float_compare(&mut self, op: CmpOp, lhs: Self::Value, rhs: Self::Value) -> Self::Value;
    /// Integer negation.
    fn int_negate(&mut self, value: Self::Value) -> Self::Value;
    /// Float negation.
    fn float_negate(&mut self, value: Self::Value) -> Self::Value;
    /// Emit a `mir::abi::Coercion` of `value` into class `to`.
    fn coerce(&mut self, value: Self::Value, coercion: Coercion, to: AbiClass) -> Self::Value;

    // ── Memory ───────────────────────────────────────────────────────────

    /// `base + offset` bytes.
    fn ptr_offset(&mut self, base: Self::Value, offset: i64) -> Self::Value;
    /// Load a value of `class` from `addr`.
    fn load(&mut self, class: AbiClass, addr: Self::Value) -> Result<Self::Value, String>;
    /// Store `value` at `addr`.
    fn store(&mut self, addr: Self::Value, value: Self::Value) -> Result<(), String>;

    // ── Symbols and calls ────────────────────────────────────────────────

    /// The function a call or address names: resolved through the callee
    /// index, or declared from the call site (`args`, `has_return`) for a
    /// callee no declaration describes.
    fn callee(
        &mut self,
        name: &str,
        args: &[Self::Value],
        has_return: bool,
    ) -> Result<Self::Func, String>;
    /// A runtime function the compiler emits calls to itself.
    fn runtime_callee(&mut self, name: &str) -> Result<Self::Func, String>;
    /// Call `func`; its result, if it returns one.
    fn call(
        &mut self,
        func: Self::Func,
        args: &[Self::Value],
    ) -> Result<Option<Self::Value>, String>;
    /// Call the function `callee` points at, with a signature of the
    /// arguments' classes returning `ret`.
    fn call_indirect(
        &mut self,
        callee: Self::Value,
        args: &[Self::Value],
        ret: Option<AbiClass>,
    ) -> Result<Option<Self::Value>, String>;
    /// The address of `func`.
    fn func_addr(&mut self, func: Self::Func) -> Self::Value;
    /// The address of a declared vtable / class_info data symbol.
    fn data_addr(&mut self, symbol: &str) -> Result<Self::Value, String>;
    /// The address of a NUL-terminated copy of `value` (one per text).
    fn string_literal(&mut self, value: &str) -> Result<Self::Value, String>;

    // ── Terminators ──────────────────────────────────────────────────────

    /// Return `value` (of the signature's class), or nothing.
    fn ret(&mut self, value: Option<Self::Value>) -> Result<(), String>;
    /// Jump to `target`.
    fn jump(&mut self, target: Self::Block) -> Result<(), String>;
    /// Branch on `cond` (an integer or pointer; non-zero is true).
    fn branch(
        &mut self,
        cond: Self::Value,
        then_block: Self::Block,
        else_block: Self::Block,
    ) -> Result<(), String>;
    /// Jump to the block of `value`'s case, else to `otherwise`.
    fn switch(
        &mut self,
        value: Self::Value,
        cases: &[(i64, Self::Block)],
        otherwise: Self::Block,
    ) -> Result<(), String>;
    /// End the block as unreachable (a trap where the backend has one).
    fn unreachable(&mut self) -> Result<(), String>;
}
