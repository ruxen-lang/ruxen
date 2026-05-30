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
            } else if file_path.is_dir() {
                // One level of recursion: per-package vendored C sources
                // (e.g. `library/std/regex/runtime/pcre2/*.c`) get picked
                // up. Mirrors `src/ruxen_repl/build.rs::collect_runtime_sources`
                // so the AOT linker sees the same set of TUs as the JIT.
                // We intentionally do NOT recurse further — keeps the
                // build glob predictable and matches the "vendor in a
                // single subdir" convention this repo uses.
                let sub_iter = match std::fs::read_dir(&file_path) {
                    Ok(it) => it,
                    Err(_) => continue,
                };
                for sub_entry in sub_iter {
                    let sub_entry = match sub_entry {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    let sub_path = sub_entry.path();
                    if sub_path.extension().and_then(|s| s.to_str()) != Some("c") {
                        continue;
                    }
                    // PCRE2 ships two ".c" files that are NOT standalone
                    // translation units — they're pulled in via
                    // `#include` from `pcre2_compile.c` /
                    // `pcre2_tables.c`. Compiling them as TUs would fail
                    // with missing-include errors. Filter by basename
                    // so the build glob stays a glob. Same exclusion
                    // list as `src/ruxen_repl/build.rs`.
                    let name = sub_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    if matches!(name, "pcre2_printint.c" | "pcre2_ucptables.c") {
                        continue;
                    }
                    sources.push(sub_path);
                }
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

/// Locate a prebuilt `libruxenrt.a` so an AOT build can link the stdlib C
/// runtime without recompiling it. Returns the first existing candidate, or
/// `None` (the caller then compiles the runtime `.c` from source).
///
/// Probe order:
/// 1. `RUXEN_RUNTIME_AR` env var — explicit override.
/// 2. `<exe>/../lib/libruxenrt.a` — installed layout (`~/.ruxen/lib/`).
/// 3. `<exe>/../lib/ruxen/<host-triple>/libruxenrt.a` — future per-triple home.
/// 4. `<exe>/libruxenrt.a` and `<exe>/../libruxenrt.a` — dev/test layouts
///    (`target/<profile>/` for `cargo build`, `.../deps/` for `cargo test`).
///
/// NOTE: this removes runtime *C compilation*, not the link step — the final
/// link still invokes `cc`. Full `cc`-elimination depends on the deferred
/// bundled-linker work.
pub fn find_prebuilt_runtime_archive() -> Option<PathBuf> {
    probe_runtime_archive(
        std::env::var("RUXEN_RUNTIME_AR").ok().as_deref(),
        std::env::current_exe().ok().as_deref(),
    )
}

/// Pure core of [`find_prebuilt_runtime_archive`] — takes the env override and
/// the executable path explicitly so it is deterministically testable without
/// mutating process-global state.
fn probe_runtime_archive(env_ar: Option<&str>, exe: Option<&Path>) -> Option<PathBuf> {
    // 1. Explicit override.
    if let Some(p) = env_ar {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }

    let exe = exe?;
    let bin_dir = exe.parent()?;

    if let Some(root) = bin_dir.parent() {
        // 2. Installed layout: ~/.ruxen/bin/ruxen -> ~/.ruxen/lib/libruxenrt.a
        let lib = root.join("lib").join("libruxenrt.a");
        if lib.is_file() {
            return Some(lib);
        }
        // 3. Future per-triple home.
        let triple = root
            .join("lib")
            .join("ruxen")
            .join(option_env!("RUXEN_HOST_TARGET").unwrap_or("unknown"))
            .join("libruxenrt.a");
        if triple.is_file() {
            return Some(triple);
        }
    }

    // 4a. Dev: target/<profile>/libruxenrt.a, exe = target/<profile>/ruxen.
    let beside = bin_dir.join("libruxenrt.a");
    if beside.is_file() {
        return Some(beside);
    }
    // 4b. Test: exe = target/<profile>/deps/<bin> -> target/<profile>/libruxenrt.a.
    if let Some(parent) = bin_dir.parent() {
        let up = parent.join("libruxenrt.a");
        if up.is_file() {
            return Some(up);
        }
    }

    None
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

/// Locate every `<dir>/runtime/*.c` source file under a single project
/// or path-dependency directory.
///
/// Mirrors `find_runtime_sources` (which scans the stdlib root) but for
/// a user project or a single path-dep. The build driver calls this for
/// the project root AND once per entry in `dep_source_dirs` so that a
/// user can drop a `runtime/foo.c` alongside `src/lib.rx` and have it
/// auto-compiled and linked, exactly like a stdlib package does.
///
/// Returns `Ok(vec![])` (NOT an error) when `<dir>/runtime` does not
/// exist — most user projects won't have any C runtime sources, and a
/// missing directory is the normal case. Errors only on read-dir
/// failures of an existing `runtime/` directory.
///
/// Output is sorted for reproducible link command lines.
pub fn find_runtime_sources_in_dir(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let runtime_dir = dir.join("runtime");
    if !runtime_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut sources: Vec<PathBuf> = Vec::new();
    let iter = std::fs::read_dir(&runtime_dir)
        .map_err(|e| format!("read_dir({}): {}", runtime_dir.display(), e))?;
    for entry in iter {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("c") {
            sources.push(path);
        }
    }
    sources.sort();
    Ok(sources)
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
    compile_with_options(program, output_path, false, &[], &[], Backend::Cranelift)
}

/// Compile a MIR program to a native executable with additional options.
///
/// - `sanitize`: when true, compile the C runtime with ASan+UBSan and link
///   the sanitizer runtime into the final binary.
/// - `extra_link_flags`: additional linker flags (e.g. `-lfoo` from FFI
///   `@[link("foo")]` attributes).
/// - `extra_runtime_sources`: additional `.c` files to compile and link
///   alongside the stdlib runtime. The build driver passes the user
///   project's own `runtime/*.c` and each path-dep's `runtime/*.c` here
///   so that a `lib "runtime/foo.c"` declaration in user code resolves
///   to symbols defined in a sibling-of-`src/` C file. Discovery is the
///   caller's responsibility — see [`find_runtime_sources_in_dir`].
/// - `backend`: which code-generation backend to use.
pub fn compile_with_options(
    program: &MirProgram,
    output_path: &str,
    sanitize: bool,
    extra_link_flags: &[String],
    extra_runtime_sources: &[PathBuf],
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

    // Step 2: Provide the C runtime to the linker.
    //
    // Fast path (opt-in): when `RUXEN_RUNTIME_AR=<archive>` names an
    // existing static archive (typically the `libruxenrt.a` that
    // `ruxen_repl`'s build script already produced under
    // `target/<profile>/build/ruxen_repl-*/out/`), skip the per-
    // package `cc -c` invocations entirely and arrange for the
    // linker to whole-archive that archive instead. With 30+ stdlib
    // runtime `.c` files this drops 30+ `cc` forks per compile to
    // zero — the dominant cost in the shell e2e harness and the
    // in-process `release_e2e_smoke` cargo test.
    //
    // Slow path (default): compile every `library/std/<pkg>/runtime/*.c`
    // to its own `.o` (post-#06.95 Phase B-2 standalone translation
    // units) and link those individually. Used when the env var
    // isn't set or the archive is missing.
    //
    // Sanitize builds always take the slow path: the prebuilt
    // archive was compiled without ASan/UBSan instrumentation, so
    // reusing it would silently strip sanitization from runtime calls.
    // `find_prebuilt_runtime_archive` probes the `RUXEN_RUNTIME_AR` override
    // first, then the installed `~/.ruxen/lib/` layout, then dev/test
    // layouts — falling back to `None` (compile the runtime `.c`).
    //
    // The fast path is forced OFF in two cases, because the prebuilt archive
    // is a cache of the *default*, uninstrumented runtime build:
    //   - `sanitize`: the archive carries no ASan/UBSan instrumentation, so
    //     reusing it would silently strip sanitization from runtime calls.
    //   - `RUXEN_RUNTIME` set: an explicit runtime-source override (e.g. the
    //     drop/leak fixtures inject an allocation-tracking runtime). The
    //     requested source must be compiled — the archive must not replace it.
    let runtime_source_overridden = std::env::var_os("RUXEN_RUNTIME").is_some();
    let prebuilt_archive: Option<PathBuf> = if sanitize || runtime_source_overridden {
        None
    } else {
        find_prebuilt_runtime_archive()
    };

    let mut runtime_objects: Vec<PathBuf> = if prebuilt_archive.is_some() {
        Vec::new()
    } else {
        let runtime_sources = find_runtime_sources()?;
        object::compile_runtime_sources(&runtime_sources, sanitize)?
    };

    // Step 2b: Compile any caller-supplied runtime sources (user project
    // `runtime/*.c` and each path-dep's `runtime/*.c`). These share the
    // exact compile path as stdlib runtime — same `cc -c` invocation,
    // same sanitizer flags — so a user `runtime/foo.c` is
    // indistinguishable from a stdlib package's runtime at link time.
    // Always honoured, even on the fast path: the prebuilt archive
    // covers only stdlib runtime, not user runtime sources.
    // On failure we still need to clean up the stdlib objects we just
    // wrote, so route through a helper that propagates the cleanup.
    if !extra_runtime_sources.is_empty() {
        match object::compile_runtime_sources(extra_runtime_sources, sanitize) {
            Ok(mut extra) => runtime_objects.append(&mut extra),
            Err(e) => {
                for o in &runtime_objects {
                    let _ = std::fs::remove_file(o);
                }
                return Err(e);
            }
        }
    }

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

    // Step 3b: When the fast path is active, whole-archive the prebuilt
    // runtime so every `ruxen_*` symbol survives the link. The linker
    // would otherwise drop archive members no user-object section
    // happens to reference, and Cranelift dispatch helpers / FFI calls
    // would see undefined symbols at run time.
    //
    // GNU ld and lld accept `-Wl,--whole-archive <ar> -Wl,--no-whole-archive`.
    // Apple ld doesn't honour `--whole-archive`; on macOS we emit
    // `-Wl,-force_load,<ar>` instead, matching `ruxen_repl/build.rs`
    // and `src/ruxen_cli/build.rs` for the REPL bin.
    if let Some(ar) = &prebuilt_archive {
        let ar_str = ar.to_string_lossy().to_string();
        if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
            all_link_flags.push(format!("-Wl,-force_load,{}", ar_str));
        } else {
            all_link_flags.push("-Wl,--whole-archive".to_string());
            all_link_flags.push(ar_str);
            all_link_flags.push("-Wl,--no-whole-archive".to_string());
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

#[cfg(test)]
mod prebuilt_archive_tests {
    use super::probe_runtime_archive;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static N: AtomicU64 = AtomicU64::new(0);

    /// Unique empty temp dir per call (no external crates).
    fn fresh_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "ruxen_ar_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(p: &std::path::Path) {
        std::fs::write(p, b"!<arch>\n").unwrap();
    }

    #[test]
    fn env_override_existing_file_wins() {
        let dir = fresh_dir();
        let ar = dir.join("libruxenrt.a");
        touch(&ar);
        let got = probe_runtime_archive(Some(ar.to_str().unwrap()), None);
        assert_eq!(got.as_deref(), Some(ar.as_path()));
    }

    #[test]
    fn env_override_missing_file_skipped() {
        let got = probe_runtime_archive(Some("/no/such/libruxenrt.a"), None);
        assert_eq!(got, None);
    }

    #[test]
    fn installed_lib_layout_found() {
        // <root>/bin/ruxen  ->  <root>/lib/libruxenrt.a
        let root = fresh_dir();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let exe = root.join("bin").join("ruxen");
        touch(&exe);
        let ar = root.join("lib").join("libruxenrt.a");
        touch(&ar);
        let got = probe_runtime_archive(None, Some(&exe));
        assert_eq!(got.as_deref(), Some(ar.as_path()));
    }

    #[test]
    fn dev_beside_binary_found() {
        // <profile>/ruxen  ->  <profile>/libruxenrt.a
        let prof = fresh_dir();
        let exe = prof.join("ruxen");
        touch(&exe);
        let ar = prof.join("libruxenrt.a");
        touch(&ar);
        let got = probe_runtime_archive(None, Some(&exe));
        assert_eq!(got.as_deref(), Some(ar.as_path()));
    }

    #[test]
    fn dev_deps_parent_found() {
        // <profile>/deps/<testbin>  ->  <profile>/libruxenrt.a
        let prof = fresh_dir();
        std::fs::create_dir_all(prof.join("deps")).unwrap();
        let exe = prof.join("deps").join("testbin");
        touch(&exe);
        let ar = prof.join("libruxenrt.a");
        touch(&ar);
        let got = probe_runtime_archive(None, Some(&exe));
        assert_eq!(got.as_deref(), Some(ar.as_path()));
    }

    #[test]
    fn none_when_no_archive_anywhere() {
        let prof = fresh_dir();
        let exe = prof.join("ruxen");
        touch(&exe);
        let got = probe_runtime_archive(None, Some(&exe));
        assert_eq!(got, None);
    }
}
