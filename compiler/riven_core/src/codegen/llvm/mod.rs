//! LLVM code generation backend for the Riven compiler.
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
    object_bytes: Option<Vec<u8>>,
}

impl CodeGen {
    /// Create a new LLVM code generator.
    pub fn new(opt_level: u8) -> Result<Self, String> {
        Ok(CodeGen {
            context: Context::create(),
            opt_level,
            object_bytes: None,
        })
    }

    /// Compile all functions in a MIR program to LLVM IR, optimize, and
    /// emit object code.
    pub fn compile_program(&mut self, program: &MirProgram) -> Result<(), String> {
        let module = self.context.create_module("riven_module");

        // Initialize LLVM targets
        Target::initialize_all(&InitializationConfig::default());
        let target_triple = TargetMachine::get_default_triple();
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

        // Compile MIR → LLVM IR
        emit::compile_program(program, &module, &self.context)?;

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
