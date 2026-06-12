//! LLVM code generation backend for the Ruxen compiler.
//!
//! Translates MIR programs into native object code via LLVM/inkwell.
//! Feature-gated behind `--features llvm`.

use inkwell::context::Context;
use inkwell::targets::*;
use inkwell::OptimizationLevel;

use crate::mir::nodes::MirProgram;

mod debug;
pub mod emit;
pub mod optimize;
pub mod runtime_decl;
pub mod types;

/// LLVM code generation engine.
pub struct CodeGen {
    context: Context,
    opt_level: u8,
    /// Canonical target triple string, or `None` for the host. When set, the
    /// target machine is built for this triple instead of
    /// `TargetMachine::get_default_triple()` (tier 4.02). LLVM 18's
    /// `initialize_all` already registers every target, so this is a
    /// one-string change with no feature-gate impact.
    target_triple: Option<String>,
    object_bytes: Option<Vec<u8>>,
}

impl CodeGen {
    /// Create a new LLVM code generator targeting the host.
    pub fn new(opt_level: u8) -> Result<Self, String> {
        Self::new_for_target(opt_level, None)
    }

    /// Create a new LLVM code generator for an optional cross target
    /// (tier 4.02). `target` is the canonical triple string (e.g.
    /// `"aarch64-unknown-linux-gnu"`), or `None` for the host.
    pub fn new_for_target(opt_level: u8, target: Option<String>) -> Result<Self, String> {
        Ok(CodeGen {
            context: Context::create(),
            opt_level,
            target_triple: target,
            object_bytes: None,
        })
    }

    /// Compile all functions in a MIR program to LLVM IR, optimize, and
    /// emit object code.
    pub fn compile_program(&mut self, program: &MirProgram) -> Result<(), String> {
        let module = self.context.create_module("ruxen_module");

        // Initialize LLVM targets
        Target::initialize_all(&InitializationConfig::default());
        // Tier 4.02: honour an explicit cross target; default to host.
        let target_triple = match &self.target_triple {
            Some(t) => TargetTriple::create(t),
            None => TargetMachine::get_default_triple(),
        };
        module.set_triple(&target_triple);

        let target =
            Target::from_triple(&target_triple).map_err(|e| format!("Unknown target: {}", e))?;
        let target_machine = target
            .create_target_machine(
                &target_triple,
                "generic",
                "",
                match self.opt_level {
                    0 => OptimizationLevel::None,
                    1 => OptimizationLevel::Less,
                    3 => OptimizationLevel::Aggressive,
                    _ => OptimizationLevel::Default,
                },
                RelocMode::PIC,
                CodeModel::Default,
            )
            .ok_or("Failed to create target machine")?;

        // Compile MIR → LLVM IR. On a wasm32 target, the emitter sets the
        // `export_name` attribute on each `program.wasm_exports` entry so the
        // function is a host-callable wasm export (tier 4.03). The triple is
        // already resolved into `target_triple` for the cross path.
        let is_wasm = self
            .target_triple
            .as_deref()
            .is_some_and(|t| t.starts_with("wasm32") || t.starts_with("wasm64"));
        emit::compile_program(program, &module, &self.context, is_wasm)?;

        // TODO(mixin-vtables): emit `program.vtables` and
        // `program.class_infos` as LLVM global variables with
        // function-pointer / global-pointer initializers. The Cranelift
        // backend handles them in `cranelift::mod::emit_mixin_vtables`
        // — until this LLVM path matches, runtime-dispatch dispatch
        // (`&Mixin.method()`) only works with the Cranelift backend.
        // Spec: docs/specs/types/mixin_vtables.spec.md §B2-B3.
        if !program.vtables.is_empty() || !program.class_infos.is_empty() {
            return Err(
                "mixin-vtables: LLVM backend does not yet emit vtable / class_info \
                 data sections — use the Cranelift backend (default) for code that \
                 includes a `dispatch runtime` mixin. Spec §B2-B3."
                    .to_string(),
            );
        }

        // Run optimization passes
        if self.opt_level > 0 {
            optimize::run_optimization(&module, &target_machine, self.opt_level)?;
        }

        // Verify the module
        if let Err(msg) = module.verify() {
            return Err(format!("LLVM IR verification failed: {}", msg.to_string()));
        }

        // Emit object code to memory buffer
        let buffer = target_machine
            .write_to_memory_buffer(&module, FileType::Object)
            .map_err(|e| format!("Failed to emit object: {}", e.to_string()))?;

        self.object_bytes = Some(buffer.as_slice().to_vec());
        Ok(())
    }

    /// Emit the finished object file as raw bytes.
    pub fn finish(self) -> Result<Vec<u8>, String> {
        self.object_bytes
            .ok_or_else(|| "No object bytes — compile_program() not called".to_string())
    }
}
