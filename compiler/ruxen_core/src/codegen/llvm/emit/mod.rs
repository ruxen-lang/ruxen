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

mod calls;
mod control_flow;
mod instructions;

use control_flow::translate_terminator;
use instructions::translate_instruction;

/// Compile all functions in a MIR program into LLVM IR.
///
/// `is_wasm` is `true` when the target machine is `wasm32-*`/`wasm64-*`; in
/// that case every `program.wasm_exports` function gets the LLVM `export_name`
/// string attribute so `wasm-ld` emits it as a host-callable wasm export under
/// its source name, decoupled from any Ruxen name mangling (tier 4.03; ADR
/// `docs/decisions/phase4-no-std-wasm.md`, spec §9 Q6). On non-wasm targets the
/// flag is `false` and `wasm_exports` is ignored.
pub fn compile_program<'ctx>(
    program: &MirProgram,
    module: &Module<'ctx>,
    context: &'ctx Context,
    is_wasm: bool,
) -> Result<(), String> {
    // Declare the DERIVED FFI imports FIRST so every `.rx`-declared
    // `ruxen_*` symbol is created from its lib-decl-derived signature
    // (`ty_to_llvm` per declared `Ty`). The residual
    // `declare_runtime_functions` runs AFTER and its per-name `is_none`
    // guard skips anything already declared here — so derived wins for
    // declared symbols, and the residual table answers only for the
    // compiler-internal symbols no `.rx` block declares. Post
    // zero_rust_stdlib_classes.spec.md ABI-derivation migration.
    for lib in &program.ffi_libs {
        for ffi_fn in &lib.functions {
            declare_ffi_function(module, context, ffi_fn, &lib.name, is_wasm);
        }
    }

    // Declare compiler-internal residual runtime functions.
    runtime_decl::declare_runtime_functions(module, context);

    // Pass 1: declare all user functions
    for func in &program.functions {
        let fn_type = super::types::build_function_type(func, context);
        // `main` + every `<Mixin>_dynamic_<method>` helper exports
        // External linkage so C-side runtime code (e.g.
        // library/std/future/runtime/scheduler.c, sub-phase 5) can
        // dispatch through the helper by symbol. Mirrors the cranelift
        // backend's logic — see codegen/cranelift/mod.rs::
        // is_dynamic_dispatch_helper for the naming pattern.
        // Tier 4.03 (WASM): a wasm-exported function needs External linkage
        // (Internal would be invisible to `wasm-ld`) plus the `export_name`
        // attribute set below.
        let is_wasm_export = is_wasm && program.wasm_exports.iter().any(|n| n == &func.name);
        let linkage = if func.name == "main"
            || super::super::cranelift::is_dynamic_dispatch_helper(&func.name)
            || is_wasm_export
        {
            Some(inkwell::module::Linkage::External)
        } else {
            Some(inkwell::module::Linkage::Internal)
        };
        let llvm_fn = module.add_function(&func.name, fn_type, linkage);
        if is_wasm_export {
            // `export_name` is the LLVM/clang mechanism that emits a wasm
            // export under an exact ASCII name, independent of the LLVM symbol
            // name — so the export survives `--gc-sections` and carries no
            // mangling. Equivalent to C's
            // `__attribute__((export_name("<name>")))`.
            let attr = context.create_string_attribute("wasm-export-name", &func.name);
            llvm_fn.add_attribute(inkwell::attributes::AttributeLoc::Function, attr);
        }
    }

    // Pass 2: define all user functions
    let mut string_cache: HashMap<String, GlobalValue<'ctx>> = HashMap::new();

    for func in &program.functions {
        compile_function(func, program, module, context, &mut string_cache)?;
    }

    Ok(())
}

/// Declare an FFI function in the LLVM module.
///
/// Tier 4.09 (wasm host imports): on the wasm target a top-level `lib "<module>"`
/// block declares functions the HOST supplies — there is no native linking. Each
/// gets `wasm-import-module = "<module>"` + `wasm-import-name = "<symbol>"` so
/// `wasm-ld` emits a precise, intentional import (`<module>.<symbol>`) instead of
/// relying on `--allow-undefined`'s implicit `env` default. The stdlib's
/// `ruxen_*` runtime bindings are class-body FFI (declared in Pass 1), NOT
/// top-level libs, so they are unaffected and still resolve to the linked
/// runtime objects.
fn declare_ffi_function<'ctx>(
    module: &Module<'ctx>,
    context: &'ctx Context,
    ffi_fn: &FfiFuncDecl,
    lib_name: &str,
    is_wasm: bool,
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
    let _ = llvm_fn;

    // Host imports on wasm: we deliberately do NOT pin `wasm-import-module`/
    // `-name` attributes. An explicit import-module forces the symbol to be an
    // import even when a definition exists — which conflicts with FFI bindings
    // whose C runtime IS linked on wasm (e.g. the stdlib `ruxen_string_*` /
    // `ruxen_vec_*` in WASM_RUNTIME_CORE: import-vs-defined mismatch). Instead we
    // let `wasm-ld --allow-undefined` resolve defined symbols and route the rest
    // to the default `env` import module — so genuinely host-supplied functions
    // (e.g. `ruxen_canvas_*`, and stubs for sync/io/time) arrive as `env.<symbol>`
    // for the JS host to provide, while linked runtime symbols just resolve.
    // (Custom import modules would need attrs gated on "not in the linked set" —
    // tier 4.09 follow-up.) `is_wasm` is kept for that future gating.
    let _ = is_wasm;

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
            .get_function("ruxen_env_init")
            .ok_or_else(|| "missing runtime declaration for ruxen_env_init".to_string())?;
        let argc = llvm_fn
            .get_nth_param(0)
            .ok_or_else(|| "missing argc param for main".to_string())?;
        let argv = llvm_fn
            .get_nth_param(1)
            .ok_or_else(|| "missing argv param for main".to_string())?;
        builder
            .build_call(env_init, &[argc.into(), argv.into()], "")
            .map_err(|e| format!("Failed to build ruxen_env_init call: {:?}", e))?;
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

/// Whether a MIR operand is a signed numeric value (for `sitofp`/`fptosi`
/// selection on numeric `as` casts). Mirrors
/// `cranelift::emit::mir_arg_is_signed`.
pub(super) fn mir_arg_is_signed(arg: &MirValue, func: &MirFunction) -> bool {
    let ty = match arg {
        MirValue::Literal(Literal::Int(_)) | MirValue::Literal(Literal::Char(_)) => return true,
        MirValue::Literal(Literal::Bool(_))
        | MirValue::Literal(Literal::Float(_))
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

/// Convert a MirValue to an LLVM BasicValueEnum.
pub(super) fn gen_value<'ctx>(
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
///
/// Signedness-blind convenience wrapper. For numeric int↔float casts the
/// signedness matters (`-5 as Float` must be `-5.0`, not `1.8e19`), so the
/// `Assign` codegen — the one place a Ruxen `as` numeric cast is lowered —
/// calls [`coerce_value_signed`] with the correct signedness instead. Mirrors
/// the Cranelift backend's `coerce_value` / `coerce_value_signed` split
/// (`codegen/cranelift/emit.rs`).
pub(super) fn coerce_value<'ctx>(
    val: BasicValueEnum<'ctx>,
    target_ty: BasicTypeEnum<'ctx>,
    builder: &Builder<'ctx>,
) -> BasicValueEnum<'ctx> {
    coerce_value_signed(val, target_ty, false, builder)
}

/// Signedness-aware coercion (mirrors `cranelift::emit::coerce_value_signed`).
///
/// `signed` only affects numeric int↔float conversions: for int→float it
/// selects the SOURCE interpretation (`sitofp` vs `uitofp`); for float→int it
/// selects the TARGET interpretation (`fptosi` vs `fptoui`). Int↔int and
/// float↔float widening/narrowing ignore it.
pub(super) fn coerce_value_signed<'ctx>(
    val: BasicValueEnum<'ctx>,
    target_ty: BasicTypeEnum<'ctx>,
    signed: bool,
    builder: &Builder<'ctx>,
) -> BasicValueEnum<'ctx> {
    if val.get_type() == target_ty {
        return val;
    }

    // Int -> Float (numeric cast, e.g. `n as Float`). Without this an integer
    // value stored into a float-typed slot was left as an int bit-pattern in a
    // narrower (f32) alloca — a type-punned, out-of-bounds store the LLVM
    // optimizer folds to `undef` (a 9-arg FFI's 4th f64 arg vanished on wasm).
    if let (BasicValueEnum::IntValue(int_val), BasicTypeEnum::FloatType(target_float)) =
        (val, target_ty)
    {
        return if signed {
            builder
                .build_signed_int_to_float(int_val, target_float, "sitofp")
                .unwrap()
                .into()
        } else {
            builder
                .build_unsigned_int_to_float(int_val, target_float, "uitofp")
                .unwrap()
                .into()
        };
    }

    // Float -> Int (numeric cast, e.g. `f as Int`).
    if let (BasicValueEnum::FloatValue(float_val), BasicTypeEnum::IntType(target_int)) =
        (val, target_ty)
    {
        return if signed {
            builder
                .build_float_to_signed_int(float_val, target_int, "fptosi")
                .unwrap()
                .into()
        } else {
            builder
                .build_float_to_unsigned_int(float_val, target_int, "fptoui")
                .unwrap()
                .into()
        };
    }

    // Integer <-> Integer: truncate, or extend honoring `signed` (sext for a
    // signed source so `-1i32 as Int` is `-1`, not `0x0000_0000_FFFF_FFFF`).
    // The default wrapper passes `signed = false` → zext, preserving the prior
    // behaviour at every non-cast call site. Mirrors Cranelift's
    // `coerce_value_signed` int↔int branch.
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
        } else if signed {
            builder
                .build_int_s_extend(int_val, target_int, "sext")
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
//  Shared helper functions
// ═══════════════════════════════════════════════════════════════════════

/// Check if a MIR value operand is a string-typed local.
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
/// Mirrors `codegen::cranelift::helpers::is_string_mir_ty` — see that
/// definition for the full rationale on why `Ty::Class { name: "String", .. }`
/// must be treated equivalently to `Ty::String`. Both backends must stay
/// in sync; without this the LLVM `Compare` emitter falls back to
/// pointer-eq on `def f(s: String, t: String) -> Bool s == t end` style
/// code.
fn is_string_mir_ty(ty: &Ty) -> bool {
    match ty {
        Ty::String => true,
        Ty::Class { name, .. } if name == "String" => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_string_mir_ty(inner),
        _ => false,
    }
}

/// Size estimate for heap allocation (mirrors Cranelift backend).
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
        Ty::Enum { .. } => 32,
        // See cranelift/helpers.rs::simple_type_size for rationale —
        // Class/Struct MUST have their alloc size precomputed at MIR
        // lowering time, not estimated here.
        Ty::Class { name, .. } | Ty::Struct { name, .. } => panic!(
            "llvm simple_type_size: class/struct `{}` reached the fallback estimator. \
             MIR Alloc.size must be precomputed by Lowerer::alloc_size; the historical \
             64-byte fallback silently truncated classes with >8 fields.",
            name
        ),
        Ty::Option(_) => 16,
        Ty::Result(_, _) => 16,
        Ty::Tuple(elems) => elems.len().max(1) * 8,
        // T2.02 stage 4: `n` is a `ConstExpr`; pre-stage-7 codegen only
        // resolves `Lit` values. Mirrors
        // `cranelift/helpers.rs::simple_type_size`.
        Ty::FixedArray(_, n) => n.as_lit().unwrap_or(0) as usize * 8,
        _ => 8,
    }
}
