//! Code generation — Cranelift and (optionally) LLVM backends.
//!
//! Translates MIR to native object files and links executables.

pub mod cranelift;
pub mod lang_intrinsics;
pub mod layout;
pub mod object;
pub mod runtime;
// `runtime_table` deleted in #06.95 Phase E-rest FINAL — every
// dispatch arm migrated to either per-package .rx class lib decls
// (the FFI alias map handles them) or to
// `lang_intrinsics::runtime_name` (compiler-internal mangled
// callees like `Fn(...)_call`, `Float_to_string_prec`, `&str_*`).

#[cfg(feature = "llvm")]
pub mod llvm;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use crate::mir::nodes::MirProgram;

/// Locate the stdlib package root containing per-package `runtime/*.c`
/// trees. After #06.95 Phase B-2 each stdlib package owns its own
/// `runtime/` directory; the unity-build `runtime.c` aggregator is
/// gone, replaced by per-file compilation druxen from this root.
///
/// Resolution order:
/// 1. `RUXEN_STDLIB_ROOT` env var — explicit override (path to `library/std/`)
/// 2. `<exe_dir>/../lib/std/` — installed toolchain layout
/// 3. `<exe_dir>/../share/ruxen/std/` — alternate system layout
/// 4. `<workspace_root>/library/std/` — dev/workspace builds
fn find_stdlib_root() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("RUXEN_STDLIB_ROOT") {
        let path = PathBuf::from(p);
        if path.is_dir() {
            return Ok(path);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            if let Some(install_root) = bin_dir.parent() {
                for rel in &["lib/std", "share/ruxen/std"] {
                    let candidate = install_root.join(rel);
                    if candidate.join("core/runtime").is_dir() {
                        return Ok(candidate);
                    }
                }
            }
        }
    }

    let dev_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("library")
        .join("std");
    if dev_path.join("core/runtime").is_dir() {
        return Ok(dev_path);
    }

    Err(format!(
        "library/std/ not found. Looked in:\n\
         - $RUXEN_STDLIB_ROOT\n\
         - <exe>/../lib/std\n\
         - <exe>/../share/ruxen/std\n\
         - {}\n\
         Set RUXEN_STDLIB_ROOT to override.",
        dev_path.display()
    ))
}

/// Locate every `library/std/<pkg>/runtime/*.c` source file the build
/// driver must compile and link into the final binary.
///
/// After #06.95 Phase B-2, each stdlib package's C runtime ships in its
/// own `runtime/` directory. Each `.c` is a standalone translation unit
/// (it `#include`s `library/std/core/runtime/runtime.h` for the shared
/// type/decl surface); the compiler invokes `cc -c` per file and links
/// the resulting `.o`s into the executable. There is no unity build.
pub fn find_runtime_sources() -> Result<Vec<PathBuf>, String> {
    // `RUXEN_RUNTIME` is the legacy single-file override (one
    // synthesized unity-build `.c`). Tests like
    // `compiler/ruxen_core/tests/drop_fixtures.rs` write a tracked
    // unity into a tempfile, point this env var at it, and expect
    // the compile to use ONLY that file. Honoured for backward
    // compatibility; the modern per-package path uses
    // `RUXEN_STDLIB_ROOT`.
    if let Ok(p) = std::env::var("RUXEN_RUNTIME") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Ok(vec![path]);
        }
    }

    let std_root = find_stdlib_root()?;
    let mut sources: Vec<PathBuf> = Vec::new();

    let pkg_iter = std::fs::read_dir(&std_root)
        .map_err(|e| format!("read_dir({}): {}", std_root.display(), e))?;
    for pkg_entry in pkg_iter {
        let pkg_entry = match pkg_entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let pkg_path = pkg_entry.path();
        if !pkg_path.is_dir() {
            continue;
        }
        let runtime_dir = pkg_path.join("runtime");
        if !runtime_dir.is_dir() {
            continue;
        }
        let file_iter = match std::fs::read_dir(&runtime_dir) {
            Ok(it) => it,
            Err(_) => continue,
        };
        for file_entry in file_iter {
            let file_entry = match file_entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let file_path = file_entry.path();
            if file_path.extension().and_then(|s| s.to_str()) == Some("c") {
                sources.push(file_path);
            }
        }
    }

    if sources.is_empty() {
        return Err(format!(
            "no `runtime/*.c` files found under {}",
            std_root.display()
        ));
    }

    // Stable order for reproducible builds.
    sources.sort();
    Ok(sources)
}

/// Backward-compatibility shim — returns the first stdlib runtime
/// source as a single path. Pre-B-2 callers that expected a single
/// `runtime.c` should migrate to [`find_runtime_sources`].
#[deprecated(note = "use find_runtime_sources — per-package compilation replaces the unity build")]
pub fn find_runtime_c() -> Result<PathBuf, String> {
    find_runtime_sources()?
        .into_iter()
        .next()
        .ok_or_else(|| "no runtime sources".to_string())
}

/// Walk every `library/std/<pkg>/Ruxen.toml`, extract the
/// `[system_libs] libs = [...]` entries, and return the deduplicated
/// union as `-l<name>` linker flags in stable order.
///
/// B3 of `docs/specs/system/zero_rust_stdlib_classes.spec.md`. Pre-B3
/// the linker pulled in `-lc -lm -lpthread` unconditionally via
/// `object::linker_args`; adding a new stdlib package needing
/// (say) `-lssl` required editing that Rust function. Post-B3 each
/// package declares its link needs in its own toml, and the
/// aggregation here gives the final `-l` flag set. Sanitizer flags
/// stay in code (they're not package-specific).
///
/// Schema (intentionally minimal):
/// ```toml
/// [system_libs]
/// libs = ["pthread", "c", "m"]
/// ```
/// Other tables / lines are ignored. The reader is a tiny line scanner
/// rather than a full TOML parser — adding the `toml` crate as a
/// dependency just for this surface would not pull its weight (every
/// existing `Ruxen.toml` in the workspace fits the same trivial
/// `key = value` / `key = ["str", ...]` shape).
pub fn collect_system_lib_flags() -> Result<Vec<String>, String> {
    let std_root = find_stdlib_root()?;
    let mut seen: Vec<String> = Vec::new();

    let mut pkg_paths: Vec<PathBuf> = Vec::new();
    let pkg_iter = std::fs::read_dir(&std_root)
        .map_err(|e| format!("read_dir({}): {}", std_root.display(), e))?;
    for entry in pkg_iter.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let toml_path = path.join("Ruxen.toml");
        if toml_path.is_file() {
            pkg_paths.push(toml_path);
        }
    }
    // Stable order for reproducible link command lines.
    pkg_paths.sort();

    for toml_path in &pkg_paths {
        let contents = match std::fs::read_to_string(toml_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for lib in parse_system_libs(&contents) {
            if !seen.contains(&lib) {
                seen.push(lib);
            }
        }
    }

    Ok(seen.into_iter().map(|l| format!("-l{}", l)).collect())
}

/// Extract the value of `libs = [...]` inside the `[system_libs]`
/// section of a Ruxen.toml string. Returns an empty Vec when the
/// section is absent or the array is empty. Tolerant of `# ...`
/// comments, whitespace, and string quoting (single or double).
///
/// This is the minimum-faithful parser for the B3 schema. It is not
/// a general TOML parser — it does not handle multi-line arrays, key
/// shorthand collisions, or dotted-key forms. If a future schema
/// extension requires more, expand here (do NOT pull in `toml` for
/// what is currently a 30-line surface).
pub fn parse_system_libs(toml_contents: &str) -> Vec<String> {
    let mut in_section = false;
    let mut out: Vec<String> = Vec::new();

    for raw_line in toml_contents.lines() {
        // Strip comment tail.
        let line = match raw_line.find('#') {
            Some(idx) => &raw_line[..idx],
            None => raw_line,
        };
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_section = trimmed == "[system_libs]";
            continue;
        }
        if !in_section {
            continue;
        }
        // Look for `libs = [ ... ]`.
        let Some(rest) = trimmed.strip_prefix("libs") else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim();
        if !rest.starts_with('[') {
            continue;
        }
        // Strip leading `[` and trailing `]` (with anything after).
        let inner = &rest[1..];
        let inner = match inner.rfind(']') {
            Some(idx) => &inner[..idx],
            None => continue,
        };
        for raw in inner.split(',') {
            let entry = raw.trim().trim_matches(|c| c == '"' || c == '\'');
            if !entry.is_empty() {
                out.push(entry.to_string());
            }
        }
    }

    out
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

    // Step 2: Compile every stdlib package's C runtime sources. After
    // #06.95 Phase B-2 each `.c` is a standalone translation unit; we
    // compile each to its own `.o` and link them all into the final
    // binary.
    let runtime_sources = find_runtime_sources()?;
    let runtime_objects = object::compile_runtime_sources(&runtime_sources, sanitize)?;

    // Step 3: Collect FFI link flags from the program AND from every
    // stdlib package's `[system_libs]` table. B3 of
    // `docs/specs/system/zero_rust_stdlib_classes.spec.md` moved the
    // historically hardcoded `-lc / -lm / -lpthread` set into per-
    // package Ruxen.toml `[system_libs] libs = [...]` entries so a
    // new stdlib package needing (say) `-lssl` declares it in its
    // own toml without compiler edits.
    let mut all_link_flags: Vec<String> = extra_link_flags.to_vec();
    if let Ok(system_flags) = collect_system_lib_flags() {
        for flag in system_flags {
            if !all_link_flags.contains(&flag) {
                all_link_flags.push(flag);
            }
        }
    }
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
        &runtime_objects,
        output_path,
        sanitize,
        &all_link_flags,
    )?;

    // Clean up runtime objects
    for o in &runtime_objects {
        let _ = std::fs::remove_file(o);
    }

    Ok(())
}
