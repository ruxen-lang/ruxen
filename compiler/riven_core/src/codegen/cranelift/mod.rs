//! Cranelift code generation backend for the Riven compiler.
//!
//! Translates MIR programs into native object code via Cranelift's
//! `ObjectModule`. The pipeline is:
//!   1. Declare all functions (two-pass: declare then define).
//!   2. For each function, translate MIR blocks to Cranelift IR.
//!   3. Emit the finished object bytes.
//!
//! This module is split for navigability:
//!   * `runtime_sigs` — the big `runtime_signature` lookup table.
//!   * `helpers`      — pure `Ty`/`CmpOp` ↔ Cranelift mappings.
//!   * `translation_env` — module-state borrow split from `FunctionBuilder`.
//!   * `emit`         — free functions for MIR instruction / terminator lowering.

mod emit;
mod helpers;
mod runtime_sigs;
mod translation_env;

use std::collections::{HashMap, HashSet};

use cranelift_codegen::ir::types::{self, Type};
use cranelift_codegen::ir::{
    AbiParam, InstBuilder, Signature, StackSlot, StackSlotData, StackSlotKind,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::mir::nodes::*;

use self::emit::{build_signature, def_local, translate_instruction, translate_terminator};
use self::helpers::{is_string_mir_ty, ty_to_cranelift};
use self::translation_env::TranslationEnv;

/// Cranelift code generation engine.
///
/// Holds the Cranelift module, context objects, and bookkeeping for
/// string data sections and declared functions.
pub struct CodeGen {
    module: ObjectModule,
    ctx: Context,
    builder_ctx: FunctionBuilderContext,
    string_data: HashMap<String, cranelift_module::DataId>,
    string_counter: u32,
    declared_fns: HashMap<String, FuncId>,
    /// Cranelift parameter types for every function declared in this
    /// module (FFI + user MIR fns).  Used by `coerce_call_args` to apply
    /// the *correct* narrow-int signature when the callee is a known
    /// user-defined function — runtime helpers still flow through
    /// `runtime_signature`.  Without this, an `i8` argument to e.g.
    /// `Bool_fmt` would be unconditionally widened to `i64` by the
    /// fallback widening rule and fail Cranelift IR verification.
    user_fn_param_tys: HashMap<String, Vec<Type>>,
}

impl CodeGen {
    /// Create a new code generator targeting the host machine.
    pub fn new() -> Result<Self, String> {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("opt_level", "none")
            .map_err(|e| format!("Failed to set opt_level: {}", e))?;
        flag_builder
            .set("is_pic", "true")
            .map_err(|e| format!("Failed to set is_pic: {}", e))?;

        let isa_builder = cranelift_native::builder()
            .map_err(|e| format!("Failed to create native ISA builder: {}", e))?;

        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| format!("Failed to finish ISA: {}", e))?;

        let obj_builder = ObjectBuilder::new(
            isa,
            "riven_module",
            cranelift_module::default_libcall_names(),
        )
        .map_err(|e| format!("Failed to create ObjectBuilder: {}", e))?;

        let module = ObjectModule::new(obj_builder);
        let ctx = module.make_context();

        Ok(CodeGen {
            module,
            ctx,
            builder_ctx: FunctionBuilderContext::new(),
            string_data: HashMap::new(),
            string_counter: 0,
            declared_fns: HashMap::new(),
            user_fn_param_tys: HashMap::new(),
        })
    }

    /// Compile all functions in a MIR program.
    ///
    /// Two-pass: first declare all functions, then define them.
    /// FFI declarations from `lib` and `extern "C"` blocks are declared
    /// as imported functions so they can be called from user code.
    pub fn compile_program(&mut self, program: &MirProgram) -> Result<(), String> {
        // ── Pass 0: declare FFI functions ────────────────────────────────
        for lib in &program.ffi_libs {
            for ffi_fn in &lib.functions {
                let call_conv = self.module.isa().default_call_conv();
                let mut sig = Signature::new(call_conv);
                let mut param_tys: Vec<Type> = Vec::with_capacity(ffi_fn.param_types.len());
                for param_ty in &ffi_fn.param_types {
                    if let Some(cl_ty) = ty_to_cranelift(param_ty) {
                        sig.params.push(AbiParam::new(cl_ty));
                        param_tys.push(cl_ty);
                    }
                }
                if let Some(ref ret_ty) = ffi_fn.return_type {
                    if let Some(cl_ty) = ty_to_cranelift(ret_ty) {
                        sig.returns.push(AbiParam::new(cl_ty));
                    }
                }
                let func_id = self
                    .module
                    .declare_function(&ffi_fn.name, Linkage::Import, &sig)
                    .map_err(|e| {
                        format!("Failed to declare FFI function '{}': {}", ffi_fn.name, e)
                    })?;
                self.declared_fns.insert(ffi_fn.name.clone(), func_id);
                self.user_fn_param_tys
                    .insert(ffi_fn.name.clone(), param_tys.clone());

                // Also register with the lib-qualified name (e.g., "LibM.sin")
                if !lib.name.is_empty() {
                    let qualified = format!("{}_{}", lib.name, ffi_fn.name);
                    self.declared_fns.insert(qualified.clone(), func_id);
                    self.user_fn_param_tys.insert(qualified, param_tys);
                }
            }
        }

        // ── Pass 1: declare ──────────────────────────────────────────────
        for func in &program.functions {
            let sig = build_signature(&self.module, func);
            let linkage = if func.name == "main" {
                Linkage::Export
            } else {
                Linkage::Local
            };

            // Record the cranelift param types so `coerce_call_args` can
            // apply the correct narrow-int signature when this fn is the
            // callee (Phase 2 #06.D2.S3: synth `Bool_fmt`/`Char_fmt` etc.
            // legitimately take narrow params and must not be widened).
            let param_tys: Vec<Type> = sig.params.iter().map(|p| p.value_type).collect();
            self.user_fn_param_tys.insert(func.name.clone(), param_tys);

            let func_id = self
                .module
                .declare_function(&func.name, linkage, &sig)
                .map_err(|e| format!("Failed to declare function '{}': {}", func.name, e))?;

            self.declared_fns.insert(func.name.clone(), func_id);
        }

        // ── Pass 2: define ───────────────────────────────────────────────
        for func in &program.functions {
            self.compile_function(func)?;
        }

        Ok(())
    }

    /// Emit the finished object file as raw bytes.
    pub fn finish(self) -> Result<Vec<u8>, String> {
        let product = self.module.finish();
        let bytes = product
            .emit()
            .map_err(|e| format!("Failed to emit object: {}", e))?;
        Ok(bytes)
    }

    /// Translate one MIR function into Cranelift IR and define it.
    fn compile_function(&mut self, func: &MirFunction) -> Result<(), String> {
        let sig = build_signature(&self.module, func);
        self.ctx.func.signature = sig;

        // We need to split borrows: the FunctionBuilder borrows ctx.func and
        // builder_ctx, while instruction translation needs module, declared_fns,
        // string_data, etc. We extract the "env" fields into a separate struct.
        {
            let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_ctx);

            let mut env = TranslationEnv {
                module: &mut self.module,
                declared_fns: &mut self.declared_fns,
                string_data: &mut self.string_data,
                string_counter: &mut self.string_counter,
                user_fn_param_tys: &self.user_fn_param_tys,
            };

            // ── Map MIR blocks → Cranelift blocks ────────────────────────
            let mut block_map: Vec<cranelift_codegen::ir::Block> =
                Vec::with_capacity(func.blocks.len());
            for _ in &func.blocks {
                block_map.push(builder.create_block());
            }

            // ── Declare Cranelift Variables for all locals ────────────────
            let mut var_map: HashMap<LocalId, Variable> = HashMap::new();
            for local in &func.locals {
                let cl_ty = ty_to_cranelift(&local.ty).unwrap_or(types::I64);
                let var = builder.declare_var(cl_ty);
                var_map.insert(local.id, var);
            }

            // ── Pre-scan for address-taken locals ────────────────────────
            // Any local whose address is taken via `&mut src` (RefMut) and
            // whose type is `String`/`Str` must live in a stack slot rather
            // than a Cranelift SSA variable, so the pointer we hand out
            // remains valid and observers see buffer reallocations written
            // through it (e.g. from `push`/`push_str`).
            //
            // We restrict promotion to String-typed locals because class
            // and struct receivers are already heap-pointers: a `&mut Foo`
            // in current Riven is passed by value as the object pointer,
            // and the callee reaches fields via `GetField(base, off)`.
            // Promoting those to a pointer-to-pointer would double-indirect
            // every field access and break class method calls on `&mut`
            // receivers. String is the only v1 type where the value itself
            // (a `char*` that grows) must be observably reassigned.
            let mut address_taken: HashSet<LocalId> = HashSet::new();
            for block in &func.blocks {
                for inst in &block.instructions {
                    if let MirInst::RefMut { src, .. } = inst {
                        if let Some(local) = func.locals.get(*src as usize) {
                            if is_string_mir_ty(&local.ty) {
                                address_taken.insert(*src);
                            }
                        }
                    }
                }
            }

            // Allocate one 8-byte stack slot per address-taken local.
            let mut stack_slots: HashMap<LocalId, StackSlot> = HashMap::new();
            for &local_id in &address_taken {
                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3, // log2(align) = 3 → 8-byte alignment
                ));
                stack_slots.insert(local_id, slot);
            }

            // ── Set up entry block ───────────────────────────────────────
            let entry_cl_block = block_map[func.entry_block];
            builder.switch_to_block(entry_cl_block);
            builder.append_block_params_for_function_params(entry_cl_block);

            if func.name == "main" {
                let params_vals = builder.block_params(entry_cl_block).to_vec();
                let argc = params_vals
                    .first()
                    .copied()
                    .ok_or_else(|| "missing argc param for main".to_string())?;
                let argv = params_vals
                    .get(1)
                    .copied()
                    .ok_or_else(|| "missing argv param for main".to_string())?;
                let env_init = env.declare_runtime_func(
                    "riven_env_init",
                    &[types::I32, types::I64],
                    None,
                    &mut builder,
                )?;
                builder.ins().call(env_init, &[argc, argv]);
            }

            // Bind function parameters to their local variables.
            if func.name != "main" {
                let params_vals = builder.block_params(entry_cl_block).to_vec();
                for (i, &param_id) in func.params.iter().enumerate() {
                    if i < params_vals.len() {
                        def_local(
                            &var_map,
                            &stack_slots,
                            &mut builder,
                            param_id,
                            params_vals[i],
                        );
                    }
                }
            }

            // ── Translate each block ─────────────────────────────────────
            for (mir_idx, mir_block) in func.blocks.iter().enumerate() {
                let cl_block = block_map[mir_idx];

                if mir_idx != func.entry_block {
                    builder.switch_to_block(cl_block);
                }

                for inst in &mir_block.instructions {
                    if let Err(e) = translate_instruction(
                        inst,
                        func,
                        &var_map,
                        &stack_slots,
                        &block_map,
                        &mut builder,
                        &mut env,
                    ) {
                        return Err(format!(
                            "Error in function '{}', block {}, instruction {:?}: {}",
                            func.name, mir_idx, inst, e
                        ));
                    }
                }

                translate_terminator(
                    &mir_block.terminator,
                    func,
                    &var_map,
                    &stack_slots,
                    &block_map,
                    &mut builder,
                    &mut env,
                )?;
            }

            // Seal all blocks after translation so that forward edges
            // (e.g. from Switch terminators) are registered before sealing.
            builder.seal_all_blocks();

            builder.finalize();
        }

        // Define the function in the module.
        let func_id = *self
            .declared_fns
            .get(&func.name)
            .ok_or_else(|| format!("Function '{}' was not declared", func.name))?;

        self.module
            .define_function(func_id, &mut self.ctx)
            .map_err(|e| format!("Failed to define function '{}': {:?}", func.name, e))?;

        self.module.clear_context(&mut self.ctx);
        Ok(())
    }
}

// Re-export the `Module` trait usage that callers need; the original
// monolithic file relied on `use cranelift_module::Module;` being in
// scope for `module.declare_function` / `module.finish`.  Submodules
// already import what they need locally.
