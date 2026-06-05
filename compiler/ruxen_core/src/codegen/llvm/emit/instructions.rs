//! Per-MirInst emission: arithmetic, comparisons, allocations,
//! field access, calls, and assorted scalar/memory ops.

use super::calls::{get_or_declare_func, get_or_declare_runtime};
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn translate_instruction<'ctx>(
    inst: &MirInst,
    func: &MirFunction,
    program: &MirProgram,
    local_allocas: &HashMap<LocalId, PointerValue<'ctx>>,
    _block_map: &[BasicBlock<'ctx>],
    builder: &Builder<'ctx>,
    module: &Module<'ctx>,
    context: &'ctx Context,
    string_cache: &mut HashMap<String, GlobalValue<'ctx>>,
) -> Result<(), String> {
    match inst {
        MirInst::Assign { dest, value } => {
            let val = gen_value(value, func, local_allocas, builder, context)?;
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let val = coerce_value(val, dest_ty, builder);
            builder
                .build_store(local_allocas[dest], val)
                .map_err(|e| format!("Failed to store assign: {:?}", e))?;
        }

        MirInst::BinOp { dest, op, lhs, rhs } => {
            let l = gen_value(lhs, func, local_allocas, builder, context)?;
            let r = gen_value(rhs, func, local_allocas, builder, context)?;

            // Coerce rhs to match lhs type
            let r = coerce_value(r, l.get_type(), builder);

            let result = emit_binop(*op, l, r, builder, context)?;
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let result = coerce_value(result, dest_ty, builder);
            builder
                .build_store(local_allocas[dest], result)
                .map_err(|e| format!("Failed to store binop result: {:?}", e))?;
        }

        MirInst::Negate { dest, operand } => {
            let val = gen_value(operand, func, local_allocas, builder, context)?;
            let result = if val.is_float_value() {
                builder
                    .build_float_neg(val.into_float_value(), "fneg")
                    .unwrap()
                    .into()
            } else {
                builder
                    .build_int_neg(val.into_int_value(), "neg")
                    .unwrap()
                    .into()
            };
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let result = coerce_value(result, dest_ty, builder);
            builder
                .build_store(local_allocas[dest], result)
                .map_err(|e| format!("Failed to store negate: {:?}", e))?;
        }

        MirInst::Not { dest, operand } => {
            let val = gen_value(operand, func, local_allocas, builder, context)?;
            let int_val = val.into_int_value();
            let one = int_val.get_type().const_int(1, false);
            let result: BasicValueEnum = builder.build_xor(int_val, one, "not").unwrap().into();
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.i8_type().into());
            let result = coerce_value(result, dest_ty, builder);
            builder
                .build_store(local_allocas[dest], result)
                .map_err(|e| format!("Failed to store not: {:?}", e))?;
        }

        MirInst::Compare { dest, op, lhs, rhs } => {
            let l = gen_value(lhs, func, local_allocas, builder, context)?;
            let r = gen_value(rhs, func, local_allocas, builder, context)?;

            let is_string = is_string_typed_value(lhs, func) || is_string_typed_value(rhs, func);

            let result: BasicValueEnum = if is_string && matches!(op, CmpOp::Eq | CmpOp::NotEq) {
                // String equality via runtime
                let eq_fn = get_or_declare_runtime(module, context, "ruxen_string_eq");
                let l = coerce_value(l, context.ptr_type(AddressSpace::default()).into(), builder);
                let r = coerce_value(r, context.ptr_type(AddressSpace::default()).into(), builder);
                let call = builder
                    .build_call(eq_fn, &[l.into(), r.into()], "streq")
                    .unwrap();
                let eq_result = call.try_as_basic_value().left().unwrap();

                if matches!(op, CmpOp::NotEq) {
                    // Flip: eq_result == 0
                    let zero = context.i64_type().const_int(0, false);
                    let cmp = builder
                        .build_int_compare(
                            IntPredicate::EQ,
                            eq_result.into_int_value(),
                            zero,
                            "notstreq",
                        )
                        .unwrap();
                    // zext i1 -> i8
                    builder
                        .build_int_z_extend(cmp, context.i8_type(), "zext")
                        .unwrap()
                        .into()
                } else {
                    // Truncate i64 -> i8
                    coerce_value(eq_result, context.i8_type().into(), builder)
                }
            } else if is_string {
                // Ordered string comparison via runtime
                let cmp_fn = get_or_declare_runtime(module, context, "ruxen_string_cmp");
                let l = coerce_value(l, context.ptr_type(AddressSpace::default()).into(), builder);
                let r = coerce_value(r, context.ptr_type(AddressSpace::default()).into(), builder);
                let call = builder
                    .build_call(cmp_fn, &[l.into(), r.into()], "strcmp")
                    .unwrap();
                let cmp_result = call.try_as_basic_value().left().unwrap();
                let zero = context.i64_type().const_int(0, false);
                let pred = cmpop_to_intpred(*op);
                let cmp_i1 = builder
                    .build_int_compare(pred, cmp_result.into_int_value(), zero, "cmp")
                    .unwrap();
                builder
                    .build_int_z_extend(cmp_i1, context.i8_type(), "zext")
                    .unwrap()
                    .into()
            } else {
                // Integer/pointer comparison
                let r = coerce_value(r, l.get_type(), builder);
                let pred = cmpop_to_intpred(*op);
                let cmp_i1 = builder
                    .build_int_compare(pred, l.into_int_value(), r.into_int_value(), "cmp")
                    .unwrap();
                // zext i1 -> i8
                builder
                    .build_int_z_extend(cmp_i1, context.i8_type(), "zext")
                    .unwrap()
                    .into()
            };

            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.i8_type().into());
            let result = coerce_value(result, dest_ty, builder);
            builder
                .build_store(local_allocas[dest], result)
                .map_err(|e| format!("Failed to store compare: {:?}", e))?;
        }

        MirInst::Call { dest, callee, args } => {
            let mut arg_vals: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
            for arg in args {
                let val = gen_value(arg, func, local_allocas, builder, context)?;
                arg_vals.push(val.into());
            }

            let actual_name = runtime_name(callee)?;

            // Handle inline no-op operations
            match actual_name {
                "ruxen_noop_passthrough" => {
                    if let Some(dest_id) = dest {
                        let dest_ty = ty_to_llvm(&func.locals[*dest_id as usize].ty, context)
                            .unwrap_or(context.i64_type().into());
                        let val = if !arg_vals.is_empty() {
                            let first: BasicValueEnum = arg_vals[0]
                                .try_into()
                                .unwrap_or(context.i64_type().const_int(0, false).into());
                            coerce_value(first, dest_ty, builder)
                        } else {
                            dest_ty.const_zero()
                        };
                        builder
                            .build_store(local_allocas[dest_id], val)
                            .map_err(|e| format!("Failed to store noop passthrough: {:?}", e))?;
                    }
                }
                "ruxen_noop_return_null" => {
                    if let Some(dest_id) = dest {
                        let dest_ty = ty_to_llvm(&func.locals[*dest_id as usize].ty, context)
                            .unwrap_or(context.i64_type().into());
                        let zero = dest_ty.const_zero();
                        builder
                            .build_store(local_allocas[dest_id], zero)
                            .map_err(|e| format!("Failed to store noop null: {:?}", e))?;
                    }
                }
                "ruxen_noop" => {
                    if let Some(dest_id) = dest {
                        let dest_ty = ty_to_llvm(&func.locals[*dest_id as usize].ty, context)
                            .unwrap_or(context.i64_type().into());
                        let zero = dest_ty.const_zero();
                        builder
                            .build_store(local_allocas[dest_id], zero)
                            .map_err(|e| format!("Failed to store noop: {:?}", e))?;
                    }
                }
                _ => {
                    let callee_fn = get_or_declare_func(
                        actual_name,
                        &arg_vals,
                        dest.is_some(),
                        func,
                        program,
                        module,
                        context,
                    )?;

                    // Coerce arguments to match the declared parameter types
                    let mut coerced_args: Vec<BasicMetadataValueEnum> =
                        Vec::with_capacity(arg_vals.len());
                    for (i, arg) in arg_vals.iter().enumerate() {
                        let arg_val: BasicValueEnum = (*arg)
                            .try_into()
                            .unwrap_or(context.i64_type().const_int(0, false).into());
                        if let Some(param_ty) = callee_fn.get_type().get_param_types().get(i) {
                            let coerced = coerce_value(arg_val, *param_ty, builder);
                            coerced_args.push(coerced.into());
                        } else {
                            coerced_args.push(arg_val.into());
                        }
                    }

                    let call = builder
                        .build_call(callee_fn, &coerced_args, "call")
                        .map_err(|e| {
                            format!("Failed to build call to '{}': {:?}", actual_name, e)
                        })?;

                    if let Some(dest_id) = dest {
                        if let Some(result) = call.try_as_basic_value().left() {
                            let dest_ty = ty_to_llvm(&func.locals[*dest_id as usize].ty, context)
                                .unwrap_or(context.i64_type().into());
                            let result = coerce_value(result, dest_ty, builder);
                            builder
                                .build_store(local_allocas[dest_id], result)
                                .map_err(|e| format!("Failed to store call result: {:?}", e))?;
                        } else {
                            // Void function but we have a dest — store zero
                            let dest_ty = ty_to_llvm(&func.locals[*dest_id as usize].ty, context)
                                .unwrap_or(context.i64_type().into());
                            let zero = dest_ty.const_zero();
                            builder
                                .build_store(local_allocas[dest_id], zero)
                                .map_err(|e| {
                                    format!("Failed to store zero for void call: {:?}", e)
                                })?;
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
                *precomputed_size as u64
            } else {
                simple_type_size(alloc_ty) as u64
            };
            let size_val = context.i64_type().const_int(size, false);
            let alloc_fn = get_or_declare_runtime(module, context, "ruxen_alloc");
            let call = builder
                .build_call(alloc_fn, &[size_val.into()], "alloc")
                .map_err(|e| format!("Failed to build alloc call: {:?}", e))?;
            let ptr = call.try_as_basic_value().left().unwrap();
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.ptr_type(AddressSpace::default()).into());
            let ptr = coerce_value(ptr, dest_ty, builder);
            builder
                .build_store(local_allocas[dest], ptr)
                .map_err(|e| format!("Failed to store alloc result: {:?}", e))?;
        }

        MirInst::StackAlloc { dest, .. } => {
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let zero = dest_ty.const_zero();
            builder
                .build_store(local_allocas[dest], zero)
                .map_err(|e| format!("Failed to store stack alloc: {:?}", e))?;
        }

        MirInst::GetField {
            dest,
            base,
            field_index,
        } => {
            // Load base pointer
            let base_ptr = builder
                .build_load(
                    context.ptr_type(AddressSpace::default()),
                    local_allocas[base],
                    "base_ptr",
                )
                .map_err(|e| format!("Failed to load base ptr: {:?}", e))?
                .into_pointer_value();

            // GEP with byte offset = field_index * 8
            let offset = (*field_index as u64) * 8;
            let addr = unsafe {
                builder
                    .build_gep(
                        context.i8_type(),
                        base_ptr,
                        &[context.i64_type().const_int(offset, false)],
                        "field_addr",
                    )
                    .map_err(|e| format!("Failed to build GEP: {:?}", e))?
            };

            // Load value from the field address
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let loaded = builder
                .build_load(dest_ty, addr, "field")
                .map_err(|e| format!("Failed to load field: {:?}", e))?;
            builder
                .build_store(local_allocas[dest], loaded)
                .map_err(|e| format!("Failed to store field: {:?}", e))?;
        }

        MirInst::SetField {
            base,
            field_index,
            value,
        } => {
            let base_ptr = builder
                .build_load(
                    context.ptr_type(AddressSpace::default()),
                    local_allocas[base],
                    "base_ptr",
                )
                .map_err(|e| format!("Failed to load base ptr: {:?}", e))?
                .into_pointer_value();

            let val = gen_value(value, func, local_allocas, builder, context)?;

            let offset = (*field_index as u64) * 8;
            let addr = unsafe {
                builder
                    .build_gep(
                        context.i8_type(),
                        base_ptr,
                        &[context.i64_type().const_int(offset, false)],
                        "field_addr",
                    )
                    .map_err(|e| format!("Failed to build GEP: {:?}", e))?
            };

            builder
                .build_store(addr, val)
                .map_err(|e| format!("Failed to store field value: {:?}", e))?;
        }

        MirInst::SetTag { dest, tag } => {
            let ptr = builder
                .build_load(
                    context.ptr_type(AddressSpace::default()),
                    local_allocas[dest],
                    "enum_ptr",
                )
                .map_err(|e| format!("Failed to load enum ptr: {:?}", e))?
                .into_pointer_value();

            let tag_val = context.i32_type().const_int(*tag as u64, false);
            builder
                .build_store(ptr, tag_val)
                .map_err(|e| format!("Failed to store tag: {:?}", e))?;
        }

        MirInst::GetTag { dest, src } => {
            let ptr = builder
                .build_load(
                    context.ptr_type(AddressSpace::default()),
                    local_allocas[src],
                    "enum_ptr",
                )
                .map_err(|e| format!("Failed to load enum ptr: {:?}", e))?
                .into_pointer_value();

            let tag_val = builder
                .build_load(context.i32_type(), ptr, "tag")
                .map_err(|e| format!("Failed to load tag: {:?}", e))?;
            builder
                .build_store(local_allocas[dest], tag_val)
                .map_err(|e| format!("Failed to store tag: {:?}", e))?;
        }

        MirInst::GetPayload { dest, src, .. } => {
            let ptr = builder
                .build_load(
                    context.ptr_type(AddressSpace::default()),
                    local_allocas[src],
                    "enum_ptr",
                )
                .map_err(|e| format!("Failed to load enum ptr: {:?}", e))?
                .into_pointer_value();

            // Payload is at offset 8 (past tag + padding)
            let payload_ptr = unsafe {
                builder
                    .build_gep(
                        context.i8_type(),
                        ptr,
                        &[context.i64_type().const_int(8, false)],
                        "payload_ptr",
                    )
                    .map_err(|e| format!("Failed to build GEP for payload: {:?}", e))?
            };

            builder
                .build_store(local_allocas[dest], payload_ptr)
                .map_err(|e| format!("Failed to store payload ptr: {:?}", e))?;
        }

        MirInst::Ref { dest, src } | MirInst::RefMut { dest, src } => {
            // Simple value copy (semantic differences enforced by borrow checker)
            let src_ty = ty_to_llvm(&func.locals[*src as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let val = builder
                .build_load(src_ty, local_allocas[src], "ref")
                .map_err(|e| format!("Failed to load ref src: {:?}", e))?;
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let val = coerce_value(val, dest_ty, builder);
            builder
                .build_store(local_allocas[dest], val)
                .map_err(|e| format!("Failed to store ref: {:?}", e))?;
        }

        MirInst::Copy { dest, src } => {
            let src_ty = ty_to_llvm(&func.locals[*src as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let val = builder
                .build_load(src_ty, local_allocas[src], "copy")
                .map_err(|e| format!("Failed to load copy src: {:?}", e))?;
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let val = coerce_value(val, dest_ty, builder);
            builder
                .build_store(local_allocas[dest], val)
                .map_err(|e| format!("Failed to store copy: {:?}", e))?;
        }

        MirInst::Move { dest, src } => {
            let src_ty = ty_to_llvm(&func.locals[*src as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let val = builder
                .build_load(src_ty, local_allocas[src], "move")
                .map_err(|e| format!("Failed to load move src: {:?}", e))?;
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let val = coerce_value(val, dest_ty, builder);
            builder
                .build_store(local_allocas[dest], val)
                .map_err(|e| format!("Failed to store move: {:?}", e))?;
            builder
                .build_store(local_allocas[src], src_ty.const_zero())
                .map_err(|e| format!("Failed to clear moved-from local: {:?}", e))?;
        }

        MirInst::Drop { local: _ } => {
            // MirInst::Drop is a marker — the actual `ruxen_dealloc` call
            // is emitted by `insert_drops` in `mir/lower.rs`, gated by the
            // `compute_dealloc_safe_locals` flow analysis. Doing both here
            // would double-free.
        }

        MirInst::StringLiteral { dest, value } => {
            let global = if let Some(existing) = string_cache.get(value) {
                *existing
            } else {
                let g = builder
                    .build_global_string_ptr(value, ".str")
                    .map_err(|e| format!("Failed to build string literal: {:?}", e))?;
                string_cache.insert(value.clone(), g);
                g
            };
            let ptr: BasicValueEnum = global.as_pointer_value().into();
            let dest_ty = ty_to_llvm(&func.locals[*dest as usize].ty, context)
                .unwrap_or(context.ptr_type(AddressSpace::default()).into());
            let val = coerce_value(ptr, dest_ty, builder);
            builder
                .build_store(local_allocas[dest], val)
                .map_err(|e| format!("Failed to store string literal: {:?}", e))?;
        }

        MirInst::FuncAddr { dest, func_name } => {
            let target_fn =
                get_or_declare_func(func_name, &[], true, func, program, module, context)?;
            let ptr: BasicValueEnum = target_fn.as_global_value().as_pointer_value().into();
            builder
                .build_store(local_allocas[dest], ptr)
                .map_err(|e| format!("Failed to store func addr: {:?}", e))?;
        }

        MirInst::DataAddr { dest: _, data_sym } => {
            // TODO(mixin-vtables): LLVM backend doesn't yet emit
            // `__rx_vtable_*` / `__rx_classinfo_*` data sections —
            // see the early-out in `codegen/llvm/mod.rs::compile_program`
            // that errors when `program.vtables`/`class_infos` is
            // non-empty. Reaching `DataAddr` here would mean MIR
            // referenced such a symbol despite that guard, which
            // shouldn't happen. Spec §B5.
            return Err(format!(
                "mixin-vtables: LLVM backend cannot lower DataAddr {{ data_sym: '{}' }} \
                 — use the Cranelift backend (default) for code that includes a \
                 `dispatch runtime` mixin.",
                data_sym
            ));
        }

        MirInst::CallIndirect { dest, callee, args } => {
            let callee_ptr = builder
                .build_load(
                    context.ptr_type(AddressSpace::default()),
                    local_allocas[callee],
                    "fn_ptr",
                )
                .map_err(|e| format!("Failed to load callee ptr: {:?}", e))?
                .into_pointer_value();

            let mut arg_vals: Vec<BasicMetadataValueEnum> = Vec::with_capacity(args.len());
            for arg in args {
                let val = gen_value(arg, func, local_allocas, builder, context)?;
                arg_vals.push(val.into());
            }

            // Build function type from arg/ret types
            let param_types: Vec<BasicMetadataTypeEnum> = arg_vals
                .iter()
                .map(|a| {
                    let bv: BasicValueEnum = (*a)
                        .try_into()
                        .unwrap_or(context.i64_type().const_int(0, false).into());
                    bv.get_type().into()
                })
                .collect();

            let fn_type = if dest.is_some() {
                context.i64_type().fn_type(&param_types, false)
            } else {
                context.void_type().fn_type(&param_types, false)
            };

            let call = builder
                .build_indirect_call(fn_type, callee_ptr, &arg_vals, "icall")
                .map_err(|e| format!("Failed to build indirect call: {:?}", e))?;

            if let Some(dest_id) = dest {
                if let Some(result) = call.try_as_basic_value().left() {
                    let dest_ty = ty_to_llvm(&func.locals[*dest_id as usize].ty, context)
                        .unwrap_or(context.i64_type().into());
                    let result = coerce_value(result, dest_ty, builder);
                    builder
                        .build_store(local_allocas[dest_id], result)
                        .map_err(|e| format!("Failed to store indirect call result: {:?}", e))?;
                }
            }
        }

        MirInst::Nop => {}
    }

    Ok(())
}

pub(super) fn emit_binop<'ctx>(
    op: BinOp,
    lhs: BasicValueEnum<'ctx>,
    rhs: BasicValueEnum<'ctx>,
    builder: &Builder<'ctx>,
    context: &'ctx Context,
) -> Result<BasicValueEnum<'ctx>, String> {
    // Float operations
    if lhs.is_float_value() && rhs.is_float_value() {
        let l = lhs.into_float_value();
        let r = rhs.into_float_value();
        return Ok(match op {
            BinOp::Add => builder.build_float_add(l, r, "fadd").unwrap().into(),
            BinOp::Sub => builder.build_float_sub(l, r, "fsub").unwrap().into(),
            BinOp::Mul => builder.build_float_mul(l, r, "fmul").unwrap().into(),
            BinOp::Div => builder.build_float_div(l, r, "fdiv").unwrap().into(),
            BinOp::Mod => builder.build_float_rem(l, r, "frem").unwrap().into(),
            BinOp::Eq => {
                let cmp = builder
                    .build_float_compare(inkwell::FloatPredicate::OEQ, l, r, "feq")
                    .unwrap();
                builder
                    .build_int_z_extend(cmp, context.i8_type(), "zext")
                    .unwrap()
                    .into()
            }
            BinOp::NotEq => {
                let cmp = builder
                    .build_float_compare(inkwell::FloatPredicate::ONE, l, r, "fne")
                    .unwrap();
                builder
                    .build_int_z_extend(cmp, context.i8_type(), "zext")
                    .unwrap()
                    .into()
            }
            BinOp::Lt => {
                let cmp = builder
                    .build_float_compare(inkwell::FloatPredicate::OLT, l, r, "flt")
                    .unwrap();
                builder
                    .build_int_z_extend(cmp, context.i8_type(), "zext")
                    .unwrap()
                    .into()
            }
            BinOp::LtEq => {
                let cmp = builder
                    .build_float_compare(inkwell::FloatPredicate::OLE, l, r, "fle")
                    .unwrap();
                builder
                    .build_int_z_extend(cmp, context.i8_type(), "zext")
                    .unwrap()
                    .into()
            }
            BinOp::Gt => {
                let cmp = builder
                    .build_float_compare(inkwell::FloatPredicate::OGT, l, r, "fgt")
                    .unwrap();
                builder
                    .build_int_z_extend(cmp, context.i8_type(), "zext")
                    .unwrap()
                    .into()
            }
            BinOp::GtEq => {
                let cmp = builder
                    .build_float_compare(inkwell::FloatPredicate::OGE, l, r, "fge")
                    .unwrap();
                builder
                    .build_int_z_extend(cmp, context.i8_type(), "zext")
                    .unwrap()
                    .into()
            }
            _ => lhs, // fallback for unsupported float ops (bitwise, etc.)
        });
    }

    // Integer operations
    let l = lhs.into_int_value();
    let r = rhs.into_int_value();
    Ok(match op {
        BinOp::Add => builder.build_int_add(l, r, "add").unwrap().into(),
        BinOp::Sub => builder.build_int_sub(l, r, "sub").unwrap().into(),
        BinOp::Mul => builder.build_int_mul(l, r, "mul").unwrap().into(),
        BinOp::Div => builder.build_int_signed_div(l, r, "sdiv").unwrap().into(),
        BinOp::Mod => builder.build_int_signed_rem(l, r, "srem").unwrap().into(),
        BinOp::BitAnd | BinOp::And => builder.build_and(l, r, "and").unwrap().into(),
        BinOp::BitOr | BinOp::Or => builder.build_or(l, r, "or").unwrap().into(),
        BinOp::BitXor => builder.build_xor(l, r, "xor").unwrap().into(),
        BinOp::Shl => builder.build_left_shift(l, r, "shl").unwrap().into(),
        BinOp::Shr => builder
            .build_right_shift(l, r, true, "ashr")
            .unwrap()
            .into(),
        BinOp::Eq => {
            let cmp = builder
                .build_int_compare(IntPredicate::EQ, l, r, "eq")
                .unwrap();
            builder
                .build_int_z_extend(cmp, context.i8_type(), "zext")
                .unwrap()
                .into()
        }
        BinOp::NotEq => {
            let cmp = builder
                .build_int_compare(IntPredicate::NE, l, r, "ne")
                .unwrap();
            builder
                .build_int_z_extend(cmp, context.i8_type(), "zext")
                .unwrap()
                .into()
        }
        BinOp::Lt => {
            let cmp = builder
                .build_int_compare(IntPredicate::SLT, l, r, "slt")
                .unwrap();
            builder
                .build_int_z_extend(cmp, context.i8_type(), "zext")
                .unwrap()
                .into()
        }
        BinOp::LtEq => {
            let cmp = builder
                .build_int_compare(IntPredicate::SLE, l, r, "sle")
                .unwrap();
            builder
                .build_int_z_extend(cmp, context.i8_type(), "zext")
                .unwrap()
                .into()
        }
        BinOp::Gt => {
            let cmp = builder
                .build_int_compare(IntPredicate::SGT, l, r, "sgt")
                .unwrap();
            builder
                .build_int_z_extend(cmp, context.i8_type(), "zext")
                .unwrap()
                .into()
        }
        BinOp::GtEq => {
            let cmp = builder
                .build_int_compare(IntPredicate::SGE, l, r, "sge")
                .unwrap();
            builder
                .build_int_z_extend(cmp, context.i8_type(), "zext")
                .unwrap()
                .into()
        }
        // Mirrors `cranelift/emit.rs::emit_binop`: the `=~` match operator
        // is desugared to a method call at MIR-lowering time and never
        // reaches binop emission.
        BinOp::MatchOp => {
            unreachable!("BinOp::MatchOp should have been desugared to a method call at MIR-lower")
        }
    })
}

/// Map CmpOp to LLVM IntPredicate.
pub(super) fn cmpop_to_intpred(op: CmpOp) -> IntPredicate {
    match op {
        CmpOp::Eq => IntPredicate::EQ,
        CmpOp::NotEq => IntPredicate::NE,
        CmpOp::Lt => IntPredicate::SLT,
        CmpOp::LtEq => IntPredicate::SLE,
        CmpOp::Gt => IntPredicate::SGT,
        CmpOp::GtEq => IntPredicate::SGE,
    }
}
