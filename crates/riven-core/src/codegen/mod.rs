//! Code generation — Cranelift and (optionally) LLVM backends.
//!
//! Translates MIR to native object files and links executables.

pub mod cranelift;
pub mod layout;
pub mod object;
pub mod runtime;
mod runtime_table;

#[cfg(feature = "llvm")]
pub mod llvm;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use crate::mir::nodes::MirProgram;

/// Locate `runtime.c` for the compiler at runtime.
///
/// Resolution order:
/// 1. `RIVEN_RUNTIME` env var — explicit override (path to runtime.c)
/// 2. `<exe_dir>/../lib/runtime.c` — installed toolchain layout (~/.riven/lib)
/// 3. `<exe_dir>/../share/riven/runtime.c` — alternate system layout
/// 4. `<workspace_root>/library/runtime/runtime.c` — dev/workspace builds
pub fn find_runtime_c() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("RIVEN_RUNTIME") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Ok(path);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            if let Some(install_root) = bin_dir.parent() {
                for rel in &["lib/runtime.c", "share/riven/runtime.c"] {
                    let candidate = install_root.join(rel);
                    if candidate.exists() {
                        return Ok(candidate);
                    }
                }
            }
        }
    }

    // Dev fallback — only valid when running from the workspace.
    // After #06.75 Phase B, the runtime lives at <workspace>/library/runtime/runtime.c.
    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("library")
        .join("runtime")
        .join("runtime.c");
    if dev_path.exists() {
        return Ok(dev_path);
    }

    Err(format!(
        "runtime.c not found. Looked in:\n\
         - $RIVEN_RUNTIME\n\
         - <exe>/../lib/runtime.c\n\
         - <exe>/../share/riven/runtime.c\n\
         - {}\n\
         Set RIVEN_RUNTIME to override.",
        dev_path.display()
    ))
}

/// Which code-generation backend to use.
pub enum Backend {
    Cranelift,
    #[cfg(feature = "llvm")]
    Llvm {
        opt_level: u8,
    },
}

/// Compile a MIR program to a native executable.
pub fn compile(program: &MirProgram, output_path: &str) -> Result<(), String> {
    compile_with_options(program, output_path, false, &[], Backend::Cranelift)
}

/// Compile a MIR program to a native executable with additional options.
///
/// - `sanitize`: when true, compile the C runtime with ASan+UBSan and link
///   the sanitizer runtime into the final binary.
/// - `extra_link_flags`: additional linker flags (e.g. `-lfoo` from FFI
///   `@[link("foo")]` attributes).
/// - `backend`: which code-generation backend to use.
pub fn compile_with_options(
    program: &MirProgram,
    output_path: &str,
    sanitize: bool,
    extra_link_flags: &[String],
    backend: Backend,
) -> Result<(), String> {
    // Step 1: Generate object code via the selected backend
    let object_bytes = match backend {
        Backend::Cranelift => {
            let mut codegen = cranelift::CodeGen::new()?;
            codegen.compile_program(program)?;
            codegen.finish()?
        }
        #[cfg(feature = "llvm")]
        Backend::Llvm { opt_level } => {
            let mut codegen = llvm::CodeGen::new(opt_level)?;
            codegen.compile_program(program)?;
            codegen.finish()?
        }
    };

    // Step 2: Compile the C runtime
    let runtime_c = find_runtime_c()?;
    let runtime_o = object::compile_runtime(&runtime_c, sanitize)?;

    // Step 3: Collect FFI link flags from the program
    let mut all_link_flags: Vec<String> = extra_link_flags.to_vec();
    for lib in &program.ffi_libs {
        for flag in &lib.link_flags {
            if !all_link_flags.contains(flag) {
                all_link_flags.push(flag.clone());
            }
        }
    }

    // Step 4: Link into executable
    object::emit_executable(
        &object_bytes,
        &runtime_o,
        output_path,
        sanitize,
        &all_link_flags,
    )?;

    // Clean up runtime object
    let _ = std::fs::remove_file(&runtime_o);

    Ok(())
}
