//! MIR instruction → LLVM IR translation.
//!
//! Translates each MIR instruction and terminator into LLVM IR using
//! the inkwell builder. Uses alloca-based locals (LLVM's mem2reg pass
//! promotes them to SSA automatically).

use std::collections::HashMap;

use inkwell::basic_block::BasicBlock;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::{BasicMetadataTypeEnum, BasicType, BasicTypeEnum};
use inkwell::values::{
    BasicMetadataValueEnum, BasicValueEnum, FunctionValue, GlobalValue, IntValue, PointerValue,
};
use inkwell::AddressSpace;
use inkwell::IntPredicate;

use crate::hir::types::Ty;
use crate::mir::nodes::*;
use crate::parser::ast::BinOp;

use super::runtime_decl;
use super::types::ty_to_llvm;
use crate::codegen::runtime::{extract_method_name, runtime_name};

/// Compile all functions in a MIR program into LLVM IR.
pub fn compile_program<'ctx>(
    program: &MirProgram,
    module: &Module<'ctx>,
    context: &'ctx Context,
) -> Result<(), String> {
    // Declare runtime functions
    runtime_decl::declare_runtime_functions(module, context);

    // Declare FFI functions
    for lib in &program.ffi_libs {
        for ffi_fn in &lib.functions {
            declare_ffi_function(module, context, ffi_fn, &lib.name);
        }
    }

    // Pass 1: declare all user functions
    for func in &program.functions {
        let fn_type = super::types::build_function_type(func, context);
        let linkage = if func.name == "main" {
            Some(inkwell::module::Linkage::External)
        } else {
            Some(inkwell::module::Linkage::Internal)
        };
        module.add_function(&func.name, fn_type, linkage);
    }

    // Pass 2: define all user functions
    let mut string_cache: HashMap<String, GlobalValue<'ctx>> = HashMap::new();

    for func in &program.functions {
        compile_function(func, program, module, context, &mut string_cache)?;
    }

    Ok(())
}

/// Declare an FFI function in the LLVM module.
fn declare_ffi_function<'ctx>(
    module: &Module<'ctx>,
    context: &'ctx Context,
    ffi_fn: &FfiFuncDecl,
    lib_name: &str,
) {
    if module.get_function(&ffi_fn.name).is_some() {
        return;
    }

    let param_types: Vec<BasicMetadataTypeEnum> = ffi_fn
        .param_types
        .iter()
        .filter_map(|ty| ty_to_llvm(ty, context).map(|t| t.into()))
        .collect();

    let fn_type = match &ffi_fn.return_type {
        Some(ret_ty) => match ty_to_llvm(ret_ty, context) {
            Some(ret) => ret.fn_type(&param_types, ffi_fn.is_variadic),
            None => context
                .void_type()
                .fn_type(&param_types, ffi_fn.is_variadic),
        },
        None => context
            .void_type()
            .fn_type(&param_types, ffi_fn.is_variadic),
    };

    let llvm_fn = module.add_function(
        &ffi_fn.name,
        fn_type,
        Some(inkwell::module::Linkage::External),
    );

    // Also register with lib-qualified name
    if !lib_name.is_empty() {
        let qualified = format!("{}_{}", lib_name, ffi_fn.name);
        if module.get_function(&qualified).is_none() {
            // Create an alias by adding a second function that calls through
            // For simplicity, just add the function under both names
            module.add_function(
                &qualified,
                fn_type,
                Some(inkwell::module::Linkage::External),
            );
        }
    }

    let _ = llvm_fn; // suppress unused warning
}

/// Compile a single MIR function into LLVM IR.
fn compile_function<'ctx>(
    func: &MirFunction,
    program: &MirProgram,
    module: &Module<'ctx>,
    context: &'ctx Context,
    string_cache: &mut HashMap<String, GlobalValue<'ctx>>,
) -> Result<(), String> {
    let llvm_fn = module
        .get_function(&func.name)
        .ok_or_else(|| format!("Function '{}' was not declared", func.name))?;

    let builder = context.create_builder();

    // Create entry block for allocas
    let entry_bb = context.append_basic_block(llvm_fn, "entry");
    builder.position_at_end(entry_bb);

    // Create allocas for all local variables
    let mut local_allocas: HashMap<LocalId, PointerValue<'ctx>> = HashMap::new();
    for local in &func.locals {
        let llvm_ty = ty_to_llvm(&local.ty, context).unwrap_or(context.i64_type().into());
        let alloca = builder
            .build_alloca(llvm_ty, &local.name)
            .map_err(|e| format!("Failed to build alloca for '{}': {:?}", local.name, e))?;
        local_allocas.insert(local.id, alloca);
    }

    // Store function parameters into their allocas
    if func.name != "main" {
        let mut param_idx = 0u32;
        for &param_id in &func.params {
            let param_ty = ty_to_llvm(&func.locals[param_id as usize].ty, context);
            if param_ty.is_some() {
                let param_val = llvm_fn.get_nth_param(param_idx).ok_or_else(|| {
                    format!("Missing param {} for function '{}'", param_idx, func.name)
                })?;
                builder
                    .build_store(local_allocas[&param_id], param_val)
                    .map_err(|e| format!("Failed to store param: {:?}", e))?;
                param_idx += 1;
            }
        }
    } else {
        let env_init = module
            .get_function("riven_env_init")
            .ok_or_else(|| "missing runtime declaration for riven_env_init".to_string())?;
        let argc = llvm_fn
            .get_nth_param(0)
            .ok_or_else(|| "missing argc param for main".to_string())?;
        let argv = llvm_fn
            .get_nth_param(1)
            .ok_or_else(|| "missing argv param for main".to_string())?;
        builder
            .build_call(env_init, &[argc.into(), argv.into()], "")
            .map_err(|e| format!("Failed to build riven_env_init call: {:?}", e))?;
    }

    // Create LLVM basic blocks for each MIR block
    let mut block_map: Vec<BasicBlock<'ctx>> = Vec::with_capacity(func.blocks.len());
    for mir_block in &func.blocks {
        let bb = context.append_basic_block(llvm_fn, &format!("bb{}", mir_block.id));
        block_map.push(bb);
    }

    // Branch from entry to the first MIR block
    builder
        .build_unconditional_branch(block_map[func.entry_block])
        .map_err(|e| format!("Failed to branch to entry block: {:?}", e))?;

    // Translate each MIR block
    for (mir_idx, mir_block) in func.blocks.iter().enumerate() {
        builder.position_at_end(block_map[mir_idx]);

        for inst in &mir_block.instructions {
            translate_instruction(
                inst,
                func,
                program,
                &local_allocas,
                &block_map,
                &builder,
                module,
                context,
                string_cache,
            )?;
        }

        translate_terminator(
            &mir_block.terminator,
            func,
            &local_allocas,
            &block_map,
            &builder,
            module,
            context,
        )?;
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════
//  Value generation
// ═══════════════════════════════════════════════════════════════════════

/// Convert a MirValue to an LLVM BasicValueEnum.
fn gen_value<'ctx>(
    mir_val: &MirValue,
    func: &MirFunction,
    local_allocas: &HashMap<LocalId, PointerValue<'ctx>>,
    builder: &Builder<'ctx>,
    context: &'ctx Context,
) -> Result<BasicValueEnum<'ctx>, String> {
    match mir_val {
        MirValue::Literal(lit) => match lit {
            Literal::Int(n) => Ok(context.i64_type().const_int(*n as u64, true).into()),
            Literal::Float(f) => Ok(context.f64_type().const_float(*f).into()),
            Literal::Bool(b) => Ok(context.i8_type().const_int(*b as u64, false).into()),
            Literal::Char(c) => Ok(context.i32_type().const_int(*c as u64, false).into()),
            Literal::String(_) => Ok(context
                .ptr_type(AddressSpace::default())
                .const_null()
                .into()),
        },
        MirValue::Use(local_id) => {
            let alloca = local_allocas
                .get(local_id)
                .ok_or_else(|| format!("Unknown local {} in function '{}'", local_id, func.name))?;
            let local_ty = ty_to_llvm(&func.locals[*local_id as usize].ty, context)
                .unwrap_or(context.i64_type().into());
            let val = builder
                .build_load(local_ty, *alloca, "load")
                .map_err(|e| format!("Failed to build load: {:?}", e))?;
            Ok(val)
        }
        MirValue::Unit => Ok(context.i64_type().const_int(0, false).into()),
    }
}

// ═══════════════════════════════════════════════════════════════════════
//  Type coercion
// ═══════════════════════════════════════════════════════════════════════

/// Coerce a value to a target type if they differ.
fn coerce_value<'ctx>(
    val: BasicValueEnum<'ctx>,
    target_ty: BasicTypeEnum<'ctx>,
    builder: &Builder<'ctx>,
) -> BasicValueEnum<'ctx> {
    if val.get_type() == target_ty {
        return val;
    }

    // Integer <-> Integer: truncate or zero-extend
    if let (BasicValueEnum::IntValue(int_val), BasicTypeEnum::IntType(target_int)) =
        (val, target_ty)
    {
        let src_bits = int_val.get_type().get_bit_width();
        let dst_bits = target_int.get_bit_width();
        return if src_bits > dst_bits {
            builder
                .build_int_truncate(int_val, target_int, "trunc")
                .unwrap()
                .into()
        } else {
            builder
                .build_int_z_extend(int_val, target_int, "zext")
                .unwrap()
                .into()
        };
    }

    // Float <-> Float: truncate or extend
    if let (BasicValueEnum::FloatValue(float_val), BasicTypeEnum::FloatType(target_float)) =
        (val, target_ty)
    {
        let src_bits = match float_val.get_type() {
            t if t
                == builder
                    .get_insert_block()
                    .unwrap()
                    .get_parent()
                    .unwrap()
                    .get_type()
                    .get_context()
                    .f32_type() =>
            {
                32
            }
            _ => 64,
        };
        let dst_bits = match target_float {
            t if t
                == builder
                    .get_insert_block()
                    .unwrap()
                    .get_parent()
                    .unwrap()
                    .get_type()
                    .get_context()
                    .f32_type() =>
            {
                32
            }
            _ => 64,
        };
        return if src_bits > dst_bits {
            builder
                .build_float_trunc(float_val, target_float, "ftrunc")
                .unwrap()
                .into()
        } else {
            builder
                .build_float_ext(float_val, target_float, "fext")
                .unwrap()
                .into()
        };
    }

    // Int -> Pointer
    if let (BasicValueEnum::IntValue(int_val), BasicTypeEnum::PointerType(ptr_ty)) =
        (val, target_ty)
    {
        return builder
            .build_int_to_ptr(int_val, ptr_ty, "inttoptr")
            .unwrap()
            .into();
    }

    // Pointer -> Int
    if let (BasicValueEnum::PointerValue(ptr_val), BasicTypeEnum::IntType(int_ty)) =
        (val, target_ty)
    {
        return builder
            .build_ptr_to_int(ptr_val, int_ty, "ptrtoint")
            .unwrap()
            .into();
    }

    // Pointer -> Pointer (opaque pointers, just return as-is)
    if let (BasicValueEnum::PointerValue(_), BasicTypeEnum::PointerType(_)) = (val, target_ty) {
        return val;
    }

    val // fallback: return unchanged
}

// ═══════════════════════════════════════════════════════════════════════
//  Instruction translation
// ═══════════════════════════════════════════════════════════════════════

#[allow(clippy::too_many_arguments)]
fn translate_instruction<'ctx>(
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
                let eq_fn = get_or_declare_runtime(module, context, "riven_string_eq");
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
                let cmp_fn = get_or_declare_runtime(module, context, "riven_string_cmp");
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
                "riven_noop_passthrough" => {
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
                "riven_noop_return_null" => {
                    if let Some(dest_id) = dest {
                        let dest_ty = ty_to_llvm(&func.locals[*dest_id as usize].ty, context)
                            .unwrap_or(context.i64_type().into());
                        let zero = dest_ty.const_zero();
                        builder
                            .build_store(local_allocas[dest_id], zero)
                            .map_err(|e| format!("Failed to store noop null: {:?}", e))?;
                    }
                }
                "riven_noop" => {
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
            let alloc_fn = get_or_declare_runtime(module, context, "riven_alloc");
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
            // MirInst::Drop is a marker — the actual `riven_dealloc` call
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

// ═══════════════════════════════════════════════════════════════════════
//  Terminator translation
// ═══════════════════════════════════════════════════════════════════════

fn translate_terminator<'ctx>(
    term: &Terminator,
    func: &MirFunction,
    local_allocas: &HashMap<LocalId, PointerValue<'ctx>>,
    block_map: &[BasicBlock<'ctx>],
    builder: &Builder<'ctx>,
    _module: &Module<'ctx>,
    context: &'ctx Context,
) -> Result<(), String> {
    match term {
        Terminator::Return(val) => {
            if func.name == "main" {
                let zero = context.i32_type().const_int(0, false);
                builder
                    .build_return(Some(&zero))
                    .map_err(|e| format!("Failed to build return: {:?}", e))?;
            } else {
                match val {
                    Some(v) => {
                        let ret_val = gen_value(v, func, local_allocas, builder, context)?;
                        if let Some(ret_ty) = ty_to_llvm(&func.return_ty, context) {
                            let ret_val = coerce_value(ret_val, ret_ty, builder);
                            builder
                                .build_return(Some(&ret_val))
                                .map_err(|e| format!("Failed to build return: {:?}", e))?;
                        } else {
                            builder
                                .build_return(Some(&ret_val))
                                .map_err(|e| format!("Failed to build return: {:?}", e))?;
                        }
                    }
                    None => {
                        if ty_to_llvm(&func.return_ty, context).is_some() {
                            // Non-void return type but no value — return zero
                            let ret_ty = ty_to_llvm(&func.return_ty, context).unwrap();
                            let zero = ret_ty.const_zero();
                            builder
                                .build_return(Some(&zero))
                                .map_err(|e| format!("Failed to build return: {:?}", e))?;
                        } else {
                            builder
                                .build_return(None)
                                .map_err(|e| format!("Failed to build void return: {:?}", e))?;
                        }
                    }
                }
            }
        }

        Terminator::Goto(target) => {
            builder
                .build_unconditional_branch(block_map[*target])
                .map_err(|e| format!("Failed to build goto: {:?}", e))?;
        }

        Terminator::Branch {
            cond,
            then_block,
            else_block,
        } => {
            let cond_val = gen_value(cond, func, local_allocas, builder, context)?;
            // Convert to i1 for LLVM's br instruction
            let cond_i1 = if cond_val.is_pointer_value() {
                // Pointer: compare != null
                let ptr_val = cond_val.into_pointer_value();
                let null = context.ptr_type(AddressSpace::default()).const_null();
                builder
                    .build_int_compare(
                        IntPredicate::NE,
                        builder
                            .build_ptr_to_int(ptr_val, context.i64_type(), "ptrtoint")
                            .unwrap(),
                        builder
                            .build_ptr_to_int(null, context.i64_type(), "nullint")
                            .unwrap(),
                        "tobool",
                    )
                    .map_err(|e| format!("Failed to build ptr compare: {:?}", e))?
            } else {
                // Integer (i8 bool): compare != 0
                let int_val = cond_val.into_int_value();
                builder
                    .build_int_compare(
                        IntPredicate::NE,
                        int_val,
                        int_val.get_type().const_zero(),
                        "tobool",
                    )
                    .map_err(|e| format!("Failed to build bool compare: {:?}", e))?
            };
            builder
                .build_conditional_branch(cond_i1, block_map[*then_block], block_map[*else_block])
                .map_err(|e| format!("Failed to build branch: {:?}", e))?;
        }

        Terminator::Switch {
            value,
            targets,
            otherwise,
        } => {
            let val = gen_value(value, func, local_allocas, builder, context)?;
            let int_val = val.into_int_value();

            let cases: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> = targets
                .iter()
                .map(|(disc, bid)| {
                    (
                        int_val.get_type().const_int(*disc as u64, true),
                        block_map[*bid],
                    )
                })
                .collect();

            let case_refs: Vec<(IntValue<'ctx>, BasicBlock<'ctx>)> = cases;
            builder
                .build_switch(int_val, block_map[*otherwise], &case_refs)
                .map_err(|e| format!("Failed to build switch: {:?}", e))?;
        }

        Terminator::Unreachable => {
            builder
                .build_unreachable()
                .map_err(|e| format!("Failed to build unreachable: {:?}", e))?;
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════
//  Binary operation emission
// ═══════════════════════════════════════════════════════════════════════

fn emit_binop<'ctx>(
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
    })
}

// ═══════════════════════════════════════════════════════════════════════
//  Helper functions
// ═══════════════════════════════════════════════════════════════════════

/// Map CmpOp to LLVM IntPredicate.
fn cmpop_to_intpred(op: CmpOp) -> IntPredicate {
    match op {
        CmpOp::Eq => IntPredicate::EQ,
        CmpOp::NotEq => IntPredicate::NE,
        CmpOp::Lt => IntPredicate::SLT,
        CmpOp::LtEq => IntPredicate::SLE,
        CmpOp::Gt => IntPredicate::SGT,
        CmpOp::GtEq => IntPredicate::SGE,
    }
}

/// Check if a MIR value operand is a string-typed local.
fn is_string_typed_value(val: &MirValue, func: &MirFunction) -> bool {
    if let MirValue::Use(local_id) = val {
        if let Some(local) = func.locals.get(*local_id as usize) {
            return is_string_mir_ty(&local.ty);
        }
    }
    false
}

/// Check if a MIR type is a string-like type.
fn is_string_mir_ty(ty: &Ty) -> bool {
    match ty {
        Ty::String | Ty::Str => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_string_mir_ty(inner),
        _ => false,
    }
}

/// Get or declare a runtime function by name.
fn get_or_declare_runtime<'ctx>(
    module: &Module<'ctx>,
    context: &'ctx Context,
    name: &str,
) -> FunctionValue<'ctx> {
    if let Some(f) = module.get_function(name) {
        return f;
    }
    // If not found, declare runtime functions and try again
    runtime_decl::declare_runtime_functions(module, context);
    module.get_function(name).unwrap_or_else(|| {
        // Fallback: declare as ptr -> i64
        let ptr_ty = context.ptr_type(AddressSpace::default());
        let fn_ty = context.i64_type().fn_type(&[ptr_ty.into()], false);
        module.add_function(name, fn_ty, Some(inkwell::module::Linkage::External))
    })
}

/// Get or declare a function (runtime, user-defined, or FFI) by name.
#[allow(clippy::too_many_arguments)]
fn get_or_declare_func<'ctx>(
    name: &str,
    arg_vals: &[BasicMetadataValueEnum<'ctx>],
    has_return: bool,
    _func: &MirFunction,
    _program: &MirProgram,
    module: &Module<'ctx>,
    context: &'ctx Context,
) -> Result<FunctionValue<'ctx>, String> {
    // Check if already declared
    if let Some(f) = module.get_function(name) {
        return Ok(f);
    }

    // For inferred-type method calls (?T..._method), search declared functions
    if name.starts_with('?') {
        let method = extract_method_name(name);
        let suffix = format!("_{}", method);
        if let Some(resolved) = find_function_by_suffix(module, &suffix) {
            return Ok(resolved);
        }
    }

    // For generic type parameter methods (T_assign, E_message)
    if let Some(pos) = name.find('_') {
        let prefix = &name[..pos];
        if prefix.len() <= 2 && !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_uppercase())
        {
            let method = &name[pos..]; // includes the _
            if let Some(resolved) = find_function_by_suffix(module, method) {
                return Ok(resolved);
            }
        }
    }

    // Try runtime functions
    if let Some(f) = module.get_function(name) {
        return Ok(f);
    }

    // Declare runtime function if it's a known one
    runtime_decl::declare_runtime_functions(module, context);
    if let Some(f) = module.get_function(name) {
        return Ok(f);
    }

    // Fallback: infer signature from call-site
    let param_types: Vec<BasicMetadataTypeEnum> = arg_vals
        .iter()
        .map(|a| {
            let bv: BasicValueEnum = (*a)
                .try_into()
                .unwrap_or(context.i64_type().const_int(0, false).into());
            bv.get_type().into()
        })
        .collect();

    let fn_type = if has_return {
        context.i64_type().fn_type(&param_types, false)
    } else {
        context.void_type().fn_type(&param_types, false)
    };

    Ok(module.add_function(name, fn_type, Some(inkwell::module::Linkage::External)))
}

/// Find a function whose name ends with the given suffix.
/// Prefers the shortest match (most specific).
fn find_function_by_suffix<'ctx>(
    module: &Module<'ctx>,
    suffix: &str,
) -> Option<FunctionValue<'ctx>> {
    let mut best: Option<FunctionValue<'ctx>> = None;
    let mut best_len = usize::MAX;

    let mut func = module.get_first_function();
    while let Some(f) = func {
        let fname = f.get_name().to_str().unwrap_or("");
        if fname.ends_with(suffix) && !fname.starts_with('?') && fname.len() < best_len {
            best = Some(f);
            best_len = fname.len();
        }
        func = f.get_next_function();
    }

    best
}

/// Size estimate for heap allocation (mirrors Cranelift backend).
fn simple_type_size(ty: &Ty) -> usize {
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
        Ty::Vec(_) => 24,
        Ty::HashMap(_, _) | Ty::Set(_) => 48,
        Ty::Ref(_)
        | Ty::RefMut(_)
        | Ty::RefLifetime(_, _)
        | Ty::RefMutLifetime(_, _)
        | Ty::RawPtr(_)
        | Ty::RawPtrMut(_)
        | Ty::RawPtrVoid
        | Ty::RawPtrMutVoid => 8,
        Ty::Unit | Ty::Never => 0,
        Ty::Enum { .. } => 32,
        Ty::Class { .. } | Ty::Struct { .. } => 64,
        Ty::Option(_) => 16,
        Ty::Result(_, _) => 16,
        Ty::Tuple(elems) => elems.len().max(1) * 8,
        Ty::Array(_, n) => n * 8,
        _ => 8,
    }
}
