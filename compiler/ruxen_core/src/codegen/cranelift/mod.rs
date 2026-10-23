//! Cranelift code generation backend for the Ruxen compiler.
//!
//! Translates MIR programs into machine code through a Cranelift module:
//! `ObjectModule` for an object file, the REPL's `JITModule` for memory.
//! `CodeGen::compile_program` is the one driver for both:
//!   1. Declare FFI imports, functions and vtable data.
//!   2. For each new function, lower it through `codegen::lower`.
//!   3. Define the vtable data; then `finish` emits object bytes (batch) or
//!      the JIT finalizes its module.
//!
//! Every ABI rule (machine classes, signatures, callee lookup, coercion
//! decisions) is `mir::abi`'s; this module is split for navigability:
//!   * `helpers`      — the `AbiClass` ↔ Cranelift type map and `CmpOp` codes.
//!   * `translation_env` — module-state borrow split from `FunctionBuilder`.
//!   * `emit`         — `ClifBuilder`, this backend's `IrBuilder` for the one
//!     MIR walk (`codegen::lower`), and its `Coercion`s.

mod emit;
mod helpers;
mod translation_env;

use emit::{build_signature, ClifBuilder};
use helpers::clif_signature;
/// The `AbiClass` / `Ty` → Cranelift type map, pinned by
/// `tests/it/runtime_abi_derivation.rs`.
pub use helpers::{clif_type, ty_to_cranelift};
use translation_env::TranslationEnv;

use std::collections::{HashMap, HashSet};

use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};

use crate::codegen::lower::lower_function;
use crate::mir::abi::{CalleeIndex, CalleeKind};
use crate::mir::nodes::*;

/// Cranelift code generation engine, generic over the Cranelift module:
/// `ObjectModule` for batch builds (`new_for_target` + `finish`), the
/// REPL's `JITModule` (`with_module`). Both drive the same
/// [`CodeGen::compile_program`].
///
/// Holds the module, context objects, and bookkeeping for string data
/// sections, declared functions and mixin-vtable data.
pub struct CodeGen<M: Module> {
    module: M,
    ctx: Context,
    builder_ctx: FunctionBuilderContext,
    string_data: HashMap<String, DataId>,
    string_counter: u32,
    declared_fns: HashMap<String, FuncId>,
    /// Mixin vtables Phase B-5: data sections for `__rx_vtable_*` and
    /// `__rx_classinfo_*` symbols. Declared early (before function
    /// definitions) so MIR `DataAddr { data_sym }` lowering can take
    /// their address from inside function bodies. Defined with their
    /// function-pointer / data-pointer relocations after function
    /// definitions complete (`define_data`).
    vtable_data: HashMap<String, DataId>,
    /// The vtable / class_info symbols already defined, so a repeated
    /// `compile_program` (the REPL lowers the whole session per input)
    /// defines each once.
    defined_data: HashSet<String>,
}

/// The Cranelift ISA for `target` (the host when `None`), position
/// independent when `is_pic`: a linked object needs it, JIT code runs at
/// the absolute addresses it was emitted for.
///
/// `None` or a host target uses `cranelift_native::builder()`, which
/// detects the running CPU. A non-host target routes through
/// `cranelift_codegen::isa::lookup(triple)`, which picks the ISA and (via
/// the triple's binary format) the ELF-vs-Mach-O object encoding. The flags
/// are the same on both paths so a host triple passed explicitly is
/// byte-identical to the implicit-host path.
pub fn isa(
    target: Option<&crate::codegen::target::ResolvedTarget>,
    is_pic: bool,
) -> Result<OwnedTargetIsa, String> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", "none")
        .map_err(|e| format!("Failed to set opt_level: {}", e))?;
    flag_builder
        .set("is_pic", if is_pic { "true" } else { "false" })
        .map_err(|e| format!("Failed to set is_pic: {}", e))?;
    let flags = settings::Flags::new(flag_builder);

    match target {
        Some(t) if !t.is_host() => cranelift_codegen::isa::lookup(t.triple().clone())
            .map_err(|e| {
                format!(
                    "unsupported Cranelift target '{}': {}. \
                     wasm/embedded targets require the LLVM backend (--release / --backend=llvm).",
                    t.canonical(),
                    e
                )
            })?
            .finish(flags)
            .map_err(|e| format!("Failed to finish ISA for '{}': {}", t.canonical(), e)),
        _ => cranelift_native::builder()
            .map_err(|e| format!("Failed to create native ISA builder: {}", e))?
            .finish(flags)
            .map_err(|e| format!("Failed to finish ISA: {}", e)),
    }
}

impl CodeGen<ObjectModule> {
    /// Create a new code generator targeting the host machine.
    ///
    /// Back-compat shim: equivalent to `new_for_target(None)`.
    pub fn new() -> Result<Self, String> {
        Self::new_for_target(None)
    }

    /// Create an object code generator for an optional cross-compilation
    /// target (tier 4.02); see [`isa`].
    pub fn new_for_target(
        target: Option<&crate::codegen::target::ResolvedTarget>,
    ) -> Result<Self, String> {
        let obj_builder = ObjectBuilder::new(
            isa(target, true)?,
            "ruxen_module",
            cranelift_module::default_libcall_names(),
        )
        .map_err(|e| format!("Failed to create ObjectBuilder: {}", e))?;
        Ok(Self::with_module(ObjectModule::new(obj_builder)))
    }

    /// Emit the finished object file as raw bytes.
    pub fn finish(self) -> Result<Vec<u8>, String> {
        #[allow(unused_mut)]
        let mut product = self.module.finish();
        // macOS: stamp an `LC_BUILD_VERSION` load command on the emitted
        // Mach-O object. Cranelift/`object` omit it by default, so Apple's
        // linker prints `warning: no platform load command found in
        // '<obj>', assuming: macOS` for every object on every link. Adding
        // the command (platform=macOS, a conservative 11.0 min/SDK) makes
        // the platform explicit and silences the warning. Native builds
        // target the host, so gating on the macOS host is correct; the
        // setter is a no-op for non-Mach-O output anyway.
        #[cfg(target_os = "macos")]
        {
            use cranelift_object::object::macho::PLATFORM_MACOS;
            use cranelift_object::object::write::MachOBuildVersion;
            let mut build_version = MachOBuildVersion::default();
            build_version.platform = PLATFORM_MACOS;
            build_version.minos = 0x000B_0000; // 11.0.0
            build_version.sdk = 0x000B_0000; // 11.0.0
            product.object.set_macho_build_version(build_version);
        }
        let bytes = product
            .emit()
            .map_err(|e| format!("Failed to emit object: {}", e))?;
        Ok(bytes)
    }
}

impl<M: Module> CodeGen<M> {
    /// A code generator emitting into `module`. The module's construction
    /// holds everything module-specific (its ISA flags, a JIT's symbol
    /// lookup); the lowering below is the same for every module.
    pub fn with_module(module: M) -> Self {
        let ctx = module.make_context();
        CodeGen {
            module,
            ctx,
            builder_ctx: FunctionBuilderContext::new(),
            string_data: HashMap::new(),
            string_counter: 0,
            declared_fns: HashMap::new(),
            vtable_data: HashMap::new(),
            defined_data: HashSet::new(),
        }
    }

    /// The module, e.g. for a JIT to finalize and look up code.
    pub fn module(&self) -> &M {
        &self.module
    }

    /// The module, mutably.
    pub fn module_mut(&mut self) -> &mut M {
        &mut self.module
    }

    /// The id a function (or import) was declared under.
    pub fn func_id(&self, name: &str) -> Option<FuncId> {
        self.declared_fns.get(name).copied()
    }

    /// Compile every function and data symbol of `program` this code
    /// generator has not compiled yet: declare (FFI imports, functions,
    /// vtable data), then define the new functions, then the new data.
    ///
    /// Incremental: a later call compiles only what earlier calls did not,
    /// which is how the REPL compiles each input's re-lowered session. A
    /// failed call leaves no function declared without a body, so a later
    /// call declares and defines it again.
    pub fn compile_program(&mut self, program: &MirProgram) -> Result<(), String> {
        let new_fns = self.declare_program(program)?;
        for (i, func) in new_fns.iter().enumerate() {
            if let Err(e) = self.define_function(func, &program.callees) {
                for undefined in &new_fns[i..] {
                    self.declared_fns.remove(&undefined.name);
                }
                return Err(e);
            }
        }
        // Spec: docs/specs/types/mixin_vtables.spec.md §B2-B3. Every
        // method symbol is declared above; one missing from `declared_fns`
        // is an upstream E1117 escape, a hard codegen error.
        self.define_data(program)
    }

    /// Declare what `program` adds: its FFI imports, its functions not
    /// declared for definition by an earlier call (returned, in order), and
    /// its vtable / class_info data symbols, which must exist before
    /// function bodies take their address (`MirInst::DataAddr`).
    fn declare_program<'p>(
        &mut self,
        program: &'p MirProgram,
    ) -> Result<Vec<&'p MirFunction>, String> {
        // Every callee's signature, built by the last lowering step.
        let callees = &program.callees;
        let call_conv = self.module.isa().default_call_conv();

        // Each `lib`/`extern` symbol once, under its C name, with the
        // signature derived from its declared types. A call by its Ruxen
        // name or lib-qualified alias resolves through the callee index.
        // Compiler-emitted runtime symbols are declared when first called.
        for lib in &program.ffi_libs {
            for ffi_fn in &lib.functions {
                if self.declared_fns.contains_key(&ffi_fn.name) {
                    continue;
                }
                let callee = callees
                    .get(&ffi_fn.name)
                    .expect("the callee index holds every FFI symbol");
                let sig = clif_signature(call_conv, &callee.abi);
                let func_id = self
                    .module
                    .declare_function(&ffi_fn.name, Linkage::Import, &sig)
                    .map_err(|e| {
                        format!("Failed to declare FFI function '{}': {}", ffi_fn.name, e)
                    })?;
                self.declared_fns.insert(ffi_fn.name.clone(), func_id);
            }
        }

        let new_fns: Vec<&MirFunction> = program
            .functions
            .iter()
            .filter(|f| !self.is_declared_for_definition(&f.name))
            .collect();
        for func in &new_fns {
            let callee = callees
                .get(&func.name)
                .filter(|c| c.kind == CalleeKind::Defined)
                .expect("the callee index holds every MIR function");
            let sig = clif_signature(call_conv, &callee.abi);
            // Lowering marks `main` and the `<Mixin>_dynamic_<method>`
            // helpers C runtime code calls by symbol (e.g. the scheduler's
            // `Future_dynamic_poll`) as exported.
            let linkage = match func.linkage {
                MirLinkage::Export => Linkage::Export,
                MirLinkage::Local => Linkage::Local,
            };
            let func_id = self
                .module
                .declare_function(&func.name, linkage, &sig)
                .map_err(|e| format!("Failed to declare function '{}': {}", func.name, e))?;
            self.declared_fns.insert(func.name.clone(), func_id);
        }

        let data_syms = program
            .vtables
            .iter()
            .map(|vt| ("vtable", vt.symbol()))
            .chain(
                program
                    .class_infos
                    .iter()
                    .map(|ci| ("class_info", ci.symbol())),
            );
        for (what, sym) in data_syms {
            if self.vtable_data.contains_key(&sym) {
                continue;
            }
            let data_id = self
                .module
                .declare_data(
                    &sym,
                    Linkage::Local,
                    /*writable*/ false,
                    /*tls*/ false,
                )
                .map_err(|e| format!("declare {} data '{}': {}", what, sym, e))?;
            self.vtable_data.insert(sym, data_id);
        }
        Ok(new_fns)
    }

    /// Whether an earlier call declared `name` as a function this module
    /// defines (an import of the same name does not count).
    fn is_declared_for_definition(&self, name: &str) -> bool {
        self.declared_fns.get(name).is_some_and(|&id| {
            self.module
                .declarations()
                .get_function_decl(id)
                .linkage
                .is_definable()
        })
    }

    /// Define the relocations of every declared vtable and class_info
    /// symbol not defined yet. Spec §B2-B3.
    fn define_data(&mut self, program: &MirProgram) -> Result<(), String> {
        const PTR_SIZE: u32 = 8;

        // Phase B-2: vtable function-pointer relocations.
        for vt in &program.vtables {
            let sym = vt.symbol();
            if self.defined_data.contains(&sym) {
                continue;
            }
            let data_id = *self
                .vtable_data
                .get(&sym)
                .ok_or_else(|| format!("vtable data '{}' was not declared", sym))?;
            let mut desc = pointer_table(vt.method_symbols.len() as u32 * PTR_SIZE);
            for (i, method_sym) in vt.method_symbols.iter().enumerate() {
                let func_id = *self.declared_fns.get(method_sym).ok_or_else(|| {
                    format!(
                        "mixin-vtables: method symbol '{}' for vtable '{}' not declared \
                         — required-method check (E1117) should have caught this",
                        method_sym, sym
                    )
                })?;
                let func_ref = self.module.declare_func_in_data(func_id, &mut desc);
                desc.write_function_addr((i as u32) * PTR_SIZE, func_ref);
            }
            self.module
                .define_data(data_id, &desc)
                .map_err(|e| format!("define vtable data '{}': {}", sym, e))?;
            self.defined_data.insert(sym);
        }

        // Phase B-3: class_info data-pointer relocations (pointing at
        // the vtables above).
        for ci in &program.class_infos {
            let sym = ci.symbol();
            if self.defined_data.contains(&sym) {
                continue;
            }
            let data_id = *self
                .vtable_data
                .get(&sym)
                .ok_or_else(|| format!("class_info data '{}' was not declared", sym))?;
            let mut desc = pointer_table(ci.vtable_symbols.len() as u32 * PTR_SIZE);
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
            self.defined_data.insert(sym);
        }
        Ok(())
    }

    /// Translate one declared MIR function into Cranelift IR and define it.
    /// The contexts are cleared either way, so a failure leaves nothing
    /// behind for the next function.
    fn define_function(&mut self, func: &MirFunction, callees: &CalleeIndex) -> Result<(), String> {
        let result = self.translate_into_ctx(func, callees).and_then(|()| {
            let func_id = *self
                .declared_fns
                .get(&func.name)
                .ok_or_else(|| format!("Function '{}' was not declared", func.name))?;
            self.module
                .define_function(func_id, &mut self.ctx)
                .map_err(|e| format!("Failed to define function '{}': {:?}", func.name, e))
        });
        self.module.clear_context(&mut self.ctx);
        if result.is_err() {
            // A translation that stopped midway never finalized its builder.
            self.builder_ctx = FunctionBuilderContext::new();
        }
        result
    }

    /// Lower a single MIR function into `self.ctx.func` (the Cranelift IR
    /// function) through the one MIR walk (`codegen::lower`), leaving it
    /// built-but-not-yet-defined. Shared by `define_function` (which then
    /// defines + clears the context) and by the `clif_for_test` hook (which
    /// reads back the CLIF text).
    fn translate_into_ctx(
        &mut self,
        func: &MirFunction,
        callees: &CalleeIndex,
    ) -> Result<(), String> {
        self.ctx.func.signature = build_signature(&self.module, func);
        // The `FunctionBuilder` borrows the function and its context; the
        // module state calls and data need is borrowed beside it.
        let env = TranslationEnv {
            module: &mut self.module,
            declared_fns: &mut self.declared_fns,
            string_data: &mut self.string_data,
            string_counter: &mut self.string_counter,
            callees,
            vtable_data: &self.vtable_data,
        };
        let mut builder = ClifBuilder::new(
            FunctionBuilder::new(&mut self.ctx.func, &mut self.builder_ctx),
            env,
        );
        lower_function(func, &mut builder)?;
        // Seal after every block is emitted, so forward edges (a `Switch`)
        // are registered first.
        builder.builder.seal_all_blocks();
        builder.builder.finalize();
        Ok(())
    }
}

/// A zeroed, pointer-aligned table of `size` bytes for a vtable or
/// class_info, its slots written as relocations. Explicit zero bytes, not
/// `define_zeroinit`: ld64 on macOS rejects relocations against BSS
/// (it segfaults at link time), so the data must land in `__const`.
fn pointer_table(size: u32) -> cranelift_module::DataDescription {
    let mut desc = cranelift_module::DataDescription::new();
    desc.define(vec![0u8; size as usize].into_boxed_slice());
    desc.set_align(8);
    desc
}

/// Test-only hook: declare `program` in a batch `CodeGen` (ObjectModule)
/// and return the Cranelift IR text of its function `name`. Used by
/// `ruxen_repl`'s `tests::cranelift_share_parity`. `#[doc(hidden)]` so it
/// is not part of the stable public API, but `pub` so the cross-crate test
/// can reach it. Drives the SAME declare and `translate_into_ctx` steps as
/// `compile_program` — it does not fork them.
#[doc(hidden)]
pub fn clif_for_test(program: &MirProgram, name: &str) -> Result<String, String> {
    let mut cg = CodeGen::new()?;
    cg.declare_program(program)?;
    let func = program
        .functions
        .iter()
        .find(|f| f.name == name)
        .ok_or_else(|| format!("no function '{name}' in the program"))?;
    cg.translate_into_ctx(func, &program.callees)?;
    Ok(cg.ctx.func.display().to_string())
}
