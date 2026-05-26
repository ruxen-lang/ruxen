//! Cranelift JIT code generation for the Ruxen REPL.
//!
//! Wraps `JITModule` for in-process compilation and execution.
//! Separate from the batch `CodeGen` (which uses `ObjectModule`)
//! to avoid generification complexity.

use std::collections::{HashMap, HashSet};

use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::types::{self, Type};
use cranelift_codegen::ir::{
    AbiParam, InstBuilder, MemFlags, Signature, StackSlot, StackSlotData, StackSlotKind,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};

use ruxen_core::codegen::runtime::{extract_method_name, runtime_name};
use ruxen_core::hir::types::Ty;
use ruxen_core::mir::nodes::*;
use ruxen_core::parser::ast::BinOp;

// ── C runtime function declarations ────────────────────────────────
// These are linked into the REPL binary and registered as JIT symbols.

extern "C" {
    // Note: ruxen_puts / ruxen_print / ruxen_eputs / ruxen_print_int /
    // ruxen_print_float are intentionally NOT declared here — they are
    // swapped at symbol-registration time with capture shims (see
    // `crate::capture`) that append into a buffer we diff to surface only
    // the newest input's output.
    fn ruxen_int_to_string(n: i64) -> *const i8;
    fn ruxen_float_to_string(f: f64) -> *const i8;
    fn ruxen_bool_to_string(b: i64) -> *const i8;
    fn ruxen_char_to_string(c: i64) -> *const i8;
    fn ruxen_string_concat(a: *const i8, b: *const i8) -> *const i8;
    fn ruxen_string_from(s: *const i8) -> *const i8;
    fn ruxen_string_push_str(s: *const i8, t: *const i8) -> *const i8;
    fn ruxen_string_len(s: *const i8) -> i64;
    fn ruxen_string_is_empty(s: *const i8) -> i8;
    fn ruxen_string_trim(s: *const i8) -> *const i8;
    fn ruxen_string_to_lower(s: *const i8) -> *const i8;
    fn ruxen_string_to_upper(s: *const i8) -> *const i8;
    fn ruxen_string_chars(s: *const i8) -> *mut u8;
    fn ruxen_string_eq(a: *const i8, b: *const i8) -> i64;
    fn ruxen_string_cmp(a: *const i8, b: *const i8) -> i64;
    fn ruxen_str_split(s: *const i8, d: *const i8) -> *const i8;
    fn ruxen_str_parse_uint(s: *const i8) -> i64;
    fn ruxen_deref_ptr(p: *const i8) -> *const i8;
    fn ruxen_store_ptr(p: *mut i8, v: *const i8);
    fn ruxen_alloc(size: i64) -> *mut u8;
    fn ruxen_dealloc(ptr: *mut u8);
    fn ruxen_realloc(ptr: *mut u8, new_size: i64) -> *mut u8;
    fn ruxen_panic(msg: *const i8);
    fn ruxen_vec_new() -> *mut u8;
    fn ruxen_vec_push(v: *mut u8, item: i64);
    fn ruxen_vec_pop(v: *mut u8) -> i64;
    fn ruxen_vec_len(v: *mut u8) -> i64;
    fn ruxen_vec_get(v: *mut u8, idx: i64) -> i64;
    fn ruxen_vec_get_opt(v: *mut u8, idx: i64) -> i64;
    fn ruxen_vec_get_mut(v: *mut u8, idx: i64) -> i64;
    fn ruxen_vec_get_mut_opt(v: *mut u8, idx: i64) -> i64;
    fn ruxen_vec_is_empty(v: *mut u8) -> i8;
    fn ruxen_vec_each(v: *mut u8, cb: *const u8);
    fn ruxen_hash_new() -> *mut u8;
    fn ruxen_hash_insert(h: *mut u8, k: i64, v: i64);
    fn ruxen_hash_get(h: *mut u8, k: i64) -> i64;
    fn ruxen_hash_contains_key(h: *mut u8, k: i64) -> i8;
    fn ruxen_hash_len(h: *mut u8) -> i64;
    fn ruxen_hash_is_empty(h: *mut u8) -> i8;
    fn ruxen_set_new() -> *mut u8;
    fn ruxen_set_insert(s: *mut u8, v: i64);
    fn ruxen_set_contains(s: *mut u8, v: i64) -> i8;
    fn ruxen_set_len(s: *mut u8) -> i64;
    fn ruxen_set_is_empty(s: *mut u8) -> i8;
    fn ruxen_option_unwrap_or(opt: *mut u8, default: i64) -> i64;
    fn ruxen_option_expect(opt: *mut u8, msg: *const i8) -> i64;
    fn ruxen_option_unwrap(opt: *mut u8) -> i64;
    fn ruxen_option_is_some(opt: *mut u8) -> i8;
    fn ruxen_option_is_none(opt: *mut u8) -> i8;
    fn ruxen_result_unwrap_or_else(result: *mut u8, handler: *const u8) -> i64;
    fn ruxen_result_try_op(result: *mut u8) -> i64;
    fn ruxen_result_expect(result: *mut u8, msg: *const i8) -> i64;
    fn ruxen_result_unwrap(result: *mut u8) -> i64;
    fn ruxen_result_is_ok(result: *mut u8) -> i8;
    fn ruxen_result_is_err(result: *mut u8) -> i8;
    fn ruxen_result_ok(result: *mut u8) -> i64;
    fn ruxen_result_err(result: *mut u8) -> i64;
    fn ruxen_noop_passthrough(val: i64) -> i64;
    fn ruxen_noop_return_null() -> i64;
    fn ruxen_noop();

    // ruby-naming.spec.md §3.6 / #06.D interp routing: bare `"#{x}"`
    // for a primitive (Int/Float/Bool/Char) lowers through
    // `{Type}_fmt(value, formatter)` + `Formatter_buffer(formatter)`
    // → `ruxen_fmt_formatter_*` C symbols. Without these registrations
    // even `1 + 2` panics in the REPL with "can't resolve symbol
    // ruxen_fmt_formatter_write_str".
    fn ruxen_fmt_formatter_new() -> *mut u8;
    fn ruxen_fmt_formatter_new_with_spec(
        fill: i64,
        align: i64,
        sign: i64,
        alt: i64,
        zero: i64,
        width: i64,
        precision: i64,
    ) -> *mut u8;
    fn ruxen_fmt_formatter_write_str(f: *mut u8, s: *const i8) -> i64;
    fn ruxen_fmt_formatter_write_char(f: *mut u8, c: i64) -> i64;
    fn ruxen_fmt_formatter_buffer(f: *mut u8) -> *const i8;
    fn ruxen_fmt_formatter_len(f: *const u8) -> i64;
    fn ruxen_fmt_formatter_free(f: *mut u8);
    fn ruxen_fmt_formatter_precision(f: *const u8) -> i64;
}

/// Cranelift JIT code generation engine for the REPL.
pub struct JITCodeGen {
    module: JITModule,
    ctx: Context,
    builder_ctx: FunctionBuilderContext,
    string_data: HashMap<String, cranelift_module::DataId>,
    vtable_data: HashMap<String, DataId>,
    defined_vtable_data: HashSet<String>,
    string_counter: u32,
    declared_fns: HashMap<String, FuncId>,
    /// Param Cranelift types for every user/synth function the JIT has
    /// declared. coerce_call_args consults this to know e.g. that
    /// `Bool_fmt`'s first param is i8 (not the i64 a default widen
    /// would produce) — without it, calling synthesized primitive
    /// formatters from the JIT trips Cranelift's verifier.
    user_fn_param_tys: HashMap<String, Vec<Type>>,
}

impl JITCodeGen {
    /// Create a new JIT code generator targeting the host machine.
    ///
    /// Key difference from batch `CodeGen`: `is_pic = false` since JIT code
    /// runs at known absolute addresses in process memory.
    pub fn new() -> Result<Self, String> {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("opt_level", "none")
            .map_err(|e| format!("Failed to set opt_level: {}", e))?;
        // JIT code runs at absolute addresses — NOT position-independent
        flag_builder
            .set("is_pic", "false")
            .map_err(|e| format!("Failed to set is_pic: {}", e))?;

        let isa_builder = cranelift_native::builder()
            .map_err(|e| format!("Failed to create native ISA builder: {}", e))?;

        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| format!("Failed to finish ISA: {}", e))?;

        let mut jit_builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register all C runtime functions as JIT symbols. The explicit
        // registrations below cover the print-family capture shims and
        // a hand-picked subset for documentation; the dlsym fallback
        // below picks up everything else statically linked into this
        // binary (the runtime.c family + libc).
        register_runtime_symbols(&mut jit_builder);

        // Fallback symbol lookup via libc::dlsym. The ruxen-repl binary
        // statically links ruxen-core's C runtime (see build.rs), so
        // every `ruxen_*` symbol is present in the process address
        // space and dlsym can resolve it without an explicit
        // builder.symbol() entry. This keeps the REPL in sync with new
        // runtime symbols without per-symbol maintenance.
        jit_builder.symbol_lookup_fn(Box::new(|name: &str| {
            // Security boundary: `dlsym(RTLD_DEFAULT)` resolves ANY
            // symbol visible in the host process, including libc.
            // Without this allowlist a `lib def system as "system"` line
            // in user input JITs straight to `libc::system`. Restrict
            // to symbols we actually expect: the `ruxen_*` runtime,
            // synth helpers like `<Mixin>_dynamic_<method>` /
            // `<Class>_<method>` / `Future_*` / `Formatter_*`, and a
            // small set of libc primitives the runtime itself depends
            // on (no exec / process-control surface).
            if !is_repl_symbol_allowed(name) {
                return None;
            }
            // SAFETY: dlsym on a NUL-terminated string returned by
            // CString::new. RTLD_DEFAULT searches the main program and
            // dependent dynlibs in the standard order.
            let c_name = match std::ffi::CString::new(name) {
                Ok(c) => c,
                Err(_) => return None,
            };
            unsafe {
                let handle = libc::RTLD_DEFAULT;
                let sym = libc::dlsym(handle, c_name.as_ptr());
                if sym.is_null() {
                    None
                } else {
                    Some(sym as *const u8)
                }
            }
        }));

        let module = JITModule::new(jit_builder);
        let ctx = module.make_context();

        Ok(JITCodeGen {
            module,
            ctx,
            builder_ctx: FunctionBuilderContext::new(),
            string_data: HashMap::new(),
            vtable_data: HashMap::new(),
            defined_vtable_data: HashSet::new(),
            string_counter: 0,
            declared_fns: HashMap::new(),
            user_fn_param_tys: HashMap::new(),
        })
    }

    /// Compile a single REPL input (wrapped as `__repl_N` function) and return
    /// a callable function pointer.
    pub fn compile_repl_input(&mut self, mir_function: &MirFunction) -> Result<*const u8, String> {
        // Declare
        let sig = build_jit_signature(&self.module, mir_function);
        let func_id = self
            .module
            .declare_function(&mir_function.name, Linkage::Export, &sig)
            .map_err(|e| {
                format!(
                    "Failed to declare REPL function '{}': {}",
                    mir_function.name, e
                )
            })?;

        let param_tys: Vec<Type> = sig.params.iter().map(|p| p.value_type).collect();
        self.user_fn_param_tys
            .insert(mir_function.name.clone(), param_tys);
        self.declared_fns.insert(mir_function.name.clone(), func_id);

        // Define — on failure, drop the declared_fns entry so a retry
        // with a fresh wrapper name isn't blocked by a dangling symbol.
        if let Err(e) = self.compile_function_inner(mir_function, func_id) {
            self.declared_fns.remove(&mir_function.name);
            return Err(e);
        }

        // Finalize and get pointer
        self.module
            .finalize_definitions()
            .map_err(|e| format!("Failed to finalize: {}", e))?;

        let code_ptr = self.module.get_finalized_function(func_id);
        Ok(code_ptr)
    }

    /// Compile a user-defined function and register it in the JIT module.
    ///
    /// Does NOT finalize — callers that compile a batch of inter-referencing
    /// functions (e.g. `make_adder` + its `__closure_0`) must call
    /// `finalize_definitions` exactly once after all bodies are defined.
    pub fn compile_function(&mut self, mir_function: &MirFunction) -> Result<(), String> {
        self.declare_function(mir_function)?;
        let func_id = self.declared_fns[&mir_function.name];
        self.compile_function_inner(mir_function, func_id)
    }

    /// Declare (but don't define) a function. Idempotent — declaring an
    /// already-declared function is a no-op.
    pub fn declare_function(&mut self, mir_function: &MirFunction) -> Result<FuncId, String> {
        if let Some(&id) = self.declared_fns.get(&mir_function.name) {
            return Ok(id);
        }
        let sig = build_jit_signature(&self.module, mir_function);
        let func_id = self
            .module
            .declare_function(&mir_function.name, Linkage::Export, &sig)
            .map_err(|e| format!("Failed to declare function '{}': {}", mir_function.name, e))?;
        let param_tys: Vec<Type> = sig.params.iter().map(|p| p.value_type).collect();
        self.user_fn_param_tys
            .insert(mir_function.name.clone(), param_tys);
        self.declared_fns.insert(mir_function.name.clone(), func_id);
        Ok(func_id)
    }

    /// Finalize all pending definitions so their symbols become callable.
    pub fn finalize(&mut self) -> Result<(), String> {
        self.module
            .finalize_definitions()
            .map_err(|e| format!("Failed to finalize: {}", e))
    }

    /// Check if a function name is already declared in the JIT module.
    pub fn is_declared(&self, name: &str) -> bool {
        self.declared_fns.contains_key(name)
    }

    /// Declare vtable and class_info data symbols for a lowered program.
    ///
    /// The REPL lowers the accumulated session on every input, so this is
    /// intentionally idempotent across programs that re-emit the same stdlib
    /// metadata.
    pub fn declare_program_data(&mut self, program: &MirProgram) -> Result<(), String> {
        // ── FFI declarations ─────────────────────────────────────────────
        // Mirror the batch backend's `CodeGen::compile_program` Pass 0
        // (compiler/ruxen_core/src/codegen/cranelift/mod.rs): declare every
        // `lib`/`extern` function — including the ones merged in from the
        // stdlib bootstrap (`String.new as "ruxen_string_new"`, the fs /
        // async symbols, …) — as an imported function, and register it in
        // `declared_fns` under BOTH `ffi_fn.name` (the linked C symbol) AND
        // `ffi_fn.ruxen_name` (the mangled call-site identifier).
        //
        // Without this, a paren-less zero-arg static FFI call like
        // `let s = String.new` lowers to a MIR `Call { callee: "String_new" }`
        // (the ruxen_name, NOT the `as`-aliased C symbol), and
        // `get_or_declare_func` falls through to declaring a raw import
        // literally named `String_new`. dlsym(RTLD_DEFAULT) can't resolve
        // that — it isn't a `ruxen_*` runtime symbol — so the JIT panics
        // with "can't resolve symbol String_new". Declaring the import here
        // under the C name and aliasing the ruxen_name to the same FuncId
        // makes the call resolve to `ruxen_string_new` exactly as the AOT
        // path does.
        for lib in &program.ffi_libs {
            for ffi_fn in &lib.functions {
                // Reuse an existing declaration of the C symbol if one was
                // already made (e.g. via the `runtime_signature` path on a
                // prior input) so we never declare the same import twice;
                // otherwise declare it fresh.
                let func_id = if let Some(&id) = self.declared_fns.get(&ffi_fn.name) {
                    id
                } else {
                    let call_conv = self.module.isa().default_call_conv();
                    let mut sig = Signature::new(call_conv);
                    // Prefer the hand-maintained ABI table (`runtime_signature`)
                    // — it is the authoritative width for each runtime symbol
                    // and matches what call sites emit. Deriving purely from
                    // the HIR `param_types` via `ty_to_cranelift` widens
                    // `Char`→i32, but the REPL passes chars as i64, so the
                    // verifier rejects the call ("arg has type i64, expected
                    // i32"). Fall back to the HIR types for user FFI symbols
                    // the table doesn't know.
                    match runtime_signature(&ffi_fn.name) {
                        Some((param_tys, ret_ty)) => {
                            for p in param_tys {
                                sig.params.push(AbiParam::new(p));
                            }
                            if let Some(r) = ret_ty {
                                sig.returns.push(AbiParam::new(r));
                            }
                        }
                        None => {
                            for param_ty in &ffi_fn.param_types {
                                if let Some(cl_ty) = ty_to_cranelift(param_ty) {
                                    sig.params.push(AbiParam::new(cl_ty));
                                }
                            }
                            if let Some(ref ret_ty) = ffi_fn.return_type {
                                if let Some(cl_ty) = ty_to_cranelift(ret_ty) {
                                    sig.returns.push(AbiParam::new(cl_ty));
                                }
                            }
                        }
                    }
                    let id = self
                        .module
                        .declare_function(&ffi_fn.name, Linkage::Import, &sig)
                        .map_err(|e| format!("declare FFI function '{}': {}", ffi_fn.name, e))?;
                    self.declared_fns.insert(ffi_fn.name.clone(), id);
                    id
                };
                // Alias the mangled call-site name (`String_new`) onto the
                // same FuncId as the linked C symbol (`ruxen_string_new`).
                if ffi_fn.ruxen_name != ffi_fn.name {
                    self.declared_fns
                        .entry(ffi_fn.ruxen_name.clone())
                        .or_insert(func_id);
                }
            }
        }

        for vt in &program.vtables {
            let sym = vt.symbol();
            if self.vtable_data.contains_key(&sym) {
                continue;
            }
            let data_id = self
                .module
                .declare_data(&sym, Linkage::Local, false, false)
                .map_err(|e| format!("declare vtable data '{}': {}", sym, e))?;
            self.vtable_data.insert(sym, data_id);
        }

        for ci in &program.class_infos {
            let sym = ci.symbol();
            if self.vtable_data.contains_key(&sym) {
                continue;
            }
            let data_id = self
                .module
                .declare_data(&sym, Linkage::Local, false, false)
                .map_err(|e| format!("declare class_info data '{}': {}", sym, e))?;
            self.vtable_data.insert(sym, data_id);
        }

        Ok(())
    }

    /// Define any not-yet-defined vtable and class_info data for a program.
    ///
    /// Call after all MIR functions in the program have been declared, because
    /// vtable entries relocate to method function symbols.
    pub fn define_program_data(&mut self, program: &MirProgram) -> Result<(), String> {
        const PTR_SIZE: u32 = 8;

        for vt in &program.vtables {
            let sym = vt.symbol();
            if self.defined_vtable_data.contains(&sym) {
                continue;
            }
            let data_id = *self
                .vtable_data
                .get(&sym)
                .ok_or_else(|| format!("vtable data '{}' was not declared", sym))?;
            let size = (vt.method_symbols.len() as u32) * PTR_SIZE;
            let mut desc = DataDescription::new();
            desc.define(vec![0u8; size as usize].into_boxed_slice());
            desc.set_align(PTR_SIZE as u64);
            for (i, method_sym) in vt.method_symbols.iter().enumerate() {
                let func_id = *self.declared_fns.get(method_sym).ok_or_else(|| {
                    format!(
                        "mixin-vtables: method symbol '{}' for vtable '{}' not declared",
                        method_sym, sym
                    )
                })?;
                let func_ref = self.module.declare_func_in_data(func_id, &mut desc);
                desc.write_function_addr((i as u32) * PTR_SIZE, func_ref);
            }
            self.module
                .define_data(data_id, &desc)
                .map_err(|e| format!("define vtable data '{}': {}", sym, e))?;
            self.defined_vtable_data.insert(sym);
        }

        for ci in &program.class_infos {
            let sym = ci.symbol();
            if self.defined_vtable_data.contains(&sym) {
                continue;
            }
            let data_id = *self
                .vtable_data
                .get(&sym)
                .ok_or_else(|| format!("class_info data '{}' was not declared", sym))?;
            let size = (ci.vtable_symbols.len() as u32) * PTR_SIZE;
            let mut desc = DataDescription::new();
            desc.define(vec![0u8; size as usize].into_boxed_slice());
            desc.set_align(PTR_SIZE as u64);
            for (i, vt_sym) in ci.vtable_symbols.iter().enumerate() {
                let vt_data_id = *self.vtable_data.get(vt_sym).ok_or_else(|| {
                    format!(
                        "mixin-vtables: class_info '{}' references unknown vtable '{}'",
                        sym, vt_sym
                    )
                })?;
                let gv = self.module.declare_data_in_data(vt_data_id, &mut desc);
                desc.write_data_addr((i as u32) * PTR_SIZE, gv, 0);
            }
            self.module
                .define_data(data_id, &desc)
                .map_err(|e| format!("define class_info data '{}': {}", sym, e))?;
            self.defined_vtable_data.insert(sym);
        }

        Ok(())
    }

    /// Internal: translate MIR to Cranelift IR and define the function.
    fn compile_function_inner(
        &mut self,
        func: &MirFunction,
        func_id: FuncId,
    ) -> Result<(), String> {
        let sig = build_jit_signature(&self.module, func);
        self.ctx.func.signature = sig;

        {
            let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_ctx);

            let mut env = JITTranslationEnv {
                module: &mut self.module,
                declared_fns: &mut self.declared_fns,
                string_data: &mut self.string_data,
                vtable_data: &self.vtable_data,
                string_counter: &mut self.string_counter,
                user_fn_param_tys: &self.user_fn_param_tys,
            };

            // Map MIR blocks → Cranelift blocks
            let mut block_map: Vec<cranelift_codegen::ir::Block> =
                Vec::with_capacity(func.blocks.len());
            for _ in &func.blocks {
                block_map.push(builder.create_block());
            }

            // Declare Cranelift Variables for all locals
            let mut var_map: HashMap<LocalId, Variable> = HashMap::new();
            for local in &func.locals {
                let cl_ty = ty_to_cranelift(&local.ty).unwrap_or(types::I64);
                let var = builder.declare_var(cl_ty);
                var_map.insert(local.id, var);
            }

            // Pre-scan: allocate a stack slot for each String-typed local whose
            // address is taken via `&mut src`. The pointer we hand out to the
            // callee must remain valid and observers must see buffer
            // reallocations written through it. See cranelift.rs for details.
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
            let mut stack_slots: HashMap<LocalId, StackSlot> = HashMap::new();
            for &local_id in &address_taken {
                let slot = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                stack_slots.insert(local_id, slot);
            }

            // Set up entry block
            let entry_cl_block = block_map[func.entry_block];
            builder.switch_to_block(entry_cl_block);
            builder.append_block_params_for_function_params(entry_cl_block);

            // Bind function parameters to their local variables
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

            // Translate each block
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
                            "Error in function '{}', block {}: {}",
                            func.name, mir_idx, e
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

            builder.seal_all_blocks();
            builder.finalize();
        }

        let define_result = self.module.define_function(func_id, &mut self.ctx);
        // Always clear the shared context so a failed compilation doesn't
        // leak IR into the next one (without this, a second REPL input
        // sees the prior input's instructions and the verifier complains
        // about stale blocks).
        self.module.clear_context(&mut self.ctx);
        define_result.map_err(|e| format!("Failed to define function '{}': {:?}", func.name, e))?;

        Ok(())
    }
}

/// Write `val` into the storage for `local_id`.
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

// ── Runtime symbol registration ────────────────────────────────────

fn register_runtime_symbols(builder: &mut JITBuilder) {
    macro_rules! reg {
        ($builder:expr, $name:ident) => {
            $builder.symbol(stringify!($name), $name as *const u8);
        };
    }

    // Print family → capture shims so the REPL can diff cumulative stdout
    // between inputs and surface only the newest input's output.
    builder.symbol(
        "ruxen_puts",
        crate::capture::ruxen_repl_puts_shim as *const u8,
    );
    builder.symbol(
        "ruxen_print",
        crate::capture::ruxen_repl_print_shim as *const u8,
    );
    builder.symbol(
        "ruxen_eputs",
        crate::capture::ruxen_repl_eputs_shim as *const u8,
    );
    builder.symbol(
        "ruxen_print_int",
        crate::capture::ruxen_repl_print_int_shim as *const u8,
    );
    builder.symbol(
        "ruxen_print_float",
        crate::capture::ruxen_repl_print_float_shim as *const u8,
    );
    reg!(builder, ruxen_int_to_string);
    reg!(builder, ruxen_float_to_string);
    reg!(builder, ruxen_bool_to_string);
    reg!(builder, ruxen_char_to_string);
    reg!(builder, ruxen_string_concat);
    reg!(builder, ruxen_string_from);
    reg!(builder, ruxen_string_push_str);
    reg!(builder, ruxen_string_len);
    reg!(builder, ruxen_string_is_empty);
    reg!(builder, ruxen_string_trim);
    reg!(builder, ruxen_string_to_lower);
    reg!(builder, ruxen_string_to_upper);
    reg!(builder, ruxen_string_chars);
    reg!(builder, ruxen_string_eq);
    reg!(builder, ruxen_string_cmp);
    reg!(builder, ruxen_str_split);
    reg!(builder, ruxen_str_parse_uint);
    reg!(builder, ruxen_deref_ptr);
    reg!(builder, ruxen_store_ptr);
    reg!(builder, ruxen_alloc);
    reg!(builder, ruxen_dealloc);
    reg!(builder, ruxen_realloc);
    reg!(builder, ruxen_panic);
    reg!(builder, ruxen_vec_new);
    reg!(builder, ruxen_vec_push);
    reg!(builder, ruxen_vec_pop);
    reg!(builder, ruxen_vec_len);
    reg!(builder, ruxen_vec_get);
    reg!(builder, ruxen_vec_get_opt);
    reg!(builder, ruxen_vec_get_mut);
    reg!(builder, ruxen_vec_get_mut_opt);
    reg!(builder, ruxen_vec_is_empty);
    reg!(builder, ruxen_vec_each);
    reg!(builder, ruxen_hash_new);
    reg!(builder, ruxen_hash_insert);
    reg!(builder, ruxen_hash_get);
    reg!(builder, ruxen_hash_contains_key);
    reg!(builder, ruxen_hash_len);
    reg!(builder, ruxen_hash_is_empty);
    reg!(builder, ruxen_set_new);
    reg!(builder, ruxen_set_insert);
    reg!(builder, ruxen_set_contains);
    reg!(builder, ruxen_set_len);
    reg!(builder, ruxen_set_is_empty);
    reg!(builder, ruxen_option_unwrap_or);
    reg!(builder, ruxen_option_expect);
    reg!(builder, ruxen_option_unwrap);
    reg!(builder, ruxen_option_is_some);
    reg!(builder, ruxen_option_is_none);
    reg!(builder, ruxen_result_unwrap_or_else);
    reg!(builder, ruxen_result_try_op);
    reg!(builder, ruxen_result_expect);
    reg!(builder, ruxen_result_unwrap);
    reg!(builder, ruxen_result_is_ok);
    reg!(builder, ruxen_result_is_err);
    reg!(builder, ruxen_result_ok);
    reg!(builder, ruxen_result_err);
    reg!(builder, ruxen_noop_passthrough);
    reg!(builder, ruxen_noop_return_null);
    reg!(builder, ruxen_noop);
    // Formatter family — ruby-naming.spec.md §3.6 / #06.D routes
    // primitive interpolations through these symbols.
    reg!(builder, ruxen_fmt_formatter_new);
    reg!(builder, ruxen_fmt_formatter_new_with_spec);
    reg!(builder, ruxen_fmt_formatter_write_str);
    reg!(builder, ruxen_fmt_formatter_write_char);
    reg!(builder, ruxen_fmt_formatter_buffer);
    reg!(builder, ruxen_fmt_formatter_len);
    reg!(builder, ruxen_fmt_formatter_free);
    reg!(builder, ruxen_fmt_formatter_precision);
}

// ── JIT Translation Environment ────────────────────────────────────

struct JITTranslationEnv<'a> {
    module: &'a mut JITModule,
    declared_fns: &'a mut HashMap<String, FuncId>,
    string_data: &'a mut HashMap<String, cranelift_module::DataId>,
    vtable_data: &'a HashMap<String, DataId>,
    string_counter: &'a mut u32,
    user_fn_param_tys: &'a HashMap<String, Vec<Type>>,
}

impl<'a> JITTranslationEnv<'a> {
    fn create_string_data(&mut self, value: &str) -> Result<cranelift_module::DataId, String> {
        if let Some(&data_id) = self.string_data.get(value) {
            return Ok(data_id);
        }

        let name = format!(".str.{}", *self.string_counter);
        *self.string_counter += 1;

        let data_id = self
            .module
            .declare_data(&name, Linkage::Local, false, false)
            .map_err(|e| format!("Failed to declare string data: {}", e))?;

        let mut desc = DataDescription::new();
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        desc.define(bytes.into_boxed_slice());

        self.module
            .define_data(data_id, &desc)
            .map_err(|e| format!("Failed to define string data: {}", e))?;

        self.string_data.insert(value.to_string(), data_id);
        Ok(data_id)
    }

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

        // Try inferred-type method resolution (same as batch codegen)
        if name.starts_with("?") {
            let method = extract_method_name(name);
            let suffix = format!("_{}", method);
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

        // Generic type param resolution
        if !self.declared_fns.contains_key(name) {
            let method = extract_method_name(name);
            let type_prefix = if let Some(pos) = name.find('_') {
                &name[..pos]
            } else {
                ""
            };
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

        // Compiler-internal surface names such as `Formatter_new` lower to
        // C runtime symbols in the batch backend. Mirror that mapping before
        // declaring an import so the JIT resolves the linked runtime symbol,
        // not the synthetic source-level name.
        if let Some(mapped) = jit_runtime_name(name) {
            if let Some((param_tys, ret_ty)) = runtime_signature(mapped) {
                return self.declare_runtime_func(mapped, &param_tys, ret_ty, builder);
            }
        }
        if let Ok(mapped) = runtime_name(name) {
            if mapped != name {
                if let Some((param_tys, ret_ty)) = runtime_signature(mapped) {
                    return self.declare_runtime_func(mapped, &param_tys, ret_ty, builder);
                }
            }
        }

        // Try known runtime signatures
        if let Some((param_tys, ret_ty)) = runtime_signature(name) {
            return self.declare_runtime_func(name, &param_tys, ret_ty, builder);
        }

        // Fall back: infer signature from call-site
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
            .map_err(|e| format!("Failed to declare function '{}': {}", name, e))?;

        self.declared_fns.insert(name.to_string(), func_id);
        let func_ref = self.module.declare_func_in_func(func_id, builder.func);
        Ok(func_ref)
    }

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

// ── Signature building ─────────────────────────────────────────────

fn build_jit_signature(module: &JITModule, func: &MirFunction) -> Signature {
    let call_conv = module.isa().default_call_conv();
    let mut sig = Signature::new(call_conv);

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

// ── Instruction translation (mirrors batch codegen) ────────────────

fn translate_instruction(
    inst: &MirInst,
    func: &MirFunction,
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    _block_map: &[cranelift_codegen::ir::Block],
    builder: &mut FunctionBuilder,
    env: &mut JITTranslationEnv,
) -> Result<(), String> {
    match inst {
        MirInst::Assign { dest, value } => {
            let val = gen_value(value, func, var_map, stack_slots, builder)?;
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

            let is_string_concat = matches!(op, BinOp::Add)
                && (is_string_typed_value(lhs, func) || is_string_typed_value(rhs, func));

            let result = if is_string_concat {
                let func_ref = env.declare_runtime_func(
                    "ruxen_string_concat",
                    &[types::I64, types::I64],
                    Some(types::I64),
                    builder,
                )?;
                let call = builder.ins().call(func_ref, &[l, r]);
                builder.inst_results(call)[0]
            } else {
                let common_ty = builder.func.dfg.value_type(l);
                let r = coerce_value(r, common_ty, builder);
                emit_binop(*op, l, r, builder)
            };

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

            let is_string_compare =
                is_string_typed_value(lhs, func) || is_string_typed_value(rhs, func);

            let result = if is_string_compare && matches!(op, CmpOp::Eq | CmpOp::NotEq) {
                let func_ref = env.declare_runtime_func(
                    "ruxen_string_eq",
                    &[types::I64, types::I64],
                    Some(types::I64),
                    builder,
                )?;
                let call = builder.ins().call(func_ref, &[l, r]);
                let eq_result = builder.inst_results(call)[0];
                if matches!(op, CmpOp::NotEq) {
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.ins().icmp(IntCC::Equal, eq_result, zero)
                } else {
                    builder.ins().ireduce(types::I8, eq_result)
                }
            } else if is_string_compare {
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

            // Widen narrow-integer args to match callee parameter types.
            coerce_call_args(
                &mut arg_vals,
                args,
                func,
                actual_name,
                env.user_fn_param_tys,
                builder,
            );

            match actual_name {
                "ruxen_noop_passthrough" => {
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
                }
                "ruxen_noop_return_null" => {
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
            let val = use_local(var_map, stack_slots, builder, *src);
            def_local(var_map, stack_slots, builder, *dest, val);
        }

        MirInst::RefMut { dest, src } => {
            // Address-taken Strings live in stack slots — take the slot address
            // so the callee can write through the pointer and the caller sees
            // the change on re-read.
            if let Some(&slot) = stack_slots.get(src) {
                let addr = builder.ins().stack_addr(types::I64, slot, 0);
                def_local(var_map, stack_slots, builder, *dest, addr);
            } else {
                let val = use_local(var_map, stack_slots, builder, *src);
                def_local(var_map, stack_slots, builder, *dest, val);
            }
        }

        MirInst::Copy { dest, src } | MirInst::Move { dest, src } => {
            let val = use_local(var_map, stack_slots, builder, *src);
            def_local(var_map, stack_slots, builder, *dest, val);
        }

        MirInst::Drop { local: _ } => {
            // No-op (same as batch codegen).
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
            // Closure call: callees expect i64 args.
            for (i, arg_val) in arg_vals.iter_mut().enumerate() {
                let val_ty = builder.func.dfg.value_type(*arg_val);
                if val_ty.is_int() && val_ty.bits() < 64 {
                    let signed = mir_arg_is_signed(&args[i], func);
                    *arg_val = coerce_value_signed(*arg_val, types::I64, signed, builder);
                }
            }

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

        MirInst::DataAddr { dest, data_sym } => {
            let data_id = *env.vtable_data.get(data_sym).ok_or_else(|| {
                format!(
                    "mixin-vtables: DataAddr for unknown data symbol '{}'",
                    data_sym
                )
            })?;
            let gv = env.module.declare_data_in_func(data_id, builder.func);
            let ptr = builder.ins().symbol_value(types::I64, gv);
            def_local(var_map, stack_slots, builder, *dest, ptr);
        }
    }

    Ok(())
}

fn translate_terminator(
    term: &Terminator,
    func: &MirFunction,
    var_map: &HashMap<LocalId, Variable>,
    stack_slots: &HashMap<LocalId, StackSlot>,
    block_map: &[cranelift_codegen::ir::Block],
    builder: &mut FunctionBuilder,
    _env: &mut JITTranslationEnv,
) -> Result<(), String> {
    match term {
        Terminator::Return(val) => match val {
            Some(v) => {
                let ret_val = gen_value(v, func, var_map, stack_slots, builder)?;
                if let Some(ret_ty) = ty_to_cranelift(&func.return_ty) {
                    let ret_val = coerce_value(ret_val, ret_ty, builder);
                    builder.ins().return_(&[ret_val]);
                } else {
                    builder.ins().return_(&[]);
                }
            }
            None => {
                builder.ins().return_(&[]);
            }
        },

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

// ── Helpers (mirrored from batch codegen) ──────────────────────────

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

/// Widen narrow-integer call arguments to match the callee's expected
/// parameter types. Mirrors cranelift.rs. Runtime helpers expect i64
/// args; Cranelift's verifier rejects narrower ints at the call boundary.
fn coerce_call_args(
    arg_vals: &mut [cranelift_codegen::ir::Value],
    args: &[MirValue],
    func: &MirFunction,
    callee: &str,
    user_fn_param_tys: &HashMap<String, Vec<Type>>,
    builder: &mut FunctionBuilder,
) {
    let known_sig = runtime_signature(callee);
    let user_sig = user_fn_param_tys.get(callee);
    for (i, arg_val) in arg_vals.iter_mut().enumerate() {
        let val_ty = builder.func.dfg.value_type(*arg_val);

        let target_ty = match &known_sig {
            Some((params, _)) if i < params.len() => params[i],
            _ => match user_sig {
                // ruby-naming.spec.md §3.6: synthesized `Bool_fmt` /
                // `Char_fmt` / etc. legitimately declare narrow-int
                // params (i8 for Bool, i32 for Char). Honour the
                // declared param type instead of blindly widening to
                // i64 — otherwise Cranelift's verifier rejects the
                // call. Mirrors `user_fn_param_tys` in the batch
                // codegen.
                Some(params) if i < params.len() => params[i],
                _ => {
                    if val_ty.is_int() && val_ty.bits() < 64 {
                        types::I64
                    } else {
                        val_ty
                    }
                }
            },
        };

        if val_ty == target_ty {
            continue;
        }

        let signed = mir_arg_is_signed(&args[i], func);
        *arg_val = coerce_value_signed(*arg_val, target_ty, signed, builder);
    }
}

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
            // emit these — keep the integer form to preserve batch-codegen
            // parity.
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

fn coerce_value(
    val: cranelift_codegen::ir::Value,
    target_ty: Type,
    builder: &mut FunctionBuilder,
) -> cranelift_codegen::ir::Value {
    coerce_value_signed(val, target_ty, false, builder)
}

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
    if val_ty.is_int() && target_ty.is_int() {
        if val_ty.bits() > target_ty.bits() {
            return builder.ins().ireduce(target_ty, val);
        } else if signed {
            return builder.ins().sextend(target_ty, val);
        } else {
            return builder.ins().uextend(target_ty, val);
        }
    }
    if val_ty.is_float() && target_ty.is_float() {
        if val_ty.bits() > target_ty.bits() {
            return builder.ins().fdemote(target_ty, val);
        } else {
            return builder.ins().fpromote(target_ty, val);
        }
    }
    val
}

fn ty_to_cranelift(ty: &Ty) -> Option<Type> {
    match ty {
        Ty::Bool => Some(types::I8),
        Ty::Int8 | Ty::UInt8 => Some(types::I8),
        Ty::Int16 | Ty::UInt16 => Some(types::I16),
        Ty::Int32 | Ty::UInt32 | Ty::Char => Some(types::I32),
        Ty::Int | Ty::Int64 | Ty::UInt | Ty::UInt64 | Ty::ISize | Ty::USize => Some(types::I64),
        Ty::Float32 => Some(types::F32),
        Ty::Float | Ty::Float64 => Some(types::F64),
        Ty::String
        | Ty::Str
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
        | Ty::TypeParam { .. }
        | Ty::Infer(_)
        | Ty::Tuple(_)
        | Ty::FixedArray(_, _) => Some(types::I64),
        Ty::Unit | Ty::Never => None,
        Ty::Error => None,
        Ty::ConstArg(_) => None,
    }
}

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

fn is_string_typed_value(val: &MirValue, func: &MirFunction) -> bool {
    if let MirValue::Use(local_id) = val {
        if let Some(local) = func.locals.get(*local_id as usize) {
            return is_string_mir_ty(&local.ty);
        }
    }
    false
}

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
        Ty::Class { .. } | Ty::Struct { .. } => 64,
        Ty::Option(_) => 16,
        Ty::Result(_, _) => 16,
        Ty::Tuple(elems) => elems.len().max(1) * 8,
        Ty::FixedArray(_, n) => n.as_lit().unwrap_or(0) as usize * 8,
        _ => 8,
    }
}

fn runtime_signature(name: &str) -> Option<(Vec<Type>, Option<Type>)> {
    // Consult the compiler's authoritative ABI table first so this JIT
    // backend stays in lockstep with the batch backend for every
    // `ruxen_*` helper (208 entries and counting). The local table below
    // only needs to cover REPL-only aliases the core table omits (the
    // bare `puts`/`print`/etc. surface names). Without this delegation a
    // symbol missing from the local table (e.g. `ruxen_string_push`)
    // silently derived its width from the HIR `Char`→i32, and the JIT
    // verifier rejected the i64 call arg.
    if let Some(sig) = ruxen_core::codegen::cranelift::runtime_signature(name) {
        return Some(sig);
    }
    match name {
        "puts" | "ruxen_puts" => Some((vec![types::I64], None)),
        "eputs" | "ruxen_eputs" => Some((vec![types::I64], None)),
        "print" | "ruxen_print" => Some((vec![types::I64], None)),
        "ruxen_print_int" => Some((vec![types::I64], None)),
        "ruxen_int_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_float_to_string" => Some((vec![types::F64], Some(types::I64))),
        "ruxen_bool_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_char_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_concat" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_from" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_push_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_deref_ptr" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_store_ptr" => Some((vec![types::I64, types::I64], None)),
        "ruxen_string_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_is_empty" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_string_trim" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_to_lower" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_to_upper" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_chars" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_str_split" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_str_parse_uint" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_alloc" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_dealloc" => Some((vec![types::I64], None)),
        "ruxen_realloc" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_panic" => Some((vec![types::I64], None)),
        "ruxen_vec_new" => Some((vec![], Some(types::I64))),
        "ruxen_vec_push" => Some((vec![types::I64, types::I64], None)),
        "ruxen_vec_pop" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_vec_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_vec_get" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_get_opt" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_get_mut" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_get_mut_opt" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_is_empty" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_vec_each" => Some((vec![types::I64, types::I64], None)),
        "ruxen_hash_new" => Some((vec![], Some(types::I64))),
        "ruxen_hash_insert" => Some((vec![types::I64, types::I64, types::I64], None)),
        "ruxen_hash_get" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_hash_contains_key" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "ruxen_hash_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_is_empty" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_set_new" => Some((vec![], Some(types::I64))),
        "ruxen_set_insert" => Some((vec![types::I64, types::I64], None)),
        "ruxen_set_contains" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "ruxen_set_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_set_is_empty" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_option_unwrap_or" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_result_unwrap_or_else" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_result_try_op" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_result_expect" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_result_unwrap" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_option_expect" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_option_unwrap" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_result_ok" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_result_err" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_option_is_some" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_option_is_none" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_result_is_ok" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_result_is_err" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_noop" => Some((vec![], None)),
        "ruxen_fmt_formatter_new" => Some((vec![], Some(types::I64))),
        "ruxen_fmt_formatter_free" => Some((vec![types::I64], None)),
        "ruxen_fmt_formatter_write_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_write_char" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_buffer" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_precision" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_new_with_spec" => Some((
            vec![
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
                types::I64,
            ],
            Some(types::I64),
        )),
        _ => None,
    }
}

fn jit_runtime_name(name: &str) -> Option<&'static str> {
    match name {
        "Formatter_new" => Some("ruxen_fmt_formatter_new"),
        "Formatter_free" => Some("ruxen_fmt_formatter_free"),
        "Formatter_write_str" => Some("ruxen_fmt_formatter_write_str"),
        "Formatter_write_char" => Some("ruxen_fmt_formatter_write_char"),
        "Formatter_buffer" => Some("ruxen_fmt_formatter_buffer"),
        "Formatter_len" => Some("ruxen_fmt_formatter_len"),
        "Formatter_new_with_spec" => Some("ruxen_fmt_formatter_new_with_spec"),
        "Formatter_precision" => Some("ruxen_fmt_formatter_precision"),
        _ => None,
    }
}

/// Allowlist for the REPL's `dlsym(RTLD_DEFAULT)` symbol-lookup fallback.
///
/// Without this gate any `lib def foo as "system"` line in user input
/// resolves to `libc::system` and the REPL becomes an arbitrary-code
/// execution surface. We accept:
///
/// 1. The `ruxen_*` runtime helpers (every stdlib entry point goes
///    through this prefix).
/// 2. Class-method-mangled symbols matching `<Class>_<method>` —
///    same shape Ruxen's own codegen emits via
///    `synthesize_dynamic_dispatch_helpers` and the formatter / future
///    runtimes.
/// 3. A small list of explicit libc primitives the runtime itself
///    needs at link time (no exec / process-spawn surface here).
///
/// Anything else returns `None`, which Cranelift surfaces as a
/// "linker failed to find …" error — visible to the user and harmless.
pub(super) fn is_repl_symbol_allowed(name: &str) -> bool {
    // Ruxen runtime helpers — the canonical surface.
    if name.starts_with("ruxen_") {
        return true;
    }
    // Mangled class / mixin methods follow `<TitleCase>_<lower_snake>`.
    // Accept anything starting with an ASCII uppercase letter that's
    // a plausible Rust-style identifier (no shell metacharacters).
    if name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.contains('_')
    {
        return true;
    }
    // Specific libc primitives the runtime is allowed to look up by
    // name. Keep this list minimal — anything that can spawn a process,
    // execute a string, or open a socket is OFF.
    matches!(
        name,
        "memcpy"
            | "memmove"
            | "memset"
            | "memcmp"
            | "strlen"
            | "strcmp"
            | "strncmp"
            | "strcpy"
            | "strncpy"
            | "strchr"
            | "strrchr"
            | "malloc"
            | "calloc"
            | "realloc"
            | "free"
            | "abort"
            | "exit"
            | "puts"
            | "putchar"
            | "fputs"
            | "fputc"
            | "fwrite"
            | "fflush"
            // Math primitives the codegen may reference for f64 ops.
            | "fmod"
            | "fmodf"
            | "pow"
            | "powf"
            | "sqrt"
            | "sqrtf"
            | "floor"
            | "ceil"
            | "round"
    )
}

#[cfg(test)]
mod symbol_allowlist_tests {
    use super::is_repl_symbol_allowed;

    #[test]
    fn ruxen_runtime_symbols_pass() {
        assert!(is_repl_symbol_allowed("ruxen_string_from"));
        assert!(is_repl_symbol_allowed("ruxen_vec_new"));
        assert!(is_repl_symbol_allowed("ruxen_panic"));
    }

    #[test]
    fn mangled_class_methods_pass() {
        assert!(is_repl_symbol_allowed("Vec_push"));
        assert!(is_repl_symbol_allowed("Future_dynamic_poll"));
        assert!(is_repl_symbol_allowed("Formatter_write_str"));
    }

    #[test]
    fn allowed_libc_primitives_pass() {
        assert!(is_repl_symbol_allowed("memcpy"));
        assert!(is_repl_symbol_allowed("strlen"));
        assert!(is_repl_symbol_allowed("malloc"));
    }

    #[test]
    fn dangerous_libc_symbols_blocked() {
        // Process-spawn / exec surface — these MUST not be reachable
        // via dlsym fallback. A user-input `lib def system as "system"`
        // would otherwise JIT straight to arbitrary command execution.
        assert!(!is_repl_symbol_allowed("system"));
        assert!(!is_repl_symbol_allowed("execve"));
        assert!(!is_repl_symbol_allowed("execvp"));
        assert!(!is_repl_symbol_allowed("popen"));
        assert!(!is_repl_symbol_allowed("fork"));
        assert!(!is_repl_symbol_allowed("dlopen"));
        assert!(!is_repl_symbol_allowed("dlsym"));
    }

    #[test]
    fn lowercase_freefns_blocked() {
        // `lib def foo as "getenv"` style — lowercase identifiers don't
        // match the Ruxen mangling convention and aren't in the libc
        // allowlist, so they're refused.
        assert!(!is_repl_symbol_allowed("getenv"));
        assert!(!is_repl_symbol_allowed("setenv"));
        assert!(!is_repl_symbol_allowed("open"));
        assert!(!is_repl_symbol_allowed("read"));
        assert!(!is_repl_symbol_allowed("write"));
    }
}
