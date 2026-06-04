//! Cranelift JIT code generation for the Ruxen REPL.
//!
//! Wraps `JITModule` for in-process compilation and execution.
//! Separate from the batch `CodeGen` (which uses `ObjectModule`)
//! to avoid generification complexity.

use std::collections::{HashMap, HashSet};

use cranelift_codegen::ir::types::{self, Type};
use cranelift_codegen::ir::{
    AbiParam, Signature, StackSlot, StackSlotData, StackSlotKind,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId, Linkage, Module};

// The shared Cranelift core — formerly forked into this file (~1,100 lines).
// jit.rs is now a JIT *adapter*: JITModule construction, the capture-shim
// register_runtime_symbols + is_repl_symbol_allowed, declare/define/finalize,
// and the program-data declarations. The MIR->CLIF lowering itself lives in
// ruxen_core::codegen::cranelift and is driven here via the generic
// TranslationEnv (M = JITModule).
use ruxen_core::codegen::cranelift::{
    build_signature, def_local, is_string_mir_ty, translate_instruction, translate_terminator,
    ty_to_cranelift, TranslationEnv,
};
use ruxen_core::mir::nodes::*;

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
        let sig = build_signature(&self.module, mir_function);
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
        let sig = build_signature(&self.module, mir_function);
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
                    // Derive the signature from the HIR param/return types,
                    // but WIDEN narrow integers (i8/i16/i32 — Bool, Char,
                    // small ints) to i64. The batch backend keeps the narrow
                    // widths and coerces each call argument to match
                    // (`coerce_call_args`); the REPL's call path does NOT
                    // coerce — it materialises every scalar as i64. Declaring
                    // the FFI param as the narrow type would then fail the
                    // verifier ("arg 0 has type i64, expected i8" for a Bool,
                    // "expected i32" for a Char). Widening the declared param
                    // to i64 matches the REPL's call convention exactly. f64
                    // and pointer (i64) types pass through unchanged.
                    for param_ty in &ffi_fn.param_types {
                        if let Some(cl_ty) = ty_to_cranelift(param_ty) {
                            sig.params.push(AbiParam::new(widen_scalar_to_word(cl_ty)));
                        }
                    }
                    if let Some(ref ret_ty) = ffi_fn.return_type {
                        if let Some(cl_ty) = ty_to_cranelift(ret_ty) {
                            sig.returns.push(AbiParam::new(widen_scalar_to_word(cl_ty)));
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
        self.translate_into_ctx(func)?;

        let define_result = self.module.define_function(func_id, &mut self.ctx);
        // Always clear the shared context so a failed compilation doesn't
        // leak IR into the next one (without this, a second REPL input
        // sees the prior input's instructions and the verifier complains
        // about stale blocks).
        self.module.clear_context(&mut self.ctx);
        define_result.map_err(|e| format!("Failed to define function '{}': {:?}", func.name, e))?;

        Ok(())
    }

    /// Build `func`'s Cranelift IR into `self.ctx.func`, leaving it populated
    /// (NOT defined into the module, NOT cleared). Callers either define +
    /// clear (`compile_function_inner`) or read back the CLIF (`clif_for_test`).
    /// Mirrors the batch backend's `CodeGen::translate_into_ctx`.
    fn translate_into_ctx(&mut self, func: &MirFunction) -> Result<(), String> {
        let sig = build_signature(&self.module, func);
        self.ctx.func.signature = sig;

        {
            let mut builder = FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_ctx);

            // Shared borrow-split env (M = JITModule inferred). Same field set
            // as the deleted JITTranslationEnv; the only former difference (the
            // concrete module type) is now absorbed by the generic M.
            let mut env = TranslationEnv {
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

        Ok(())
    }
}

// `def_local` / `use_local` are now imported from the shared Cranelift core
// (`ruxen_core::codegen::cranelift`) — see the deleted-fork note at the top of
// this file. The forked copies lived here.

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
    // Session-variable slot helpers — invoked by the synthetic
    // prefix/suffix injection in `eval::build_program` (Phase 1
    // Approach A). Keep them adjacent to the puts/print shims so
    // future maintainers see all REPL-only runtime overrides in
    // one place.
    builder.symbol(
        "ruxen_repl_slot_load_i64",
        crate::slots::ruxen_repl_slot_load_i64 as *const u8,
    );
    builder.symbol(
        "ruxen_repl_slot_store_i64",
        crate::slots::ruxen_repl_slot_store_i64 as *const u8,
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

/// Widen a narrow integer Cranelift type (i8/i16/i32) to the i64 machine
/// word. The REPL materialises every scalar value (Bool, Char, small
/// ints) as i64 and does not coerce call arguments down to a callee's
/// narrow parameter type the way the batch backend does, so FFI/runtime
/// signatures the REPL declares must use i64 for those slots or the
/// Cranelift verifier rejects the call. f64 and i64 pass through.
fn widen_scalar_to_word(ty: Type) -> Type {
    match ty {
        types::I8 | types::I16 | types::I32 => types::I64,
        other => other,
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

/// Test-only hook: JIT-compile a two-`Int`-argument MIR function, run it with
/// `args`, and return the `i64` result. Used by the `cranelift_share_pin`
/// both-backends parity test in `ruxen_core` to assert the REPL JIT backend
/// lowers integer arithmetic identically to the batch backend.
///
/// `#[doc(hidden)]` so it is not part of the stable API, but `pub` so the
/// cross-crate test can reach it. It drives the SAME `compile_repl_input`
/// path the live REPL uses — it does not fork it.
///
/// # Safety contract
/// `func` must have exactly two `Int`-typed parameters and an `Int` return,
/// matching the transmuted `extern "C" fn(i64, i64) -> i64` arity. The pin
/// fixture guarantees this.
#[doc(hidden)]
/// Test seam: compile `func` through the JIT's translation path into the
/// shared codegen context and return the resulting CLIF text WITHOUT
/// finalizing (no JIT memory, no execution).
///
/// Parallels the batch `ruxen_core::codegen::cranelift::clif_for_test`: same
/// declare → `compile_function_inner` (no finalize) → `ctx.func.display()`
/// shape. Used by the share-parity tripwire to compare the JIT's emitted IR
/// against the batch backend's structurally, not just behaviourally.
pub fn clif_for_test(func: &MirFunction) -> Result<String, String> {
    let mut jit = JITCodeGen::new()?;
    // Declare so build_signature / self-calls resolve, then build the IR into
    // ctx WITHOUT defining or clearing it, so we can read the CLIF back.
    jit.declare_function(func)?;
    jit.translate_into_ctx(func)?;
    Ok(jit.ctx.func.display().to_string())
}

pub fn run_int_fn_for_test(func: &MirFunction, args: &[i64]) -> Result<i64, String> {
    assert_eq!(
        args.len(),
        2,
        "run_int_fn_for_test is wired for the 2-arg integer pin fixture only"
    );
    let mut jit = JITCodeGen::new()?;
    let code_ptr = jit.compile_repl_input(func)?;
    if code_ptr.is_null() {
        return Err("JIT returned a null function pointer".to_string());
    }
    // SAFETY: `code_ptr` is the finalized entry point of `func`, which the
    // caller guarantees is `fn(Int, Int) -> Int`. The JITModule that owns the
    // executable memory (`jit`) is kept alive until after the call returns.
    let f: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(code_ptr) };
    let result = f(args[0], args[1]);
    // `jit` (and its JITModule-owned code memory) drops here, after the call.
    Ok(result)
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
