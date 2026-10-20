//! LLVM code generation backend for the Ruxen compiler.
//!
//! Translates MIR programs into native object code via LLVM/inkwell.
//! Feature-gated behind `--features llvm`.

use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::targets::*;
use inkwell::OptimizationLevel;

use crate::mir::nodes::MirProgram;

mod debug;
pub mod emit;
pub mod optimize;
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
        let (module, target_machine) = Self::build_module(
            &self.context,
            program,
            self.opt_level,
            self.target_triple.as_deref(),
        )?;

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

    /// Build and verify the module, returning its textual LLVM IR (no linking).
    /// Used by the P0.14 debug-info tests to assert on emitted DWARF metadata
    /// (`!llvm.dbg.cu`, `DISubprogram`, `!DILocation`). Honours the same
    /// debug-info gate as `compile_program` (line info at `opt_level == 0`,
    /// non-wasm).
    pub fn emit_llvm_ir(&self, program: &MirProgram) -> Result<String, String> {
        let (module, _target_machine) = Self::build_module(
            &self.context,
            program,
            self.opt_level,
            self.target_triple.as_deref(),
        )?;
        if let Err(msg) = module.verify() {
            return Err(format!("LLVM IR verification failed: {}", msg.to_string()));
        }
        Ok(module.print_to_string().to_string())
    }

    /// Create the module, set up the target machine, and lower every MIR
    /// function into it, including DWARF line info (P0.14) for unoptimized,
    /// non-wasm builds. Shared by `compile_program` (object emission) and
    /// `emit_llvm_ir` (IR inspection). Does NOT run optimization or verify;
    /// callers do that so they can order object/IR emission themselves.
    fn build_module<'ctx>(
        context: &'ctx Context,
        program: &MirProgram,
        opt_level: u8,
        target_triple: Option<&str>,
    ) -> Result<(Module<'ctx>, TargetMachine), String> {
        let module = context.create_module("ruxen_module");

        // Initialize LLVM targets
        Target::initialize_all(&InitializationConfig::default());
        // Tier 4.02: honour an explicit cross target; default to host.
        let resolved_triple = match target_triple {
            Some(t) => TargetTriple::create(t),
            None => TargetMachine::get_default_triple(),
        };
        module.set_triple(&resolved_triple);

        let target =
            Target::from_triple(&resolved_triple).map_err(|e| format!("Unknown target: {}", e))?;
        let target_machine = target
            .create_target_machine(
                &resolved_triple,
                "generic",
                "",
                match opt_level {
                    0 => OptimizationLevel::None,
                    1 => OptimizationLevel::Less,
                    3 => OptimizationLevel::Aggressive,
                    _ => OptimizationLevel::Default,
                },
                RelocMode::PIC,
                CodeModel::Default,
            )
            .ok_or("Failed to create target machine")?;
        let target_data = target_machine.get_target_data();
        module.set_data_layout(&target_data.get_data_layout());

        // Compile MIR → LLVM IR. On a wasm32 target, the emitter sets the
        // `export_name` attribute on each `program.wasm_exports` entry so the
        // function is a host-callable wasm export (tier 4.03).
        let is_wasm =
            target_triple.is_some_and(|t| t.starts_with("wasm32") || t.starts_with("wasm64"));

        // DWARF line info (P0.14) only for unoptimized, non-wasm builds: `-O0`
        // is the existing debug/release signal, and DWARF in a wasm object
        // would perturb the (out-of-scope) wasm codegen path.
        let debug = opt_level == 0 && !is_wasm;

        emit::compile_program(program, &module, context, is_wasm, debug)?;

        Ok((module, target_machine))
    }

    /// Emit the finished object file as raw bytes.
    pub fn finish(self) -> Result<Vec<u8>, String> {
        self.object_bytes
            .ok_or_else(|| "No object bytes — compile_program() not called".to_string())
    }
}
