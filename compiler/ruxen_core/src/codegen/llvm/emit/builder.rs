//! LLVM's [`IrBuilder`]: how this backend spells each operation of the one
//! MIR walk (`codegen::lower`).
//!
//! A local is an alloca of its class's type in the function's `entry`
//! block (LLVM's mem2reg promotes it to SSA), so every local is
//! addressable. An integer-class value (an erased word) used where LLVM
//! needs a pointer is converted here, at the use: `inttoptr` for an
//! address, base or indirect callee, `ptrtoint` for a pointer operand of an
//! integer operation. These are spellings of this IR, not Ruxen crossings,
//! so MIR has no `Convert` for them.
//!
//! inkwell's `build_*` calls fail only on an unpositioned builder, and the
//! walk positions it before emitting anything (`new`, `switch_to_block`);
//! the infallible trait methods therefore `expect` on that invariant.

use std::collections::HashMap;

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::debug_info::DISubprogram;
use inkwell::module::{Linkage, Module};
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValue, BasicValueEnum, FunctionValue, GlobalValue, IntValue,
    PointerValue,
};
use inkwell::{AddressSpace, FloatPredicate, IntPredicate};

use crate::codegen::ir_builder::{FloatOp, IntOp, IrBuilder};
use crate::mir::abi::{AbiClass, CalleeIndex, Coercion};
use crate::mir::nodes::{CmpOp, LocalId, MirFunction};

use super::super::debug::DebugCtx;
use super::super::types::llvm_type;

const POSITIONED: &str = "the walk positions the builder before emitting";

/// The [`IrBuilder`] of one LLVM function: its instruction builder, the
/// module state calls and data need, and each local's alloca by id.
pub(super) struct LlvmBuilder<'a, 'ctx> {
    builder: Builder<'ctx>,
    module: &'a Module<'ctx>,
    context: &'ctx Context,
    function: FunctionValue<'ctx>,
    callees: &'a CalleeIndex,
    /// One global per string text, shared by the whole program.
    strings: &'a mut HashMap<String, GlobalValue<'ctx>>,
    /// The debug-info builder and this function's scope, when it has one.
    debug: Option<(&'a DebugCtx<'ctx>, DISubprogram<'ctx>)>,
    /// Each local's alloca and its type, by id.
    locals: Vec<(PointerValue<'ctx>, BasicTypeEnum<'ctx>)>,
}

impl<'a, 'ctx> LlvmBuilder<'a, 'ctx> {
    /// A builder positioned in `func`'s new `entry` block, where every
    /// alloca lives. Attaches `func`'s `DISubprogram` when debug info is on
    /// and it has a source span (a synthesized helper has neither scope
    /// nor locations, as the LLVM verifier expects).
    pub(super) fn new(
        func: &MirFunction,
        module: &'a Module<'ctx>,
        context: &'ctx Context,
        callees: &'a CalleeIndex,
        strings: &'a mut HashMap<String, GlobalValue<'ctx>>,
        debug: Option<&'a DebugCtx<'ctx>>,
    ) -> Result<Self, String> {
        let function = module
            .get_function(&func.name)
            .ok_or_else(|| format!("Function '{}' was not declared", func.name))?;
        let debug = debug.and_then(|dctx| {
            let local_to_unit = matches!(function.get_linkage(), Linkage::Internal);
            dctx.function_scope(func, local_to_unit).map(|scope| {
                function.set_subprogram(scope);
                (dctx, scope)
            })
        });
        let builder = context.create_builder();
        builder.position_at_end(context.append_basic_block(function, "entry"));
        Ok(LlvmBuilder {
            builder,
            module,
            context,
            function,
            callees,
            strings,
            debug,
            locals: Vec::with_capacity(func.locals.len()),
        })
    }

    fn local(&self, id: LocalId) -> Result<(PointerValue<'ctx>, BasicTypeEnum<'ctx>), String> {
        self.locals
            .get(id as usize)
            .copied()
            .ok_or_else(|| format!("Unknown local {id}"))
    }

    fn ptr_type(&self) -> inkwell::types::PointerType<'ctx> {
        self.context.ptr_type(AddressSpace::default())
    }

    /// `value` as an address: an integer-class word converts by `inttoptr`
    /// (zero-extending or truncating to the target's pointer width).
    fn address(&self, value: BasicValueEnum<'ctx>) -> PointerValue<'ctx> {
        match value {
            BasicValueEnum::PointerValue(p) => p,
            other => self
                .builder
                .build_int_to_ptr(other.into_int_value(), self.ptr_type(), "i2p")
                .expect(POSITIONED),
        }
    }

    /// An integer operand of an integer operation. A pointer operand
    /// (closure or heap-handle identity, a comparison against nil, a
    /// word-typed temporary) is its address as an `i64`: on wasm32 a
    /// pointer is 32 bits, so this also gives both operands one width.
    /// Address comparison is the correct semantics for pointer `==`/`!=`
    /// (tier 4.09).
    fn int_operand(&self, value: BasicValueEnum<'ctx>) -> IntValue<'ctx> {
        match value {
            BasicValueEnum::PointerValue(p) => self
                .builder
                .build_ptr_to_int(p, self.context.i64_type(), "p2i")
                .expect(POSITIONED),
            other => other.into_int_value(),
        }
    }

    /// An `i1` comparison widened to the `I8` Bool.
    fn bool_of(&self, bit: IntValue<'ctx>) -> BasicValueEnum<'ctx> {
        self.builder
            .build_int_z_extend(bit, self.context.i8_type(), "bool")
            .expect(POSITIONED)
            .into()
    }
}

fn int_predicate(op: CmpOp) -> IntPredicate {
    match op {
        CmpOp::Eq => IntPredicate::EQ,
        CmpOp::NotEq => IntPredicate::NE,
        CmpOp::Lt => IntPredicate::SLT,
        CmpOp::LtEq => IntPredicate::SLE,
        CmpOp::Gt => IntPredicate::SGT,
        CmpOp::GtEq => IntPredicate::SGE,
    }
}

/// `==` is ordered and `!=` unordered, as on Cranelift.
fn float_predicate(op: CmpOp) -> FloatPredicate {
    match op {
        CmpOp::Eq => FloatPredicate::OEQ,
        CmpOp::NotEq => FloatPredicate::UNE,
        CmpOp::Lt => FloatPredicate::OLT,
        CmpOp::LtEq => FloatPredicate::OLE,
        CmpOp::Gt => FloatPredicate::OGT,
        CmpOp::GtEq => FloatPredicate::OGE,
    }
}

fn metadata_args<'ctx>(args: &[BasicValueEnum<'ctx>]) -> Vec<BasicMetadataValueEnum<'ctx>> {
    args.iter().map(|&a| a.into()).collect()
}

fn param_types<'ctx>(args: &[BasicValueEnum<'ctx>]) -> Vec<BasicMetadataTypeEnum<'ctx>> {
    args.iter().map(|a| a.get_type().into()).collect()
}

impl<'a, 'ctx> IrBuilder for LlvmBuilder<'a, 'ctx> {
    type Value = BasicValueEnum<'ctx>;
    type Block = BasicBlock<'ctx>;
    type Func = FunctionValue<'ctx>;

    fn create_block(&mut self) -> BasicBlock<'ctx> {
        self.context.append_basic_block(self.function, "bb")
    }

    /// Every local is an alloca, so `address_taken` needs nothing more.
    fn declare_local(&mut self, id: LocalId, class: AbiClass, _address_taken: bool) {
        debug_assert_eq!(id as usize, self.locals.len(), "locals declare in id order");
        let ty = llvm_type(class, self.context);
        let slot = self.builder.build_alloca(ty, "").expect(POSITIONED);
        self.locals.push((slot, ty));
    }

    fn entry_params(&mut self, _entry: BasicBlock<'ctx>) -> Result<Vec<Self::Value>, String> {
        Ok(self.function.get_param_iter().collect())
    }

    /// The allocas and parameter stores stay in `entry`, which falls
    /// through to the body.
    fn enter_body(&mut self, entry: BasicBlock<'ctx>) -> Result<(), String> {
        self.jump(entry)
    }

    fn switch_to_block(&mut self, block: BasicBlock<'ctx>) {
        self.builder.position_at_end(block);
    }

    fn tracks_locations(&self) -> bool {
        self.debug.is_some()
    }

    fn set_location(&mut self, line: u32, column: u32) {
        if let Some((dctx, scope)) = self.debug {
            self.builder.set_current_debug_location(dctx.location(
                self.context,
                line,
                column,
                scope,
            ));
        }
    }

    fn def_local(&mut self, id: LocalId, value: Self::Value) -> Result<(), String> {
        let (slot, _) = self.local(id)?;
        self.builder
            .build_store(slot, value)
            .map_err(|e| format!("Failed to store local {id}: {e:?}"))?;
        Ok(())
    }

    fn use_local(&mut self, id: LocalId) -> Result<Self::Value, String> {
        let (slot, ty) = self.local(id)?;
        self.builder
            .build_load(ty, slot, "")
            .map_err(|e| format!("Failed to load local {id}: {e:?}"))
    }

    fn local_addr(&mut self, id: LocalId) -> Result<Self::Value, String> {
        Ok(self.local(id)?.0.into())
    }

    /// A `Ptr` constant is that address (`inttoptr` of the integer).
    fn iconst(&mut self, class: AbiClass, value: i64) -> Self::Value {
        match class {
            AbiClass::Ptr => self
                .context
                .i64_type()
                .const_int(value as u64, true)
                .const_to_pointer(self.ptr_type())
                .into(),
            _ => llvm_type(class, self.context)
                .into_int_type()
                .const_int(value as u64, true)
                .into(),
        }
    }

    fn f64const(&mut self, value: f64) -> Self::Value {
        self.context.f64_type().const_float(value).into()
    }

    /// `Ptr`'s zero is the target's null pointer (32 bits on wasm32).
    fn zero(&mut self, class: AbiClass) -> Self::Value {
        llvm_type(class, self.context).const_zero()
    }

    fn int_op(&mut self, op: IntOp, lhs: Self::Value, rhs: Self::Value) -> Self::Value {
        let (l, r) = (self.int_operand(lhs), self.int_operand(rhs));
        let b = &self.builder;
        match op {
            IntOp::Add => b.build_int_add(l, r, "add"),
            IntOp::Sub => b.build_int_sub(l, r, "sub"),
            IntOp::Mul => b.build_int_mul(l, r, "mul"),
            IntOp::Div => b.build_int_signed_div(l, r, "sdiv"),
            IntOp::Rem => b.build_int_signed_rem(l, r, "srem"),
            IntOp::And => b.build_and(l, r, "and"),
            IntOp::Or => b.build_or(l, r, "or"),
            IntOp::Xor => b.build_xor(l, r, "xor"),
            IntOp::Shl => b.build_left_shift(l, r, "shl"),
            IntOp::Shr => b.build_right_shift(l, r, true, "ashr"),
        }
        .expect(POSITIONED)
        .into()
    }

    fn float_op(
        &mut self,
        op: FloatOp,
        lhs: Self::Value,
        rhs: Self::Value,
    ) -> Result<Self::Value, String> {
        let (l, r) = (lhs.into_float_value(), rhs.into_float_value());
        let b = &self.builder;
        match op {
            FloatOp::Add => b.build_float_add(l, r, "fadd"),
            FloatOp::Sub => b.build_float_sub(l, r, "fsub"),
            FloatOp::Mul => b.build_float_mul(l, r, "fmul"),
            FloatOp::Div => b.build_float_div(l, r, "fdiv"),
            FloatOp::Rem => b.build_float_rem(l, r, "frem"),
        }
        .map(Into::into)
        .map_err(|e| format!("Failed to build float {op:?}: {e:?}"))
    }

    fn int_compare(&mut self, op: CmpOp, lhs: Self::Value, rhs: Self::Value) -> Self::Value {
        let (l, r) = (self.int_operand(lhs), self.int_operand(rhs));
        let bit = self
            .builder
            .build_int_compare(int_predicate(op), l, r, "cmp")
            .expect(POSITIONED);
        self.bool_of(bit)
    }

    fn float_compare(&mut self, op: CmpOp, lhs: Self::Value, rhs: Self::Value) -> Self::Value {
        let bit = self
            .builder
            .build_float_compare(
                float_predicate(op),
                lhs.into_float_value(),
                rhs.into_float_value(),
                "fcmp",
            )
            .expect(POSITIONED);
        self.bool_of(bit)
    }

    fn int_negate(&mut self, value: Self::Value) -> Self::Value {
        let int = self.int_operand(value);
        self.builder
            .build_int_neg(int, "neg")
            .expect(POSITIONED)
            .into()
    }

    fn float_negate(&mut self, value: Self::Value) -> Self::Value {
        self.builder
            .build_float_neg(value.into_float_value(), "fneg")
            .expect(POSITIONED)
            .into()
    }

    fn coerce(&mut self, val: Self::Value, coercion: Coercion, to: AbiClass) -> Self::Value {
        let target_ty = llvm_type(to, self.context);
        let b = &self.builder;
        match coercion {
            Coercion::Same | Coercion::Keep => val,
            Coercion::Truncate => b
                .build_int_truncate(val.into_int_value(), target_ty.into_int_type(), "trunc")
                .expect(POSITIONED)
                .into(),
            Coercion::Extend { signed: true } => b
                .build_int_s_extend(val.into_int_value(), target_ty.into_int_type(), "sext")
                .expect(POSITIONED)
                .into(),
            Coercion::Extend { signed: false } => b
                .build_int_z_extend(val.into_int_value(), target_ty.into_int_type(), "zext")
                .expect(POSITIONED)
                .into(),
            Coercion::FloatPromote => b
                .build_float_ext(val.into_float_value(), target_ty.into_float_type(), "fext")
                .expect(POSITIONED)
                .into(),
            Coercion::FloatDemote => b
                .build_float_trunc(
                    val.into_float_value(),
                    target_ty.into_float_type(),
                    "ftrunc",
                )
                .expect(POSITIONED)
                .into(),
            Coercion::IntToFloat { signed: true } => b
                .build_signed_int_to_float(
                    val.into_int_value(),
                    target_ty.into_float_type(),
                    "sitofp",
                )
                .expect(POSITIONED)
                .into(),
            Coercion::IntToFloat { signed: false } => b
                .build_unsigned_int_to_float(
                    val.into_int_value(),
                    target_ty.into_float_type(),
                    "uitofp",
                )
                .expect(POSITIONED)
                .into(),
            // Saturating (clamp, NaN -> 0), as on every backend.
            Coercion::FloatToInt { signed } => {
                let name = if signed {
                    "llvm.fptosi.sat"
                } else {
                    "llvm.fptoui.sat"
                };
                let intrinsic = inkwell::intrinsics::Intrinsic::find(name)
                    .unwrap_or_else(|| panic!("LLVM 18 provides `{name}`"));
                let decl = intrinsic
                    .get_declaration(self.module, &[target_ty, val.get_type()])
                    .unwrap_or_else(|| panic!("`{name}` is overloaded on its int and float types"));
                b.build_call(decl, &[val.into()], "fp2int_sat")
                    .expect(POSITIONED)
                    .try_as_basic_value()
                    .left()
                    .expect("a saturating conversion returns its integer")
            }
            Coercion::IntToPtr { signed } => {
                // `inttoptr` zero-extends a narrower integer; a signed one
                // sign-extends to the word first.
                let int = val.into_int_value();
                let int = if signed && int.get_type().get_bit_width() < 64 {
                    b.build_int_s_extend(int, self.context.i64_type(), "sext")
                        .expect(POSITIONED)
                } else {
                    int
                };
                b.build_int_to_ptr(int, target_ty.into_pointer_type(), "inttoptr")
                    .expect(POSITIONED)
                    .into()
            }
            Coercion::PtrToInt => b
                .build_ptr_to_int(
                    val.into_pointer_value(),
                    target_ty.into_int_type(),
                    "ptrtoint",
                )
                .expect(POSITIONED)
                .into(),
            // The bits zero-extend into, or truncate out of, the word.
            Coercion::FloatToBits { int } => {
                let bits = b
                    .build_bit_cast(val, llvm_type(int, self.context), "word_bits")
                    .expect(POSITIONED)
                    .into_int_value();
                match target_ty {
                    BasicTypeEnum::PointerType(ptr) => b
                        .build_int_to_ptr(bits, ptr, "inttoptr")
                        .expect(POSITIONED)
                        .into(),
                    _ => b
                        .build_int_cast_sign_flag(bits, target_ty.into_int_type(), false, "word")
                        .expect(POSITIONED)
                        .into(),
                }
            }
            Coercion::BitsToFloat { int } => {
                let int = llvm_type(int, self.context).into_int_type();
                let bits = match val {
                    BasicValueEnum::PointerValue(p) => b.build_ptr_to_int(p, int, "ptrtoint"),
                    _ => b.build_int_cast_sign_flag(val.into_int_value(), int, false, "bits"),
                }
                .expect(POSITIONED);
                b.build_bit_cast(bits, target_ty, "word_float")
                    .expect(POSITIONED)
            }
        }
    }

    /// A byte GEP, so the address keeps the target's pointer width.
    fn ptr_offset(&mut self, base: Self::Value, offset: i64) -> Self::Value {
        let base = self.address(base);
        let offset = self.context.i64_type().const_int(offset as u64, true);
        // SAFETY: inkwell marks GEP unsafe because an out-of-bounds index
        // is UB in the emitted IR; MIR's field and payload offsets lie
        // within the object's allocation (`layout::field_slot_offset`).
        unsafe {
            self.builder
                .build_gep(self.context.i8_type(), base, &[offset], "addr")
                .expect(POSITIONED)
        }
        .into()
    }

    fn load(&mut self, class: AbiClass, addr: Self::Value) -> Result<Self::Value, String> {
        let addr = self.address(addr);
        self.builder
            .build_load(llvm_type(class, self.context), addr, "load")
            .map_err(|e| format!("Failed to build load: {e:?}"))
    }

    fn store(&mut self, addr: Self::Value, value: Self::Value) -> Result<(), String> {
        let addr = self.address(addr);
        self.builder
            .build_store(addr, value)
            .map_err(|e| format!("Failed to build store: {e:?}"))?;
        Ok(())
    }

    /// A name the callee index knows resolves to its link symbol, declared
    /// before lowering: an FFI's Ruxen name or lib-qualified alias reaches
    /// its C symbol, so a residual Ruxen name never becomes a new undefined
    /// external. Only a callee no declaration describes takes its
    /// signature from the call.
    fn callee(
        &mut self,
        name: &str,
        args: &[Self::Value],
        has_return: bool,
    ) -> Result<Self::Func, String> {
        if let Some(f) = self.module.get_function(name) {
            return Ok(f);
        }
        if let Some(callee) = self.callees.get(name) {
            return self.module.get_function(&callee.link_name).ok_or_else(|| {
                format!(
                    "function '{name}' resolved to symbol '{}', but the symbol was not declared",
                    callee.link_name
                )
            });
        }
        // An abstract callee (`?T_method`) has no concrete body; MIR never
        // lets one reach codegen (`prune_unmonomorphised_bodies`).
        if name.starts_with('?') {
            return Err(format!(
                "internal error: abstract callee `{name}` reached codegen (its receiver is \
                 still a generic parameter)"
            ));
        }
        let params = param_types(args);
        let fn_type = if has_return {
            self.context.i64_type().fn_type(&params, false)
        } else {
            self.context.void_type().fn_type(&params, false)
        };
        Ok(self
            .module
            .add_function(name, fn_type, Some(Linkage::External)))
    }

    /// Every import, the compiler-emitted runtime symbols included, is
    /// declared before any body is lowered.
    fn runtime_callee(&mut self, name: &str) -> Result<Self::Func, String> {
        self.module.get_function(name).ok_or_else(|| {
            format!("internal error: runtime function `{name}` has no declared or residual ABI")
        })
    }

    fn call(
        &mut self,
        func: Self::Func,
        args: &[Self::Value],
    ) -> Result<Option<Self::Value>, String> {
        let call = self
            .builder
            .build_call(func, &metadata_args(args), "call")
            .map_err(|e| format!("Failed to build call: {e:?}"))?;
        Ok(call.try_as_basic_value().left())
    }

    /// The signature is the arguments' and `ret`'s own types: it must
    /// match the closure's actual type, or wasm `call_indirect` traps.
    fn call_indirect(
        &mut self,
        callee: Self::Value,
        args: &[Self::Value],
        ret: Option<AbiClass>,
    ) -> Result<Option<Self::Value>, String> {
        let callee = self.address(callee);
        let params = param_types(args);
        let fn_type = match ret {
            Some(class) => llvm_type(class, self.context).fn_type(&params, false),
            None => self.context.void_type().fn_type(&params, false),
        };
        let call = self
            .builder
            .build_indirect_call(fn_type, callee, &metadata_args(args), "icall")
            .map_err(|e| format!("Failed to build indirect call: {e:?}"))?;
        Ok(call.try_as_basic_value().left())
    }

    fn func_addr(&mut self, func: Self::Func) -> Self::Value {
        func.as_global_value().as_pointer_value().into()
    }

    fn data_addr(&mut self, symbol: &str) -> Result<Self::Value, String> {
        let global = self.module.get_global(symbol).ok_or_else(|| {
            format!(
                "mixin-vtables: DataAddr for unknown data symbol '{symbol}' — \
                 `__rx_vtable_*` / `__rx_classinfo_*` should have been declared first"
            )
        })?;
        Ok(global.as_pointer_value().into())
    }

    fn string_literal(&mut self, value: &str) -> Result<Self::Value, String> {
        let global = match self.strings.get(value) {
            Some(global) => *global,
            None => {
                let global = self
                    .builder
                    .build_global_string_ptr(value, ".str")
                    .map_err(|e| format!("Failed to build string literal: {e:?}"))?;
                self.strings.insert(value.to_string(), global);
                global
            }
        };
        Ok(global.as_pointer_value().into())
    }

    fn ret(&mut self, value: Option<Self::Value>) -> Result<(), String> {
        let value = value.as_ref().map(|v| v as &dyn BasicValue<'ctx>);
        self.builder
            .build_return(value)
            .map_err(|e| format!("Failed to build return: {e:?}"))?;
        Ok(())
    }

    fn jump(&mut self, target: BasicBlock<'ctx>) -> Result<(), String> {
        self.builder
            .build_unconditional_branch(target)
            .map_err(|e| format!("Failed to build branch: {e:?}"))?;
        Ok(())
    }

    /// `br` takes an `i1`: a pointer is true when non-null, an integer
    /// when non-zero.
    fn branch(
        &mut self,
        cond: Self::Value,
        then_block: BasicBlock<'ctx>,
        else_block: BasicBlock<'ctx>,
    ) -> Result<(), String> {
        let bit = match cond {
            BasicValueEnum::PointerValue(p) => self.builder.build_is_not_null(p, "tobool"),
            other => {
                let int = other.into_int_value();
                self.builder.build_int_compare(
                    IntPredicate::NE,
                    int,
                    int.get_type().const_zero(),
                    "tobool",
                )
            }
        }
        .map_err(|e| format!("Failed to build branch condition: {e:?}"))?;
        self.builder
            .build_conditional_branch(bit, then_block, else_block)
            .map_err(|e| format!("Failed to build branch: {e:?}"))?;
        Ok(())
    }

    fn switch(
        &mut self,
        value: Self::Value,
        cases: &[(i64, BasicBlock<'ctx>)],
        otherwise: BasicBlock<'ctx>,
    ) -> Result<(), String> {
        let int = self.int_operand(value);
        let cases: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> = cases
            .iter()
            .map(|&(discriminant, block)| {
                (int.get_type().const_int(discriminant as u64, true), block)
            })
            .collect();
        self.builder
            .build_switch(int, otherwise, &cases)
            .map_err(|e| format!("Failed to build switch: {e:?}"))?;
        Ok(())
    }

    fn unreachable(&mut self) -> Result<(), String> {
        self.builder
            .build_unreachable()
            .map_err(|e| format!("Failed to build unreachable: {e:?}"))?;
        Ok(())
    }
}
