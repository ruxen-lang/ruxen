//! Free functions — instruction and terminator translation.
//!
//! These free helpers form the actual MIR-to-Cranelift lowering pass.
//! Extracted from the original monolithic `cranelift.rs` for navigability —
//! the contents are otherwise unchanged.

use std::collections::HashMap;

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::types::{self, Type};
use cranelift_codegen::ir::{AbiParam, InstBuilder, MemFlags, Signature, StackSlot};
use cranelift_frontend::{FunctionBuilder, Variable};
use cranelift_module::Module;

use crate::hir::types::Ty;
use crate::mir::nodes::*;
use crate::parser::ast::BinOp;

use crate::codegen::runtime::runtime_name;

use super::helpers::{
    cmpop_to_floatcc, cmpop_to_intcc, is_string_typed_value, simple_type_size, ty_to_cranelift,
};
use super::runtime_sigs::runtime_signature;
use super::translation_env::TranslationEnv;

/// Build a Cranelift `Signature` from a MIR function.
pub fn build_signature<M: Module>(module: &M, func: &MirFunction) -> Signature {
    let call_conv = module.isa().default_call_conv();
    let mut sig = Signature::new(call_conv);

    if func.name == "main" {
        sig.params.push(AbiParam::new(types::I32));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I32));
        return sig;
    }

    for &param_id in &func.params {
        let local = &func.locals[param_id as usize];
        if let Some(cl_ty) = ty_to_cranelift(&local.ty) {
            sig.params.push(AbiParam::new(cl_ty));
        }
    }

    if let Some(ret_ty) = ty_to_cranelift(&func.return_ty) {
        sig.returns.push(AbiParam::new(ret_ty));
    }

    sig
}

/// Write `val` into the storage for `local_id`.
///
/// For address-taken locals (those promoted to a stack slot so that `&mut`
/// can hand out a stable pointer), this stores into the stack slot. For all
/// other locals it updates the Cranelift SSA variable as before. Stored
/// values are always widened to I64 when going through a stack slot, since
/// slots are uniformly 8 bytes wide to match the pointer representation.
pub fn def_local(
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    builder: &mut FunctionBuilder,
    local_id: LocalId,
    val: cranelift_codegen::ir::Value,
) {
    if let Some(&slot) = stack_slots.get(&local_id) {
        let widened = coerce_value(val, types::I64, builder);
        builder.ins().stack_store(widened, slot, 0);
    } else if let Some(&var) = var_map.get(&local_id) {
        builder.def_var(var, val);
    }
}

/// Read the current value of `local_id`.
///
/// Mirrors `def_local`: address-taken locals read from their stack slot so
/// that a mutation written through a `&mut` pointer (stored back via
/// `ruxen_store_ptr`) is visible to subsequent uses. Everything else goes
/// through the Cranelift SSA variable.
pub fn use_local(
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    builder: &mut FunctionBuilder,
    local_id: LocalId,
) -> cranelift_codegen::ir::Value {
    if let Some(&slot) = stack_slots.get(&local_id) {
        builder.ins().stack_load(types::I64, slot, 0)
    } else {
        let var = var_map[&local_id];
        builder.use_var(var)
    }
}

/// Translate a single MIR instruction.
pub fn translate_instruction<M: Module>(
    inst: &MirInst,
    func: &MirFunction,
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    _block_map: &[cranelift_codegen::ir::Block],
    builder: &mut FunctionBuilder,
    env: &mut TranslationEnv<M>,
) -> Result<(), String> {
    match inst {
        MirInst::Assign { dest, value } => {
            let val = gen_value(value, func, var_map, stack_slots, builder)?;
            // Coerce value to match the declared type of the destination local.
            let dest_ty = func
                .locals
                .get(*dest as usize)
                .and_then(|l| ty_to_cranelift(&l.ty))
                .unwrap_or(types::I64);
            let val = coerce_value(val, dest_ty, builder);
            def_local(var_map, stack_slots, builder, *dest, val);
        }

        MirInst::BinOp { dest, op, lhs, rhs } => {
            let l = gen_value(lhs, func, var_map, stack_slots, builder)?;
            let r = gen_value(rhs, func, var_map, stack_slots, builder)?;
            // Ensure both operands have the same type for binop.
            let common_ty = builder.func.dfg.value_type(l);
            let r = coerce_value(r, common_ty, builder);
            let result = emit_binop(*op, l, r, builder);
            let dest_ty = func
                .locals
                .get(*dest as usize)
                .and_then(|l| ty_to_cranelift(&l.ty))
                .unwrap_or(types::I64);
            let result = coerce_value(result, dest_ty, builder);
            def_local(var_map, stack_slots, builder, *dest, result);
        }

        MirInst::Negate { dest, operand } => {
            let val = gen_value(operand, func, var_map, stack_slots, builder)?;
            let result = if builder.func.dfg.value_type(val).is_float() {
                builder.ins().fneg(val)
            } else {
                builder.ins().ineg(val)
            };
            let dest_ty = func
                .locals
                .get(*dest as usize)
                .and_then(|l| ty_to_cranelift(&l.ty))
                .unwrap_or(types::I64);
            let result = coerce_value(result, dest_ty, builder);
            def_local(var_map, stack_slots, builder, *dest, result);
        }

        MirInst::Not { dest, operand } => {
            let val = gen_value(operand, func, var_map, stack_slots, builder)?;
            let val_ty = builder.func.dfg.value_type(val);
            let one = builder.ins().iconst(val_ty, 1);
            let result = builder.ins().bxor(val, one);
            let dest_ty = func
                .locals
                .get(*dest as usize)
                .and_then(|l| ty_to_cranelift(&l.ty))
                .unwrap_or(types::I8);
            let result = coerce_value(result, dest_ty, builder);
            def_local(var_map, stack_slots, builder, *dest, result);
        }

        MirInst::Compare { dest, op, lhs, rhs } => {
            let l = gen_value(lhs, func, var_map, stack_slots, builder)?;
            let r = gen_value(rhs, func, var_map, stack_slots, builder)?;

            // Check if either operand is a string type — if so, use
            // runtime string comparison (strcmp) instead of pointer
            // equality.
            let is_string_compare =
                is_string_typed_value(lhs, func) || is_string_typed_value(rhs, func);

            let result = if is_string_compare && matches!(op, CmpOp::Eq | CmpOp::NotEq) {
                // Call ruxen_string_eq(a, b) which returns 1 for equal, 0 for not.
                let func_ref = env.declare_runtime_func(
                    "ruxen_string_eq",
                    &[types::I64, types::I64],
                    Some(types::I64),
                    builder,
                )?;
                let call = builder.ins().call(func_ref, &[l, r]);
                let eq_result = builder.inst_results(call)[0];
                if matches!(op, CmpOp::NotEq) {
                    // Flip: not_eq = (eq_result == 0)
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.ins().icmp(IntCC::Equal, eq_result, zero)
                } else {
                    // Truncate I64 to I8 for bool
                    builder.ins().ireduce(types::I8, eq_result)
                }
            } else if is_string_compare {
                // For ordered comparisons on strings, call ruxen_string_cmp
                let func_ref = env.declare_runtime_func(
                    "ruxen_string_cmp",
                    &[types::I64, types::I64],
                    Some(types::I64),
                    builder,
                )?;
                let call = builder.ins().call(func_ref, &[l, r]);
                let cmp_result = builder.inst_results(call)[0];
                let zero = builder.ins().iconst(types::I64, 0);
                let cc = cmpop_to_intcc(*op);
                builder.ins().icmp(cc, cmp_result, zero)
            } else {
                // Integer/pointer or float comparison — dispatch by operand type.
                let common_ty = builder.func.dfg.value_type(l);
                let r = coerce_value(r, common_ty, builder);
                if common_ty.is_float() {
                    let cc = cmpop_to_floatcc(*op);
                    builder.ins().fcmp(cc, l, r)
                } else {
                    let cc = cmpop_to_intcc(*op);
                    builder.ins().icmp(cc, l, r)
                }
            };
            // icmp always produces I8; coerce if dest expects something else.
            let dest_ty = func
                .locals
                .get(*dest as usize)
                .and_then(|l| ty_to_cranelift(&l.ty))
                .unwrap_or(types::I8);
            let result = coerce_value(result, dest_ty, builder);
            def_local(var_map, stack_slots, builder, *dest, result);
        }

        MirInst::Call { dest, callee, args } => {
            let mut arg_vals = Vec::with_capacity(args.len());
            for arg in args {
                arg_vals.push(gen_value(arg, func, var_map, stack_slots, builder)?);
            }

            let actual_name = runtime_name(callee)?;

            // Widen narrow-integer arguments to match the callee's expected
            // parameter types. Runtime helpers like `ruxen_puts`,
            // `ruxen_int_to_string`, and `ruxen_string_concat` all expect i64
            // args, but narrow Ruxen types (Char/Int32→i32, UInt8→i8, etc.)
            // would otherwise reach the call as their narrow Cranelift type
            // and fail the IR verifier. We do this BEFORE declaring the
            // function so call-site signature inference also sees widened
            // types for unknown runtime helpers.
            coerce_call_args(
                &mut arg_vals,
                args,
                func,
                actual_name,
                env.user_fn_param_tys,
                builder,
            );

            // Handle inline no-op operations that don't need a real C call.
            match actual_name {
                "ruxen_noop_passthrough" => {
                    // Return the first argument, or zero if no args.
                    if let Some(dest_id) = dest {
                        let dest_ty = func
                            .locals
                            .get(*dest_id as usize)
                            .and_then(|l| ty_to_cranelift(&l.ty))
                            .unwrap_or(types::I64);
                        let val = if !arg_vals.is_empty() {
                            coerce_value(arg_vals[0], dest_ty, builder)
                        } else {
                            builder.ins().iconst(dest_ty, 0)
                        };
                        def_local(var_map, stack_slots, builder, *dest_id, val);
                    }
                    // No actual call needed.
                }
                "ruxen_noop_return_null" => {
                    // Return a null/zero pointer.
                    if let Some(dest_id) = dest {
                        let dest_ty = func
                            .locals
                            .get(*dest_id as usize)
                            .and_then(|l| ty_to_cranelift(&l.ty))
                            .unwrap_or(types::I64);
                        let zero = builder.ins().iconst(dest_ty, 0);
                        def_local(var_map, stack_slots, builder, *dest_id, zero);
                    }
                }
                "ruxen_noop" => {
                    // Do nothing, don't even set a result.
                    if let Some(dest_id) = dest {
                        let dest_ty = func
                            .locals
                            .get(*dest_id as usize)
                            .and_then(|l| ty_to_cranelift(&l.ty))
                            .unwrap_or(types::I64);
                        let zero = builder.ins().iconst(dest_ty, 0);
                        def_local(var_map, stack_slots, builder, *dest_id, zero);
                    }
                }
                _ => {
                    // Normal function call via the runtime or user-defined function.
                    let func_ref =
                        env.get_or_declare_func(actual_name, &arg_vals, dest.is_some(), builder)?;
                    let call = builder.ins().call(func_ref, &arg_vals);

                    if let Some(dest_id) = dest {
                        let results = builder.inst_results(call);
                        if !results.is_empty() {
                            let dest_ty = func
                                .locals
                                .get(*dest_id as usize)
                                .and_then(|l| ty_to_cranelift(&l.ty))
                                .unwrap_or(types::I64);
                            let result = coerce_value(results[0], dest_ty, builder);
                            def_local(var_map, stack_slots, builder, *dest_id, result);
                        }
                    }
                }
            }
        }

        MirInst::Alloc {
            dest,
            ty: alloc_ty,
            size: precomputed_size,
        } => {
            let size = if *precomputed_size > 0 {
                *precomputed_size as i64
            } else {
                simple_type_size(alloc_ty) as i64
            };
            let size_val = builder.ins().iconst(types::I64, size);
            let func_ref =
                env.declare_runtime_func("ruxen_alloc", &[types::I64], Some(types::I64), builder)?;
            let call = builder.ins().call(func_ref, &[size_val]);
            let ptr = builder.inst_results(call)[0];
            def_local(var_map, stack_slots, builder, *dest, ptr);
        }

        MirInst::StackAlloc { dest, .. } => {
            let dest_ty = func
                .locals
                .get(*dest as usize)
                .and_then(|l| ty_to_cranelift(&l.ty))
                .unwrap_or(types::I64);
            let zero = builder.ins().iconst(dest_ty, 0);
            def_local(var_map, stack_slots, builder, *dest, zero);
        }

        MirInst::GetField {
            dest,
            base,
            field_index,
        } => {
            let base_val = use_local(var_map, stack_slots, builder, *base);
            let offset = (*field_index as i64) * 8;
            let addr = builder.ins().iadd_imm(base_val, offset);
            // Load using the declared type of the destination local.
            let dest_ty = func
                .locals
                .get(*dest as usize)
                .and_then(|l| ty_to_cranelift(&l.ty))
                .unwrap_or(types::I64);
            let loaded = builder.ins().load(dest_ty, MemFlags::new(), addr, 0);
            def_local(var_map, stack_slots, builder, *dest, loaded);
        }

        MirInst::SetField {
            base,
            field_index,
            value,
        } => {
            let base_val = use_local(var_map, stack_slots, builder, *base);
            let val = gen_value(value, func, var_map, stack_slots, builder)?;
            let offset = (*field_index as i64) * 8;
            let addr = builder.ins().iadd_imm(base_val, offset);
            builder.ins().store(MemFlags::new(), val, addr, 0);
        }

        MirInst::SetTag { dest, tag } => {
            let ptr = use_local(var_map, stack_slots, builder, *dest);
            let tag_val = builder.ins().iconst(types::I32, *tag as i64);
            builder.ins().store(MemFlags::new(), tag_val, ptr, 0);
        }

        MirInst::GetTag { dest, src } => {
            let ptr = use_local(var_map, stack_slots, builder, *src);
            let tag_val = builder.ins().load(types::I32, MemFlags::new(), ptr, 0);
            def_local(var_map, stack_slots, builder, *dest, tag_val);
        }

        MirInst::GetPayload { dest, src, .. } => {
            let ptr = use_local(var_map, stack_slots, builder, *src);
            let payload_ptr = builder.ins().iadd_imm(ptr, 8);
            def_local(var_map, stack_slots, builder, *dest, payload_ptr);
        }

        MirInst::Ref { dest, src } => {
            // Immutable borrow stays by-value for now: most callers use `&T`
            // purely to read, and keeping it cheap preserves existing
            // behaviour for fixtures that pass `&String`. Only `RefMut`
            // promotes to a real pointer-to-storage below.
            let val = use_local(var_map, stack_slots, builder, *src);
            def_local(var_map, stack_slots, builder, *dest, val);
        }

        MirInst::RefMut { dest, src } => {
            // The pre-scan allocated a stack slot for every RefMut source,
            // so this lookup should always succeed. Take the slot's address
            // as a plain pointer — the callee can load/store through it to
            // mutate the caller's local in place.
            if let Some(&slot) = stack_slots.get(src) {
                let addr = builder.ins().stack_addr(types::I64, slot, 0);
                def_local(var_map, stack_slots, builder, *dest, addr);
            } else {
                // Defensive fallback: if somehow the pre-scan missed this
                // local, fall back to the old by-value semantics rather
                // than panicking in codegen.
                let val = use_local(var_map, stack_slots, builder, *src);
                def_local(var_map, stack_slots, builder, *dest, val);
            }
        }

        MirInst::Copy { dest, src } => {
            let val = use_local(var_map, stack_slots, builder, *src);
            def_local(var_map, stack_slots, builder, *dest, val);
        }

        MirInst::Move { dest, src } => {
            let val = use_local(var_map, stack_slots, builder, *src);
            def_local(var_map, stack_slots, builder, *dest, val);

            let src_ty = func
                .locals
                .get(*src as usize)
                .and_then(|local| ty_to_cranelift(&local.ty))
                .unwrap_or(types::I64);
            let zero = builder.ins().iconst(src_ty, 0);
            def_local(var_map, stack_slots, builder, *src, zero);
        }

        MirInst::Drop { local: _ } => {
            // MirInst::Drop is a marker — the actual `ruxen_dealloc` call
            // is emitted by `insert_drops` in `mir/lower.rs`, gated by the
            // `compute_dealloc_safe_locals` flow analysis. Doing both here
            // would double-free.
        }

        MirInst::StringLiteral { dest, value } => {
            let gv = env.create_string_data(value)?;
            let ptr = env.module.declare_data_in_func(gv, builder.func);
            let val = builder.ins().global_value(types::I64, ptr);
            def_local(var_map, stack_slots, builder, *dest, val);
        }

        MirInst::Nop => {}

        MirInst::FuncAddr { dest, func_name } => {
            let func_ref = env.get_or_declare_func(func_name, &[], true, builder)?;
            let addr = builder.ins().func_addr(types::I64, func_ref);
            def_local(var_map, stack_slots, builder, *dest, addr);
        }

        MirInst::DataAddr { dest, data_sym } => {
            // Phase B-5: look up the pre-declared data symbol (vtable
            // or class_info) by name. The data ID is registered in
            // Pass 1.5 of `compile_program` before any function body
            // is lowered.
            let data_id = *env.vtable_data.get(data_sym).ok_or_else(|| {
                format!(
                    "mixin-vtables: DataAddr for unknown data symbol '{}' — \
                     `__rx_vtable_*` / `__rx_classinfo_*` should have been declared in Pass 1.5",
                    data_sym
                )
            })?;
            let gv = env.module.declare_data_in_func(data_id, builder.func);
            let addr = builder.ins().symbol_value(types::I64, gv);
            def_local(var_map, stack_slots, builder, *dest, addr);
        }

        MirInst::CallIndirect { dest, callee, args } => {
            let callee_val = use_local(var_map, stack_slots, builder, *callee);
            let mut arg_vals = Vec::with_capacity(args.len());
            for arg in args {
                arg_vals.push(gen_value(arg, func, var_map, stack_slots, builder)?);
            }

            // Build signature: param types come from the arg values
            // (which already carry their MIR-declared Cranelift types).
            // The return type was historically hardcoded to I64 — that
            // produces wrong codegen when the indirect call's real
            // return is `I8` (Bool), `I32` (Char), or `F64`: Cranelift
            // either rejects the verifier or reads garbage upper bits.
            // Source-of-truth for the return is the DESTINATION local's
            // MIR type (set at MIR-lowering time by the call site);
            // mirror it into the imported signature.
            let call_conv = env.module.isa().default_call_conv();
            let mut sig = Signature::new(call_conv);
            for val in &arg_vals {
                let ty = builder.func.dfg.value_type(*val);
                sig.params.push(AbiParam::new(ty));
            }
            let dest_cl_ty: Option<Type> = dest.and_then(|d| {
                func.locals
                    .get(d as usize)
                    .and_then(|l| ty_to_cranelift(&l.ty))
            });
            if let Some(t) = dest_cl_ty {
                sig.returns.push(AbiParam::new(t));
            }
            let sig_ref = builder.import_signature(sig);
            let call = builder.ins().call_indirect(sig_ref, callee_val, &arg_vals);

            if let Some(dest_id) = dest {
                let results = builder.inst_results(call);
                if !results.is_empty() {
                    let dest_ty = dest_cl_ty.unwrap_or(types::I64);
                    let result = coerce_value(results[0], dest_ty, builder);
                    def_local(var_map, stack_slots, builder, *dest_id, result);
                }
            }
        }
    }

    Ok(())
}

/// Translate a MIR terminator.
pub fn translate_terminator<M: Module>(
    term: &Terminator,
    func: &MirFunction,
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    block_map: &[cranelift_codegen::ir::Block],
    builder: &mut FunctionBuilder,
    _env: &mut TranslationEnv<M>,
) -> Result<(), String> {
    match term {
        Terminator::Return(val) => {
            if func.name == "main" {
                let zero = builder.ins().iconst(types::I32, 0);
                builder.ins().return_(&[zero]);
            } else {
                match val {
                    Some(v) => {
                        let ret_val = gen_value(v, func, var_map, stack_slots, builder)?;
                        // Coerce to match function's return type.
                        if let Some(ret_ty) = ty_to_cranelift(&func.return_ty) {
                            let ret_val = coerce_value(ret_val, ret_ty, builder);
                            builder.ins().return_(&[ret_val]);
                        } else {
                            builder.ins().return_(&[ret_val]);
                        }
                    }
                    None => {
                        builder.ins().return_(&[]);
                    }
                }
            }
        }

        Terminator::Goto(target) => {
            builder.ins().jump(block_map[*target], &[]);
        }

        Terminator::Branch {
            cond,
            then_block,
            else_block,
        } => {
            let cond_val = gen_value(cond, func, var_map, stack_slots, builder)?;
            builder.ins().brif(
                cond_val,
                block_map[*then_block],
                &[],
                block_map[*else_block],
                &[],
            );
        }

        Terminator::Switch {
            value,
            targets,
            otherwise,
        } => {
            let val = gen_value(value, func, var_map, stack_slots, builder)?;
            let mut switch = cranelift_frontend::Switch::new();
            for &(discriminant, block_id) in targets {
                switch.set_entry(discriminant as u128, block_map[block_id]);
            }
            switch.emit(builder, val, block_map[*otherwise]);
        }

        Terminator::Unreachable => {
            builder
                .ins()
                .trap(cranelift_codegen::ir::TrapCode::user(1).unwrap());
        }
    }

    Ok(())
}

/// Convert a `MirValue` to a Cranelift `Value`.
pub fn gen_value(
    mir_val: &MirValue,
    func: &MirFunction,
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    builder: &mut FunctionBuilder,
) -> Result<cranelift_codegen::ir::Value, String> {
    match mir_val {
        MirValue::Literal(lit) => match lit {
            Literal::Int(n) => Ok(builder.ins().iconst(types::I64, *n)),
            Literal::Float(n) => Ok(builder.ins().f64const(*n)),
            Literal::Bool(b) => Ok(builder.ins().iconst(types::I8, *b as i64)),
            Literal::Char(c) => Ok(builder.ins().iconst(types::I32, *c as i64)),
            Literal::String(_) => Ok(builder.ins().iconst(types::I64, 0)),
        },
        MirValue::Use(local_id) => {
            if !var_map.contains_key(local_id) {
                return Err(format!(
                    "Unknown local {} in function '{}'",
                    local_id, func.name
                ));
            }
            Ok(use_local(var_map, stack_slots, builder, *local_id))
        }
        MirValue::Unit => Ok(builder.ins().iconst(types::I64, 0)),
    }
}

/// Emit a binary operation in Cranelift IR.
///
/// Dispatches to float (`fadd`/`fsub`/…) or integer (`iadd`/`isub`/…)
/// instructions based on the runtime type of the left operand.
pub fn emit_binop(
    op: BinOp,
    lhs: cranelift_codegen::ir::Value,
    rhs: cranelift_codegen::ir::Value,
    builder: &mut FunctionBuilder,
) -> cranelift_codegen::ir::Value {
    let is_float = builder.func.dfg.value_type(lhs).is_float();
    if is_float {
        match op {
            BinOp::Add => builder.ins().fadd(lhs, rhs),
            BinOp::Sub => builder.ins().fsub(lhs, rhs),
            BinOp::Mul => builder.ins().fmul(lhs, rhs),
            BinOp::Div => builder.ins().fdiv(lhs, rhs),
            // Cranelift has no native float remainder; most languages don't
            // expose `%` on floats. Fall back to int rem (will fail verifier
            // if it ever actually runs — surfaced as a compiler error).
            BinOp::Mod => builder.ins().srem(lhs, rhs),
            BinOp::Eq => builder.ins().fcmp(FloatCC::Equal, lhs, rhs),
            BinOp::NotEq => builder.ins().fcmp(FloatCC::NotEqual, lhs, rhs),
            BinOp::Lt => builder.ins().fcmp(FloatCC::LessThan, lhs, rhs),
            BinOp::LtEq => builder.ins().fcmp(FloatCC::LessThanOrEqual, lhs, rhs),
            BinOp::Gt => builder.ins().fcmp(FloatCC::GreaterThan, lhs, rhs),
            BinOp::GtEq => builder.ins().fcmp(FloatCC::GreaterThanOrEqual, lhs, rhs),
            // Bitwise / logical ops aren't valid on floats; caller shouldn't
            // emit these — keep the integer form to preserve old behavior.
            BinOp::BitAnd | BinOp::And => builder.ins().band(lhs, rhs),
            BinOp::BitOr | BinOp::Or => builder.ins().bor(lhs, rhs),
            BinOp::BitXor => builder.ins().bxor(lhs, rhs),
            BinOp::Shl => builder.ins().ishl(lhs, rhs),
            BinOp::Shr => builder.ins().sshr(lhs, rhs),
            // `~=` is desugared at MIR-lower time (Phase 6) into a
            // method call on the Regex handle. If it ever reaches
            // codegen something upstream let it through unlowered.
            BinOp::MatchOp => unreachable!(
                "BinOp::MatchOp should have been desugared to a method call at MIR-lower"
            ),
        }
    } else {
        match op {
            BinOp::Add => builder.ins().iadd(lhs, rhs),
            BinOp::Sub => builder.ins().isub(lhs, rhs),
            BinOp::Mul => builder.ins().imul(lhs, rhs),
            BinOp::Div => builder.ins().sdiv(lhs, rhs),
            BinOp::Mod => builder.ins().srem(lhs, rhs),
            BinOp::BitAnd => builder.ins().band(lhs, rhs),
            BinOp::BitOr => builder.ins().bor(lhs, rhs),
            BinOp::BitXor => builder.ins().bxor(lhs, rhs),
            BinOp::Shl => builder.ins().ishl(lhs, rhs),
            BinOp::Shr => builder.ins().sshr(lhs, rhs),
            BinOp::And => builder.ins().band(lhs, rhs),
            BinOp::Or => builder.ins().bor(lhs, rhs),
            BinOp::Eq => builder.ins().icmp(IntCC::Equal, lhs, rhs),
            BinOp::NotEq => builder.ins().icmp(IntCC::NotEqual, lhs, rhs),
            BinOp::Lt => builder.ins().icmp(IntCC::SignedLessThan, lhs, rhs),
            BinOp::LtEq => builder.ins().icmp(IntCC::SignedLessThanOrEqual, lhs, rhs),
            BinOp::Gt => builder.ins().icmp(IntCC::SignedGreaterThan, lhs, rhs),
            BinOp::GtEq => builder
                .ins()
                .icmp(IntCC::SignedGreaterThanOrEqual, lhs, rhs),
            // See comment on the float branch above — MIR lower must
            // desugar `~=` to a method call before codegen runs.
            BinOp::MatchOp => unreachable!(
                "BinOp::MatchOp should have been desugared to a method call at MIR-lower"
            ),
        }
    }
}

/// Coerce a Cranelift value to a target type if they differ.
///
/// Handles integer width conversions (e.g., I64 → I8, I8 → I64) using
/// `ireduce` (narrowing) or `uextend`/`sextend` (widening).
pub fn coerce_value(
    val: cranelift_codegen::ir::Value,
    target_ty: Type,
    builder: &mut FunctionBuilder,
) -> cranelift_codegen::ir::Value {
    coerce_value_signed(val, target_ty, false, builder)
}

/// Signedness-aware variant of `coerce_value`.
///
/// When widening integers, uses `sextend` if `signed` is true and
/// `uextend` otherwise. This matters for negative values: a signed
/// `-1i32` must become `0xFFFF_FFFF_FFFF_FFFF` when promoted to i64,
/// not `0x0000_0000_FFFF_FFFF`.
pub fn coerce_value_signed(
    val: cranelift_codegen::ir::Value,
    target_ty: Type,
    signed: bool,
    builder: &mut FunctionBuilder,
) -> cranelift_codegen::ir::Value {
    let val_ty = builder.func.dfg.value_type(val);
    if val_ty == target_ty {
        return val;
    }
    // Both are integer types — convert via ireduce or extend.
    if val_ty.is_int() && target_ty.is_int() {
        if val_ty.bits() > target_ty.bits() {
            return builder.ins().ireduce(target_ty, val);
        } else if signed {
            return builder.ins().sextend(target_ty, val);
        } else {
            return builder.ins().uextend(target_ty, val);
        }
    }
    // Float ↔ Float conversion
    if val_ty.is_float() && target_ty.is_float() {
        if val_ty.bits() > target_ty.bits() {
            return builder.ins().fdemote(target_ty, val);
        } else {
            return builder.ins().fpromote(target_ty, val);
        }
    }
    // Int → Float or Float → Int — just keep the value as-is for now
    // (cast semantics would need explicit handling).
    val
}

/// Widen narrow-integer call arguments to match the callee's expected
/// parameter types.
///
/// The Ruxen narrow integer types (`Char`/`Int32` → i32, `UInt8` → i8, etc.)
/// are stored natively in Cranelift at their declared width for memory
/// efficiency. But runtime helpers (`ruxen_puts`, `ruxen_int_to_string`,
/// `ruxen_string_concat`, …) and most user-level callees expect i64 args —
/// passing a narrow value directly fails Cranelift's IR verifier with
/// `arg N has type iXX, expected i64`.
///
/// This helper inspects each MIR argument, pairs it with the expected
/// Cranelift param type (from `runtime_signature` when known), and inserts
/// a sign- or zero-extend using the MIR type's signedness. For callees
/// whose signature isn't known here (user-defined or FFI functions), we
/// widen any sub-i64 integer argument to i64 as a safe default — this
/// matches the default signature inference path in
/// `get_or_declare_func`, which uses i64 everywhere.
pub fn coerce_call_args(
    arg_vals: &mut [cranelift_codegen::ir::Value],
    args: &[MirValue],
    func: &MirFunction,
    callee: &str,
    user_fn_param_tys: &HashMap<String, Vec<Type>>,
    builder: &mut FunctionBuilder,
) {
    // Resolve the callee's signature in priority order:
    //   1. `runtime_signature` — hand-rolled signature table for the C
    //      runtime helpers (`ruxen_*`).  Wins for known runtime fns.
    //   2. `user_fn_param_tys` — recorded at Pass 0/1 of compile_program
    //      for FFI fns and every MIR function in the program.  This
    //      catches synthesized fns like `Bool_fmt` (`(i8, i64) -> ()`)
    //      that legitimately take narrow params.
    //   3. fallback — widen narrow ints to i64 (variadic-style for
    //      unknown imports).
    let known_sig: Option<Vec<Type>> = runtime_signature(callee)
        .map(|(p, _)| p)
        .or_else(|| user_fn_param_tys.get(callee).cloned());
    for (i, arg_val) in arg_vals.iter_mut().enumerate() {
        let val_ty = builder.func.dfg.value_type(*arg_val);

        // Determine the target Cranelift type for this argument.
        let target_ty = match &known_sig {
            Some(params) if i < params.len() => params[i],
            _ => {
                if val_ty.is_int() && val_ty.bits() < 64 {
                    types::I64
                } else {
                    val_ty
                }
            }
        };

        if val_ty == target_ty {
            continue;
        }

        // Infer signedness from the MIR operand's type so that negative
        // signed values sign-extend correctly.
        let signed = mir_arg_is_signed(&args[i], func);
        *arg_val = coerce_value_signed(*arg_val, target_ty, signed, builder);
    }
}

/// Decide whether a MIR argument's integer type is signed, for the purpose
/// of width-extending it at a call boundary.
fn mir_arg_is_signed(arg: &MirValue, func: &MirFunction) -> bool {
    let ty = match arg {
        MirValue::Literal(Literal::Int(_)) => return true,
        MirValue::Literal(Literal::Char(_)) => return true,
        MirValue::Literal(Literal::Bool(_)) => return false,
        MirValue::Literal(Literal::Float(_))
        | MirValue::Literal(Literal::String(_))
        | MirValue::Unit => return false,
        MirValue::Use(local_id) => match func.locals.get(*local_id as usize) {
            Some(local) => &local.ty,
            None => return false,
        },
    };
    matches!(
        ty,
        Ty::Int8 | Ty::Int16 | Ty::Int32 | Ty::Int | Ty::Int64 | Ty::ISize | Ty::Char
    )
}
