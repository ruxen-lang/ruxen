//! Cranelift code generation backend for the Riven compiler.
//!
//! Translates MIR programs into native object code via Cranelift's
//! `ObjectModule`. The pipeline is:
//!   1. Declare all functions (two-pass: declare then define).
//!   2. For each function, translate MIR blocks to Cranelift IR.
//!   3. Emit the finished object bytes.

use std::collections::{HashMap, HashSet};

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::types::{self, Type};
use cranelift_codegen::ir::{
    AbiParam, InstBuilder, MemFlags, Signature, StackSlot, StackSlotData, StackSlotKind,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{DataDescription, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::hir::types::Ty;
use crate::mir::nodes::*;
use crate::parser::ast::BinOp;

use super::runtime::{extract_method_name, runtime_name};

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
            self.user_fn_param_tys
                .insert(func.name.clone(), param_tys);

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

// ════════════════════════════════════════════════════════════════════════════
//  Translation environment — holds module-level state needed during
//  instruction translation, split from the FunctionBuilder borrow.
// ════════════════════════════════════════════════════════════════════════════

struct TranslationEnv<'a> {
    module: &'a mut ObjectModule,
    declared_fns: &'a mut HashMap<String, FuncId>,
    string_data: &'a mut HashMap<String, cranelift_module::DataId>,
    string_counter: &'a mut u32,
    /// Param types of every previously-declared user / FFI fn.  Read-only:
    /// instruction lowering never declares new entries here.  Used so that
    /// `coerce_call_args` can apply the *real* narrow-int signature of
    /// known user fns instead of the default widen-to-i64 fallback.
    user_fn_param_tys: &'a HashMap<String, Vec<Type>>,
}

impl<'a> TranslationEnv<'a> {
    /// Create a data section for a null-terminated string literal.
    fn create_string_data(&mut self, value: &str) -> Result<cranelift_module::DataId, String> {
        if let Some(&data_id) = self.string_data.get(value) {
            return Ok(data_id);
        }

        let name = format!(".str.{}", *self.string_counter);
        *self.string_counter += 1;

        let data_id = self
            .module
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(|e| format!("Failed to declare string data '{}': {}", name, e))?;

        let mut desc = DataDescription::new();
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        desc.define(bytes.into_boxed_slice());

        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("Failed to define string data '{}': {}", name, e))?;

        self.string_data.insert(value.to_string(), data_id);
        Ok(data_id)
    }

    /// Get or declare a function by name, returning a `FuncRef` usable inside
    /// the current Cranelift function being built.
    fn get_or_declare_func(
        &mut self,
        name: &str,
        arg_vals: &[cranelift_codegen::ir::Value],
        has_return: bool,
        builder: &mut FunctionBuilder,
    ) -> Result<cranelift_codegen::ir::FuncRef, String> {
        if let Some(&func_id) = self.declared_fns.get(name) {
            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
            return Ok(func_ref);
        }

        // For inferred-type method calls (?T..._method), search for a
        // declared function whose name ends with _method. This resolves
        // calls like ?T260_message to TaskError_message.
        // Prefer the shortest match to avoid picking e.g.
        // TaskList_find_by_id when we want Task_id.
        if name.starts_with("?") {
            let method = extract_method_name(name);
            let suffix = format!("_{}", method);
            // Prefer exact "TypeName_method" form: the suffix should appear
            // right after the type name, with only one underscore-delimited
            // segment before the method name.  If multiple candidates match,
            // pick the shortest (most specific).
            let match_name = self
                .declared_fns
                .keys()
                .filter(|k| k.ends_with(&suffix) && !k.starts_with("?"))
                .min_by_key(|k| k.len())
                .cloned();
            if let Some(resolved) = match_name {
                let func_id = self.declared_fns[&resolved];
                let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                return Ok(func_ref);
            }
        }

        // For unresolved generic type parameters (e.g., T_assign, E_message),
        // use suffix matching to find the concrete method.
        if !self.declared_fns.contains_key(name) {
            let method = extract_method_name(name);
            let type_prefix = if let Some(pos) = name.find('_') {
                &name[..pos]
            } else {
                ""
            };
            // Match single-letter type params or common generic names
            let is_generic_param =
                type_prefix.len() <= 2 && type_prefix.chars().all(|c| c.is_ascii_uppercase());
            if is_generic_param && !type_prefix.is_empty() {
                let suffix = format!("_{}", method);
                let match_name = self
                    .declared_fns
                    .keys()
                    .filter(|k| {
                        k.ends_with(&suffix) && !k.starts_with("?") && k.len() > suffix.len()
                    })
                    .min_by_key(|k| k.len())
                    .cloned();
                if let Some(resolved) = match_name {
                    let func_id = self.declared_fns[&resolved];
                    let func_ref = self.module.declare_func_in_func(func_id, builder.func);
                    return Ok(func_ref);
                }
            }
        }

        // Try known runtime signatures first.
        if let Some((param_tys, ret_ty)) = runtime_signature(name) {
            return self.declare_runtime_func(name, &param_tys, ret_ty, builder);
        }

        // Fall back: infer signature from call-site.
        let call_conv = self.module.isa().default_call_conv();
        let mut sig = Signature::new(call_conv);
        for val in arg_vals {
            let ty = builder.func.dfg.value_type(*val);
            sig.params.push(AbiParam::new(ty));
        }
        if has_return {
            sig.returns.push(AbiParam::new(types::I64));
        }

        let func_id = self
            .module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| format!("Failed to declare imported function '{}': {}", name, e))?;

        self.declared_fns.insert(name.to_string(), func_id);
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        Ok(func_ref)
    }

    /// Declare a runtime function with an explicit signature.
    fn declare_runtime_func(
        &mut self,
        name: &str,
        params: &[Type],
        ret: Option<Type>,
        builder: &mut FunctionBuilder,
    ) -> Result<cranelift_codegen::ir::FuncRef, String> {
        if let Some(&func_id) = self.declared_fns.get(name) {
            let func_ref = self.module.declare_func_in_func(func_id, builder.func);
            return Ok(func_ref);
        }

        let call_conv = self.module.isa().default_call_conv();
        let mut sig = Signature::new(call_conv);
        for &p in params {
            sig.params.push(AbiParam::new(p));
        }
        if let Some(r) = ret {
            sig.returns.push(AbiParam::new(r));
        }

        let func_id = self
            .module
            .declare_function(name, Linkage::Import, &sig)
            .map_err(|e| format!("Failed to declare runtime function '{}': {}", name, e))?;

        self.declared_fns.insert(name.to_string(), func_id);
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        Ok(func_ref)
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Free functions — instruction and terminator translation
// ════════════════════════════════════════════════════════════════════════════

/// Build a Cranelift `Signature` from a MIR function.
fn build_signature(module: &ObjectModule, func: &MirFunction) -> Signature {
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
fn def_local(
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
/// `riven_store_ptr`) is visible to subsequent uses. Everything else goes
/// through the Cranelift SSA variable.
fn use_local(
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
fn translate_instruction(
    inst: &MirInst,
    func: &MirFunction,
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    _block_map: &[cranelift_codegen::ir::Block],
    builder: &mut FunctionBuilder,
    env: &mut TranslationEnv,
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
                // Call riven_string_eq(a, b) which returns 1 for equal, 0 for not.
                let func_ref = env.declare_runtime_func(
                    "riven_string_eq",
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
                // For ordered comparisons on strings, call riven_string_cmp
                let func_ref = env.declare_runtime_func(
                    "riven_string_cmp",
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
            // parameter types. Runtime helpers like `riven_puts`,
            // `riven_int_to_string`, and `riven_string_concat` all expect i64
            // args, but narrow Riven types (Char/Int32→i32, UInt8→i8, etc.)
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
                "riven_noop_passthrough" => {
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
                "riven_noop_return_null" => {
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
                "riven_noop" => {
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
                env.declare_runtime_func("riven_alloc", &[types::I64], Some(types::I64), builder)?;
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
            // MirInst::Drop is a marker — the actual `riven_dealloc` call
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

        MirInst::CallIndirect { dest, callee, args } => {
            let callee_val = use_local(var_map, stack_slots, builder, *callee);
            let mut arg_vals = Vec::with_capacity(args.len());
            for arg in args {
                arg_vals.push(gen_value(arg, func, var_map, stack_slots, builder)?);
            }

            // Build signature: all args are I64, return is I64 if dest exists.
            let call_conv = env.module.isa().default_call_conv();
            let mut sig = Signature::new(call_conv);
            for val in &arg_vals {
                let ty = builder.func.dfg.value_type(*val);
                sig.params.push(AbiParam::new(ty));
            }
            if dest.is_some() {
                sig.returns.push(AbiParam::new(types::I64));
            }
            let sig_ref = builder.import_signature(sig);
            let call = builder.ins().call_indirect(sig_ref, callee_val, &arg_vals);

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

    Ok(())
}

/// Translate a MIR terminator.
fn translate_terminator(
    term: &Terminator,
    func: &MirFunction,
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    block_map: &[cranelift_codegen::ir::Block],
    builder: &mut FunctionBuilder,
    _env: &mut TranslationEnv,
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
fn gen_value(
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
fn emit_binop(
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
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
//  Type and helper mappings
// ════════════════════════════════════════════════════════════════════════════

/// Coerce a Cranelift value to a target type if they differ.
///
/// Handles integer width conversions (e.g., I64 → I8, I8 → I64) using
/// `ireduce` (narrowing) or `uextend`/`sextend` (widening).
fn coerce_value(
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
fn coerce_value_signed(
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
/// The Riven narrow integer types (`Char`/`Int32` → i32, `UInt8` → i8, etc.)
/// are stored natively in Cranelift at their declared width for memory
/// efficiency. But runtime helpers (`riven_puts`, `riven_int_to_string`,
/// `riven_string_concat`, …) and most user-level callees expect i64 args —
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
fn coerce_call_args(
    arg_vals: &mut [cranelift_codegen::ir::Value],
    args: &[MirValue],
    func: &MirFunction,
    callee: &str,
    user_fn_param_tys: &HashMap<String, Vec<Type>>,
    builder: &mut FunctionBuilder,
) {
    // Resolve the callee's signature in priority order:
    //   1. `runtime_signature` — hand-rolled signature table for the C
    //      runtime helpers (`riven_*`).  Wins for known runtime fns.
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

/// Map a Riven `Ty` to a Cranelift IR type.
///
/// Returns `None` for `Unit` / `Never` (no runtime representation in return
/// position), `Some(type)` for everything else.
fn ty_to_cranelift(ty: &Ty) -> Option<Type> {
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
        | Ty::Vec(_)
        | Ty::HashMap(_, _)
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
        | Ty::DynTrait(_)
        | Ty::ImplTrait(_)
        | Ty::Alias { .. }
        | Ty::Newtype { .. }
        | Ty::TypeParam { .. }
        | Ty::Infer(_)
        | Ty::Tuple(_)
        | Ty::Array(_, _) => Some(types::I64),

        Ty::Unit | Ty::Never => None,
        Ty::Error => None,
        // T2.02 S6: const-arg markers never reach codegen on their
        // own — they live inside a parent type's generic_args list.
        Ty::ConstArg(_) => None,
    }
}

/// Map a MIR `CmpOp` to a Cranelift `IntCC`.
fn cmpop_to_intcc(op: CmpOp) -> IntCC {
    match op {
        CmpOp::Eq => IntCC::Equal,
        CmpOp::NotEq => IntCC::NotEqual,
        CmpOp::Lt => IntCC::SignedLessThan,
        CmpOp::LtEq => IntCC::SignedLessThanOrEqual,
        CmpOp::Gt => IntCC::SignedGreaterThan,
        CmpOp::GtEq => IntCC::SignedGreaterThanOrEqual,
    }
}

fn cmpop_to_floatcc(op: CmpOp) -> FloatCC {
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

/// Size estimate for heap allocation.
///
/// For classes and structs, we use a field-count-based heuristic (8 bytes
/// per field, since all fields are stored as 64-bit words). For enums,
/// we allocate tag (8 bytes) + payload. Minimum allocation is 8 bytes
/// for any composite type.
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
        // Enums: tag (8 bytes aligned) + payload (conservatively 8 bytes per field,
        // with space for the largest variant's payload).
        Ty::Enum { .. } => 32, // tag + up to 3 payload fields
        // Classes and structs: allocate generously.
        // A more precise calculation would require the symbol table,
        // but 64 bytes covers most cases (up to 8 fields).
        Ty::Class { .. } | Ty::Struct { .. } => 64,
        // Option: tag + payload
        Ty::Option(_) => 16,
        // Result: tag + payload
        Ty::Result(_, _) => 16,
        // Tuples: 8 bytes per element
        Ty::Tuple(elems) => elems.len().max(1) * 8,
        // Arrays.  T2.02 stage 4: `n` is a `ConstExpr`; pre-stage-7
        // codegen only resolves `Lit` values.
        Ty::Array(_, n) => n.as_lit().unwrap_or(0) as usize * 8,
        _ => 8,
    }
}

/// Known runtime function signatures.
///
/// Returns `(param_types, optional_return_type)`.
fn runtime_signature(name: &str) -> Option<(Vec<Type>, Option<Type>)> {
    match name {
        // I/O
        "puts" | "riven_puts" => Some((vec![types::I64], None)),
        "eputs" | "riven_eputs" => Some((vec![types::I64], None)),
        "print" | "riven_print" => Some((vec![types::I64], None)),
        "println" => Some((vec![types::I64], None)),
        "eprintln" => Some((vec![types::I64], None)),
        "read_line" | "riven_read_line" => Some((vec![], Some(types::I64))),
        "stdin" | "riven_stdin" => Some((vec![], Some(types::I64))),
        "stdout" | "riven_stdout" => Some((vec![], Some(types::I64))),
        "stderr" | "riven_stderr" => Some((vec![], Some(types::I64))),
        "riven_stdin_read_line" => Some((vec![types::I64], Some(types::I64))),
        "riven_stdin_read_to_string" => Some((vec![types::I64], Some(types::I64))),
        "riven_stdin_lines" => Some((vec![types::I64], Some(types::I64))),
        // Phase 2 stdlib (#06.A3): std::fmt::Formatter buffer surface.
        "riven_fmt_formatter_new" => Some((vec![], Some(types::I64))),
        "riven_fmt_formatter_free" => Some((vec![types::I64], None)),
        "riven_fmt_formatter_write_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_fmt_formatter_write_char" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_fmt_formatter_buffer" => Some((vec![types::I64], Some(types::I64))),
        "riven_fmt_formatter_len" => Some((vec![types::I64], Some(types::I64))),
        // Phase 2 stdlib (#06.D4): spec helpers.
        "riven_fmt_formatter_new_with_spec" => Some((
            vec![types::I64, types::I64, types::I64, types::I64],
            Some(types::I64),
        )),
        "riven_fmt_formatter_precision" => Some((vec![types::I64], Some(types::I64))),
        "riven_float_to_string_prec" => Some((vec![types::F64, types::I64], Some(types::I64))),
        "riven_string_truncate_chars" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_stdout_write_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_stdout_flush" => Some((vec![types::I64], Some(types::I64))),
        "riven_stderr_write_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_stderr_flush" => Some((vec![types::I64], Some(types::I64))),
        "riven_env_init" => Some((vec![types::I32, types::I64], None)),
        "riven_env_args_count" => Some((vec![], Some(types::I64))),
        "riven_env_args_at" => Some((vec![types::I64], Some(types::I64))),
        "args" | "riven_env_args" => Some((vec![], Some(types::I64))),
        "riven_env_var" => Some((vec![types::I64], Some(types::I64))),
        "riven_process_exit" => Some((vec![types::I64], None)),
        "riven_fs_read_to_string" => Some((vec![types::I64], Some(types::I64))),
        "riven_fs_write" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_fs_exists" => Some((vec![types::I64], Some(types::I64))),
        "riven_fs_remove_file" => Some((vec![types::I64], Some(types::I64))),
        "riven_fs_create_dir" => Some((vec![types::I64], Some(types::I64))),
        "riven_fs_create_dir_all" => Some((vec![types::I64], Some(types::I64))),
        "riven_fs_rename" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_print_int" => Some((vec![types::I64], None)),
        // Conversions
        "riven_int_to_string" => Some((vec![types::I64], Some(types::I64))),
        "riven_float_to_string" => Some((vec![types::F64], Some(types::I64))),
        "riven_bool_to_string" => Some((vec![types::I64], Some(types::I64))),
        "riven_char_to_string" => Some((vec![types::I64], Some(types::I64))),
        // String operations
        "riven_string_concat" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_string_from" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_push_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        // Pointer-to-pointer helpers used to implement &mut T mutation.
        "riven_deref_ptr" => Some((vec![types::I64], Some(types::I64))),
        "riven_store_ptr" => Some((vec![types::I64, types::I64], None)),
        "riven_string_len" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_is_empty" => Some((vec![types::I64], Some(types::I8))),
        "riven_string_trim" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_to_lower" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_to_upper" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_chars" => Some((vec![types::I64], Some(types::I64))),
        // String stdlib (#02): all char* / Vec ptr / Option / Result are
        // pointer-sized, so they ride the I64 calling convention.
        "riven_string_new" => Some((vec![], Some(types::I64))),
        "riven_string_with_capacity" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_as_str" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_to_string" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_bytes" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_trim_start" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_trim_end" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_find" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_string_splitn" => Some((vec![types::I64, types::I64, types::I64], Some(types::I64))),
        "riven_string_clear" => Some((vec![types::I64], None)),
        "riven_string_truncate" => Some((vec![types::I64, types::I64], None)),
        "riven_string_insert" => Some((vec![types::I64, types::I64, types::I64], Some(types::I64))),
        "riven_string_insert_str" => {
            Some((vec![types::I64, types::I64, types::I64], Some(types::I64)))
        }
        "riven_string_remove" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_string_parse_int" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_parse_float" => Some((vec![types::I64], Some(types::I64))),
        "riven_parse_error_message" => Some((vec![types::I64], Some(types::I64))),
        // String stdlib batch 2 (#02): split / push / into_bytes.
        "riven_string_split" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_string_push" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_string_into_bytes" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_eq" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_string_cmp" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_string_hash" => Some((vec![types::I64], Some(types::I64))),
        "riven_thread_sleep_ns" => Some((vec![types::I64], None)),
        "riven_thread_yield" => Some((vec![], None)),
        "riven_str_split" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_str_parse_uint" => Some((vec![types::I64], Some(types::I64))),
        // Memory
        "riven_alloc" => Some((vec![types::I64], Some(types::I64))),
        "riven_dealloc" => Some((vec![types::I64], None)),
        "riven_realloc" => Some((vec![types::I64, types::I64], Some(types::I64))),
        // Heap-owned built-in drops (P0.7).
        "riven_string_free" => Some((vec![types::I64], None)),
        "riven_vec_free" => Some((vec![types::I64], None)),
        "riven_hash_free" => Some((vec![types::I64], None)),
        // Phase 2 stdlib (#04 batch 2): set spine + HashMap/Set
        // per-element drop selectors.
        "riven_set_free" => Some((vec![types::I64], None)),
        "riven_hash_drop_string_v" => Some((vec![types::I64], None)),
        "riven_hash_drop_v_string" => Some((vec![types::I64], None)),
        "riven_hash_drop_string_string" => Some((vec![types::I64], None)),
        "riven_hash_drop_v_vec" => Some((vec![types::I64], None)),
        "riven_set_drop_string" => Some((vec![types::I64], None)),
        // Phase 2 stdlib batch 2 (#03): element-aware Vec drop helpers
        // and the new from_iter / dedup / set surface.
        "riven_vec_drop_string" => Some((vec![types::I64], None)),
        "riven_vec_drop_vec" => Some((vec![types::I64], None)),
        "riven_vec_from_iter" => Some((vec![types::I64], Some(types::I64))),
        "riven_string_from_iter" => Some((vec![types::I64], Some(types::I64))),
        "riven_vec_dedup" => Some((vec![types::I64], None)),
        "riven_vec_set" => Some((vec![types::I64, types::I64, types::I64], None)),
        // Panic
        "riven_panic" => Some((vec![types::I64], None)),
        // Vec operations
        "riven_vec_new" => Some((vec![], Some(types::I64))),
        "riven_vec_push" => Some((vec![types::I64, types::I64], None)),
        "riven_vec_pop" => Some((vec![types::I64], Some(types::I64))),
        "riven_vec_len" => Some((vec![types::I64], Some(types::I64))),
        "riven_vec_get" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_vec_get_opt" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_vec_get_mut" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_vec_get_mut_opt" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_vec_is_empty" => Some((vec![types::I64], Some(types::I8))),
        "riven_vec_each" => Some((vec![types::I64, types::I64], None)),
        // Hash operations
        "riven_hash_new" => Some((vec![], Some(types::I64))),
        "riven_hash_from_iter" => Some((vec![types::I64], Some(types::I64))),
        "riven_hash_insert" => Some((vec![types::I64, types::I64, types::I64], None)),
        "riven_hash_get" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_hash_contains_key" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "riven_hash_len" => Some((vec![types::I64], Some(types::I64))),
        "riven_hash_is_empty" => Some((vec![types::I64], Some(types::I8))),
        // Set operations
        "riven_set_new" => Some((vec![], Some(types::I64))),
        "riven_set_from_iter" => Some((vec![types::I64], Some(types::I64))),
        "riven_set_insert" => Some((vec![types::I64, types::I64], None)),
        "riven_set_contains" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "riven_set_len" => Some((vec![types::I64], Some(types::I64))),
        "riven_set_is_empty" => Some((vec![types::I64], Some(types::I8))),
        // Phase 2 stdlib (#04): HashMap[K,V] full surface.
        "riven_hash_with_capacity" => Some((vec![types::I64], Some(types::I64))),
        "riven_hash_remove" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_hash_clear" => Some((vec![types::I64], None)),
        "riven_hash_keys" => Some((vec![types::I64], Some(types::I64))),
        "riven_hash_values" => Some((vec![types::I64], Some(types::I64))),
        "riven_hash_iter" => Some((vec![types::I64], Some(types::I64))),
        "riven_hash_eq" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "riven_hash_index" => Some((vec![types::I64, types::I64], Some(types::I64))),
        // Phase 2 stdlib (#04): HashSet[T] full surface.
        "riven_set_with_capacity" => Some((vec![types::I64], Some(types::I64))),
        "riven_set_remove" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "riven_set_clear" => Some((vec![types::I64], None)),
        "riven_set_iter" => Some((vec![types::I64], Some(types::I64))),
        "riven_set_eq" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "riven_set_union" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_set_intersection" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_set_difference" => Some((vec![types::I64, types::I64], Some(types::I64))),
        // Option/Result helpers
        "riven_option_unwrap_or" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_result_unwrap_or_else" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_result_try_op" => Some((vec![types::I64], Some(types::I64))),
        "riven_result_expect" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_result_unwrap" => Some((vec![types::I64], Some(types::I64))),
        "riven_option_expect" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "riven_option_unwrap" => Some((vec![types::I64], Some(types::I64))),
        "riven_result_ok" => Some((vec![types::I64], Some(types::I64))),
        "riven_result_err" => Some((vec![types::I64], Some(types::I64))),
        "riven_option_is_some" => Some((vec![types::I64], Some(types::I8))),
        "riven_option_is_none" => Some((vec![types::I64], Some(types::I8))),
        "riven_result_is_ok" => Some((vec![types::I64], Some(types::I8))),
        "riven_result_is_err" => Some((vec![types::I64], Some(types::I8))),
        // No-ops: these are declared via call-site inference (variable arity).
        "riven_noop" => Some((vec![], None)),
        _ => None,
    }
}
