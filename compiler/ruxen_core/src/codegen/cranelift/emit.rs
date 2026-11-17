//! Cranelift's [`IrBuilder`]: how this backend spells each operation of the
//! one MIR walk (`codegen::lower`), plus its signatures and `Coercion`s.
//!
//! A local is a Cranelift `Variable`; one whose address a `RefMut` takes
//! lives in an 8-byte stack slot for the whole frame instead, holding the
//! uniform word (`slot_word`), so a callee's write through the address is
//! what the next read sees.

use cranelift_codegen::ir::types::{self, Type};
use cranelift_codegen::ir::{
    AbiParam, Block, FuncRef, InstBuilder, MemFlags, Signature, StackSlot, StackSlotData,
    StackSlotKind, TrapCode, Value,
};
use cranelift_frontend::{FunctionBuilder, Switch, Variable};
use cranelift_module::Module;

use crate::codegen::ir_builder::{FloatOp, IntOp, IrBuilder};
use crate::mir::abi::{self, AbiClass, Coercion, FnAbi, Mode};
use crate::mir::nodes::{CmpOp, LocalId, MirFunction};

use super::helpers::{clif_class, clif_signature, clif_type, cmpop_to_floatcc, cmpop_to_intcc};
use super::translation_env::TranslationEnv;

/// Build a Cranelift `Signature` from a MIR function (`abi::FnAbi`).
pub fn build_signature<M: Module>(module: &M, func: &MirFunction) -> Signature {
    clif_signature(module.isa().default_call_conv(), &FnAbi::of_function(func))
}

/// One local's storage: its variable, or its `&var` stack slot.
struct ClifLocal {
    var: Variable,
    ty: Type,
    slot: Option<StackSlot>,
}

/// The [`IrBuilder`] of one Cranelift function: its `FunctionBuilder`, the
/// module state calls and data need, and each local's storage by id.
pub struct ClifBuilder<'a, 'f, M: Module> {
    pub builder: FunctionBuilder<'f>,
    pub env: TranslationEnv<'a, M>,
    locals: Vec<ClifLocal>,
}

impl<'a, 'f, M: Module> ClifBuilder<'a, 'f, M> {
    pub fn new(builder: FunctionBuilder<'f>, env: TranslationEnv<'a, M>) -> Self {
        ClifBuilder {
            builder,
            env,
            locals: Vec::new(),
        }
    }

    fn local(&self, id: LocalId) -> Result<&ClifLocal, String> {
        self.locals
            .get(id as usize)
            .ok_or_else(|| format!("Unknown local {id}"))
    }

    fn ins(&mut self) -> cranelift_frontend::FuncInstBuilder<'_, 'f> {
        self.builder.ins()
    }
}

impl<'a, 'f, M: Module> IrBuilder for ClifBuilder<'a, 'f, M> {
    type Value = Value;
    type Block = Block;
    type Func = FuncRef;

    fn create_block(&mut self) -> Block {
        self.builder.create_block()
    }

    fn declare_local(&mut self, id: LocalId, class: AbiClass, address_taken: bool) {
        debug_assert_eq!(id as usize, self.locals.len(), "locals declare in id order");
        let ty = clif_type(class);
        let var = self.builder.declare_var(ty);
        let slot = address_taken.then(|| {
            self.builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                8,
                3, // log2(align) = 3 → 8-byte alignment
            ))
        });
        self.locals.push(ClifLocal { var, ty, slot });
    }

    /// The body's first block is the function's entry, holding the
    /// parameters as block parameters.
    fn entry_params(&mut self, entry: Block) -> Result<Vec<Value>, String> {
        self.builder.switch_to_block(entry);
        self.builder.append_block_params_for_function_params(entry);
        Ok(self.builder.block_params(entry).to_vec())
    }

    fn enter_body(&mut self, _entry: Block) -> Result<(), String> {
        Ok(())
    }

    fn switch_to_block(&mut self, block: Block) {
        // The entry block is current already, holding the parameter binding.
        if self.builder.current_block() != Some(block) {
            self.builder.switch_to_block(block);
        }
    }

    /// B01: the 8-byte slot IS an erased uniform word, so a `Float` is
    /// stored by its bit pattern (`slot_word`), not converted.
    fn def_local(&mut self, id: LocalId, val: Value) -> Result<(), String> {
        let (var, slot) = {
            let local = self.local(id)?;
            (local.var, local.slot)
        };
        match slot {
            Some(slot) => {
                let val_ty = self.builder.func.dfg.value_type(val);
                let word =
                    emit_coercion(val, slot_word(val_ty, true), types::I64, &mut self.builder);
                self.ins().stack_store(word, slot, 0);
            }
            None => self.builder.def_var(var, val),
        }
        Ok(())
    }

    /// The inverse of `def_local`: a slot's word is reinterpreted back to
    /// the local's declared type.
    fn use_local(&mut self, id: LocalId) -> Result<Value, String> {
        let (var, ty, slot) = {
            let local = self.local(id)?;
            (local.var, local.ty, local.slot)
        };
        Ok(match slot {
            Some(slot) => {
                let word = self.ins().stack_load(types::I64, slot, 0);
                emit_coercion(word, slot_word(ty, false), ty, &mut self.builder)
            }
            None => self.builder.use_var(var),
        })
    }

    fn local_addr(&mut self, id: LocalId) -> Result<Value, String> {
        let slot = self.local(id)?.slot.ok_or_else(|| {
            format!("`RefMut` source local {id} has no stack slot (not address-taken)")
        })?;
        Ok(self.ins().stack_addr(types::I64, slot, 0))
    }

    fn iconst(&mut self, class: AbiClass, value: i64) -> Value {
        self.ins().iconst(clif_type(class), value)
    }

    fn f64const(&mut self, value: f64) -> Value {
        self.ins().f64const(value)
    }

    fn zero(&mut self, class: AbiClass) -> Value {
        match class {
            AbiClass::F64 => self.ins().f64const(0.0),
            AbiClass::F32 => self.ins().f32const(0.0),
            _ => self.ins().iconst(clif_type(class), 0),
        }
    }

    fn int_op(&mut self, op: IntOp, l: Value, r: Value) -> Value {
        let ins = self.ins();
        match op {
            IntOp::Add => ins.iadd(l, r),
            IntOp::Sub => ins.isub(l, r),
            IntOp::Mul => ins.imul(l, r),
            IntOp::Div => ins.sdiv(l, r),
            IntOp::Rem => ins.srem(l, r),
            IntOp::And => ins.band(l, r),
            IntOp::Or => ins.bor(l, r),
            IntOp::Xor => ins.bxor(l, r),
            IntOp::Shl => ins.ishl(l, r),
            IntOp::Shr => ins.sshr(l, r),
        }
    }

    fn float_op(&mut self, op: FloatOp, l: Value, r: Value) -> Result<Value, String> {
        Ok(match op {
            FloatOp::Add => self.ins().fadd(l, r),
            FloatOp::Sub => self.ins().fsub(l, r),
            FloatOp::Mul => self.ins().fmul(l, r),
            FloatOp::Div => self.ins().fdiv(l, r),
            // Cranelift has no float remainder instruction, so `%` calls
            // libm `fmod`: the same truncated remainder as LLVM `frem`.
            FloatOp::Rem => {
                let fmod = self.env.declare_runtime_func(
                    "fmod",
                    &[types::F64, types::F64],
                    Some(types::F64),
                    &mut self.builder,
                )?;
                let call = self.ins().call(fmod, &[l, r]);
                self.builder.inst_results(call)[0]
            }
        })
    }

    fn int_compare(&mut self, op: CmpOp, l: Value, r: Value) -> Value {
        self.ins().icmp(cmpop_to_intcc(op), l, r)
    }

    fn float_compare(&mut self, op: CmpOp, l: Value, r: Value) -> Value {
        self.ins().fcmp(cmpop_to_floatcc(op), l, r)
    }

    fn int_negate(&mut self, val: Value) -> Value {
        self.ins().ineg(val)
    }

    fn float_negate(&mut self, val: Value) -> Value {
        self.ins().fneg(val)
    }

    fn coerce(&mut self, val: Value, coercion: Coercion, to: AbiClass) -> Value {
        emit_coercion(val, coercion, clif_type(to), &mut self.builder)
    }

    fn ptr_offset(&mut self, base: Value, offset: i64) -> Value {
        self.ins().iadd_imm(base, offset)
    }

    fn load(&mut self, class: AbiClass, addr: Value) -> Result<Value, String> {
        Ok(self.ins().load(clif_type(class), MemFlags::new(), addr, 0))
    }

    fn store(&mut self, addr: Value, val: Value) -> Result<(), String> {
        self.ins().store(MemFlags::new(), val, addr, 0);
        Ok(())
    }

    fn callee(&mut self, name: &str, args: &[Value], has_return: bool) -> Result<FuncRef, String> {
        self.env
            .get_or_declare_func(name, args, has_return, &mut self.builder)
    }

    fn runtime_callee(&mut self, name: &str) -> Result<FuncRef, String> {
        self.env.runtime_func(name, &mut self.builder)
    }

    fn call(&mut self, func: FuncRef, args: &[Value]) -> Result<Option<Value>, String> {
        let call = self.ins().call(func, args);
        Ok(self.builder.inst_results(call).first().copied())
    }

    fn call_indirect(
        &mut self,
        callee: Value,
        args: &[Value],
        ret: Option<AbiClass>,
    ) -> Result<Option<Value>, String> {
        let mut sig = Signature::new(self.env.module.isa().default_call_conv());
        for &arg in args {
            sig.params
                .push(AbiParam::new(self.builder.func.dfg.value_type(arg)));
        }
        sig.returns.extend(ret.map(|c| AbiParam::new(clif_type(c))));
        let sig_ref = self.builder.import_signature(sig);
        let call = self.ins().call_indirect(sig_ref, callee, args);
        Ok(self.builder.inst_results(call).first().copied())
    }

    fn func_addr(&mut self, func: FuncRef) -> Value {
        self.ins().func_addr(types::I64, func)
    }

    fn data_addr(&mut self, symbol: &str) -> Result<Value, String> {
        let data_id = *self.env.vtable_data.get(symbol).ok_or_else(|| {
            format!(
                "mixin-vtables: DataAddr for unknown data symbol '{symbol}' — \
                 `__rx_vtable_*` / `__rx_classinfo_*` should have been declared first"
            )
        })?;
        let gv = self
            .env
            .module
            .declare_data_in_func(data_id, self.builder.func);
        Ok(self.ins().symbol_value(types::I64, gv))
    }

    fn string_literal(&mut self, value: &str) -> Result<Value, String> {
        let data_id = self.env.create_string_data(value)?;
        let gv = self
            .env
            .module
            .declare_data_in_func(data_id, self.builder.func);
        Ok(self.ins().global_value(types::I64, gv))
    }

    fn ret(&mut self, val: Option<Value>) -> Result<(), String> {
        match val {
            Some(val) => self.ins().return_(&[val]),
            None => self.ins().return_(&[]),
        };
        Ok(())
    }

    fn jump(&mut self, target: Block) -> Result<(), String> {
        self.ins().jump(target, &[]);
        Ok(())
    }

    fn branch(&mut self, cond: Value, then_block: Block, else_block: Block) -> Result<(), String> {
        self.ins().brif(cond, then_block, &[], else_block, &[]);
        Ok(())
    }

    fn switch(
        &mut self,
        val: Value,
        cases: &[(i64, Block)],
        otherwise: Block,
    ) -> Result<(), String> {
        let mut switch = Switch::new();
        for &(discriminant, block) in cases {
            switch.set_entry(discriminant as u128, block);
        }
        switch.emit(&mut self.builder, val, otherwise);
        Ok(())
    }

    fn unreachable(&mut self) -> Result<(), String> {
        self.ins()
            .trap(TrapCode::user(1).expect("1 is a valid user trap code"));
        Ok(())
    }
}

/// Emit `coercion` of `val` into a value of `target_ty`.
fn emit_coercion(
    val: Value,
    coercion: Coercion,
    target_ty: Type,
    builder: &mut FunctionBuilder,
) -> Value {
    let ins = builder.ins();
    match coercion {
        Coercion::Same | Coercion::Keep => val,
        // A pointer is an `I64` here, so a pointer crossing only resizes
        // the integer.
        Coercion::IntToPtr { signed } => resize_int(val, target_ty, signed, builder),
        Coercion::PtrToInt => resize_int(val, target_ty, false, builder),
        Coercion::Truncate => ins.ireduce(target_ty, val),
        Coercion::Extend { signed: true } => ins.sextend(target_ty, val),
        Coercion::Extend { signed: false } => ins.uextend(target_ty, val),
        Coercion::FloatPromote => ins.fpromote(target_ty, val),
        Coercion::FloatDemote => ins.fdemote(target_ty, val),
        Coercion::IntToFloat { signed: true } => ins.fcvt_from_sint(target_ty, val),
        Coercion::IntToFloat { signed: false } => ins.fcvt_from_uint(target_ty, val),
        Coercion::FloatToInt { signed: true } => ins.fcvt_to_sint_sat(target_ty, val),
        Coercion::FloatToInt { signed: false } => ins.fcvt_to_uint_sat(target_ty, val),
        // The bits zero-extend into, or truncate out of, the word.
        Coercion::FloatToBits { int } => {
            let bits = ins.bitcast(clif_type(int), MemFlags::new(), val);
            resize_int(bits, target_ty, false, builder)
        }
        Coercion::BitsToFloat { int } => {
            let bits = resize_int(val, clif_type(int), false, builder);
            builder.ins().bitcast(target_ty, MemFlags::new(), bits)
        }
    }
}

/// Extend (`signed`: sign-extend) or truncate an integer to `target_ty`'s
/// width.
fn resize_int(val: Value, target_ty: Type, signed: bool, builder: &mut FunctionBuilder) -> Value {
    let bits = builder.func.dfg.value_type(val).bits();
    if bits < target_ty.bits() && signed {
        builder.ins().sextend(target_ty, val)
    } else if bits < target_ty.bits() {
        builder.ins().uextend(target_ty, val)
    } else if bits > target_ty.bits() {
        builder.ins().ireduce(target_ty, val)
    } else {
        val
    }
}

/// How a value of type `ty` moves into (`into_slot`) or out of the 8-byte
/// stack slot an address-taken local lives in. The slot
/// is this backend's storage, not a Ruxen-level crossing (LLVM keeps each
/// local in an alloca of its own type), so MIR has no `Convert` for it: it
/// holds the uniform word (B01), a float by its bit pattern.
fn slot_word(ty: Type, into_slot: bool) -> Coercion {
    let (class, word) = (clif_class(ty), AbiClass::I64);
    if into_slot {
        abi::coercion(class, word, Mode::Word)
    } else {
        abi::coercion(word, class, Mode::Word)
    }
}
