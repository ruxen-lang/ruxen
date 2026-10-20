//! Code generation — Cranelift and (optionally) LLVM backends.
//!
//! Translates MIR to native object files and links executables.

mod backend;
pub mod cranelift;
pub mod ir_builder;
pub mod layout;
pub mod lower;
pub mod object;
pub mod target;
// The lowering policy (`mir::abi`), the compiler-emitted runtime ABI
// (`mir::residual_abi`) and the callee name mapping
// (`mir::lang_intrinsics::runtime_name`) are MIR's: MIR applies them once as
// explicit `MirInst::Convert`s (`mir::coercion`), and each backend reads them.

#[cfg(feature = "llvm")]
pub mod llvm;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use crate::mir::nodes::MirProgram;

pub use backend::Backend;

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
    // synthesized unity-build `.c`); the compile then uses ONLY that
    // file. Honoured for backward compatibility; in-process tests pass
    // their runtime to `compile_with_runtime_sources` instead, and the
    // modern per-package path uses `RUXEN_STDLIB_ROOT`.
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
                    let name = sub_path.file_name().and_then(|s| s.to_str()).unwrap_or("");
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
        // `.c` C runtime shims, plus `.m` Objective-C shims (macOS AppKit/Cocoa
        // backends — clang compiles `.m` as Objective-C by extension). Frameworks
        // the `.m` needs are added in `object::linker_args` on macOS.
        match path.extension().and_then(|s| s.to_str()) {
            Some("c") | Some("m") => sources.push(path),
            _ => {}
        }
    }
    sources.sort();
    Ok(sources)
}

/// What a project's link line needs beyond the stdlib, shared by `ruxen build`
/// (`ruxen_cli`) and `ruxen test` (`ruxenc`) so the two cannot drift.
///
/// Dependencies are flat-merged into the binary (Q16), so nothing of theirs
/// arrives through an rlib: their C shims and link needs (Q32) are gathered
/// here alongside the project's own.
#[derive(Debug, Default)]
pub struct LinkInputs {
    /// `runtime/*.c` and `.m` shims of the project, then of each dependency.
    pub runtime_sources: Vec<PathBuf>,
    /// Each dependency's `[system_libs] libs`, deduplicated, as bare names.
    pub system_libs: Vec<String>,
    /// `[system_libs] frameworks` of the project and each dependency,
    /// deduplicated. Only macOS links them; callers skip them elsewhere.
    pub frameworks: Vec<String>,
}

impl LinkInputs {
    /// Gather the link inputs of `project_dir` and its topologically sorted
    /// flat-merged `dep_dirs`. A missing `runtime/` or `Ruxen.toml`
    /// contributes nothing; an unreadable `runtime/` is an error.
    pub fn for_project(project_dir: &Path, dep_dirs: &[PathBuf]) -> Result<Self, String> {
        let mut inputs = LinkInputs {
            runtime_sources: find_runtime_sources_in_dir(project_dir)?,
            ..LinkInputs::default()
        };
        push_unique(
            &mut inputs.frameworks,
            parse_system_frameworks(&read_package_manifest(project_dir)),
        );
        for dep_dir in dep_dirs {
            inputs
                .runtime_sources
                .extend(find_runtime_sources_in_dir(dep_dir)?);
            let manifest = read_package_manifest(dep_dir);
            push_unique(&mut inputs.system_libs, parse_system_libs(&manifest));
            push_unique(&mut inputs.frameworks, parse_system_frameworks(&manifest));
        }
        Ok(inputs)
    }
}

/// The text of `<package_dir>/Ruxen.toml`, or empty when it is missing or
/// unreadable (a package without one declares no `[system_libs]`).
fn read_package_manifest(package_dir: &Path) -> String {
    std::fs::read_to_string(package_dir.join("Ruxen.toml")).unwrap_or_default()
}

/// Append each of `items` not already in `out`, keeping first-seen order.
fn push_unique(out: &mut Vec<String>, items: Vec<String>) {
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
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

    let mut pkg_dirs: Vec<PathBuf> = Vec::new();
    let pkg_iter = std::fs::read_dir(&std_root)
        .map_err(|e| format!("read_dir({}): {}", std_root.display(), e))?;
    for entry in pkg_iter.flatten() {
        let path = entry.path();
        if path.is_dir() {
            pkg_dirs.push(path);
        }
    }
    // Stable order for reproducible link command lines.
    pkg_dirs.sort();

    for pkg_dir in &pkg_dirs {
        push_unique(
            &mut seen,
            parse_system_libs(&read_package_manifest(pkg_dir)),
        );
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
    parse_system_array(toml_contents, "libs")
}

/// Like [`parse_system_libs`] but reads `frameworks = [...]` inside
/// `[system_libs]` — the macOS `-framework <name>` set a binary (or one of its
/// deps) needs at link time. A native AppKit widget backend declares
/// `frameworks = ["Cocoa"]`; the build forwards each as `-framework <name>` on
/// macOS (ignored on other targets). Opt-in per package, so non-GUI binaries
/// never load-link AppKit.
pub fn parse_system_frameworks(toml_contents: &str) -> Vec<String> {
    parse_system_array(toml_contents, "frameworks")
}

/// Shared line scanner for a `key = ["a", "b"]` array inside `[system_libs]`.
fn parse_system_array(toml_contents: &str, key: &str) -> Vec<String> {
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
        // Look for `<key> = [ ... ]`.
        let Some(rest) = trimmed.strip_prefix(key) else {
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
///
/// Host-target entry point. Delegates to [`compile_with_options_for_target`]
/// with `None`.
pub fn compile_with_options(
    program: &MirProgram,
    output_path: &str,
    sanitize: bool,
    extra_link_flags: &[String],
    extra_runtime_sources: &[PathBuf],
    backend: Backend,
) -> Result<(), String> {
    compile_with_options_for_target(
        program,
        output_path,
        sanitize,
        extra_link_flags,
        extra_runtime_sources,
        backend,
        None,
    )
}

/// Compile a MIR program to a native executable for an optional
/// cross-compilation target (tier 4.02).
///
/// `target = None` (or a resolved host target) emits for the host and links
/// with [`link_host`]. A non-host target emits for that target, compiles the
/// stdlib runtime *for the target*, and links via the resolved
/// [`target::LinkerSpec`]: local `cc -arch` for a Darwin cross, or the
/// two-stage Docker flow for a Linux target on a host without a cross
/// toolchain. The prebuilt-archive fast path is host-only (a host
/// `libruxenrt.a` is the wrong ABI for a cross target).
#[allow(clippy::too_many_arguments)]
pub fn compile_with_options_for_target(
    program: &MirProgram,
    output_path: &str,
    sanitize: bool,
    extra_link_flags: &[String],
    extra_runtime_sources: &[PathBuf],
    backend: Backend,
    target: Option<&target::ResolvedTarget>,
) -> Result<(), String> {
    let plan = LinkPlan {
        sanitize,
        extra_link_flags,
        extra_runtime_sources,
        runtime_override: None,
    };
    match target {
        Some(target) if !target.is_host() => {
            compile_cross(program, output_path, &plan, backend, target)
        }
        _ => {
            let object_bytes = backend.emit_object(program, &target::ResolvedTarget::host())?;
            link_host(&object_bytes, output_path, &plan)
        }
    }
}

/// Compile a MIR program for the host, linking `runtime_sources` in place
/// of the stdlib C runtime. The rest of the link (caller runtime sources,
/// `[system_libs]` flags, `extra_link_flags`) is the production host path.
/// Used by test harnesses that inject an instrumented runtime, such as the
/// allocation-tracking runtime of the `drop_fixtures` leak tests.
pub fn compile_with_runtime_sources(
    program: &MirProgram,
    output_path: &str,
    runtime_sources: &[PathBuf],
    extra_link_flags: &[String],
) -> Result<(), String> {
    let object_bytes = Backend::Cranelift.emit_object(program, &target::ResolvedTarget::host())?;
    link_host(
        &object_bytes,
        output_path,
        &LinkPlan {
            sanitize: false,
            extra_link_flags,
            extra_runtime_sources: &[],
            runtime_override: Some(runtime_sources),
        },
    )
}

/// What an executable link needs besides the program object. It depends on
/// the target, never on the backend that emitted the object.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinkPlan<'a> {
    /// Compile the C runtime with ASan+UBSan and link the sanitizer runtime
    /// (host links only).
    pub sanitize: bool,
    /// Linker flags ahead of the stdlib `[system_libs]` set: FFI `-l` flags,
    /// dependency `[system_libs]`, frameworks, dependency objects.
    pub extra_link_flags: &'a [String],
    /// Project and dependency `runtime/*.c`, compiled and linked beside the
    /// stdlib runtime even when the prebuilt archive stands in for it.
    pub extra_runtime_sources: &'a [PathBuf],
    /// Sources that replace the stdlib runtime (an instrumented test
    /// runtime); disables the prebuilt archive.
    pub runtime_override: Option<&'a [PathBuf]>,
}

/// The link flags of every executable link: `extra_link_flags`, then each
/// stdlib `[system_libs]` flag not already among them.
///
/// B3 of `docs/specs/system/zero_rust_stdlib_classes.spec.md` moved the
/// historically hardcoded `-lc / -lm / -lpthread` set into per-package
/// Ruxen.toml `[system_libs] libs = [...]` entries, so a new stdlib package
/// needing (say) `-lssl` declares it in its own toml without compiler edits.
pub fn aggregate_link_flags(extra_link_flags: &[String]) -> Vec<String> {
    let mut flags = extra_link_flags.to_vec();
    if let Ok(system_flags) = collect_system_lib_flags() {
        push_unique(&mut flags, system_flags);
    }
    flags
}

/// Link a host `object_bytes` into the executable `output_path`: the one
/// host link of `ruxen build`, `ruxen compile`/`test` and the test harnesses.
///
/// The stdlib C runtime arrives as the prebuilt `libruxenrt.a` when
/// [`find_prebuilt_runtime_archive`] finds one (`RUXEN_RUNTIME_AR`, the
/// installed `~/.ruxen/lib/`, then dev layouts), which skips 30+ `cc -c`
/// forks per link; otherwise each `library/std/<pkg>/runtime/*.c` is
/// compiled to its own object. The archive is a cache of the default,
/// uninstrumented runtime, so it is not used for a `sanitize` link (it would
/// strip the sanitizers from runtime calls) or when the runtime sources are
/// overridden (`plan.runtime_override` or `RUXEN_RUNTIME`): the requested
/// sources must be compiled. The plan's own runtime sources are always
/// compiled, with the same `cc -c` as the stdlib's.
pub fn link_host(object_bytes: &[u8], output_path: &str, plan: &LinkPlan) -> Result<(), String> {
    let runtime_source_overridden =
        plan.runtime_override.is_some() || std::env::var_os("RUXEN_RUNTIME").is_some();
    let prebuilt_archive: Option<PathBuf> = if plan.sanitize || runtime_source_overridden {
        None
    } else {
        find_prebuilt_runtime_archive()
    };

    let mut sources: Vec<PathBuf> = match (&prebuilt_archive, plan.runtime_override) {
        (Some(_), _) => Vec::new(),
        (None, Some(sources)) => sources.to_vec(),
        (None, None) => find_runtime_sources()?,
    };
    sources.extend_from_slice(plan.extra_runtime_sources);
    let runtime_objects =
        object::compile_c_sources(&sources, |src| object::compile_runtime(src, plan.sanitize))?;

    // `emit_executable` whole-archives the prebuilt runtime (so every
    // `ruxen_*` symbol survives) and places it, like every other input,
    // before the link flags.
    let linked = object::emit_executable(
        object_bytes,
        &runtime_objects,
        prebuilt_archive.as_deref(),
        output_path,
        plan.sanitize,
        &aggregate_link_flags(plan.extra_link_flags),
    );
    object::remove_objects(&runtime_objects);
    linked
}

/// The cross-compilation path (tier 4.02): non-host target.
///
/// Always compiles the stdlib runtime from source FOR THE TARGET (the host
/// `libruxenrt.a` fast path is skipped — wrong ABI), then links via the
/// resolved [`target::LinkerSpec`]. Sanitizer builds are not supported on the
/// cross path (the runtime is compiled plainly).
fn compile_cross(
    program: &MirProgram,
    output_path: &str,
    plan: &LinkPlan,
    backend: Backend,
    target: &target::ResolvedTarget,
) -> Result<(), String> {
    // `Backend::select` refuses Cranelift for a target that needs LLVM.
    let object_bytes = backend.emit_object(program, target)?;

    // Tier 4.09 (WASM): link the curated heap-core runtime + bundled allocator/
    // libc shim so `String`/`Array` work. The LLVM backend emitted the user's
    // WebAssembly object (with `export_name` attrs on every
    // `program.wasm_exports` entry). Compile the curated runtime `.c` subset
    // (WASM_RUNTIME_CORE) + the wasm shim to wasm objects with clang, then
    // link everything with `wasm-ld`.
    if target.is_wasm() {
        let core_sources: Vec<PathBuf> = find_runtime_sources()?
            .into_iter()
            .filter(|src| {
                let base = src.file_name().and_then(|s| s.to_str()).unwrap_or("");
                object::WASM_RUNTIME_CORE.contains(&base)
            })
            .collect();
        let mut runtime_objects =
            object::compile_c_sources(&core_sources, object::compile_runtime_for_wasm)?;
        let shim = object::compile_wasm_shim()
            .inspect_err(|_| object::remove_objects(&runtime_objects))?;
        runtime_objects.push(shim);
        return object::emit_wasm_module(&object_bytes, &runtime_objects, output_path, target);
    }

    // Resolve the linker strategy for this target on this host.
    let spec = target::linker_for(
        target,
        target::HostOs::current(),
        target::HostArch::current(),
        which_on_path,
    )?;

    // Runtime sources for the target. The container path compiles them
    // inside the container; the local path compiles them here with the
    // target-aware `cc` so the runtime object matches the target ABI.
    let mut all_runtime_sources = find_runtime_sources()?;
    all_runtime_sources.extend_from_slice(plan.extra_runtime_sources);
    let runtime_objects: Vec<PathBuf> = if spec.needs_container {
        Vec::new()
    } else {
        object::compile_c_sources(&all_runtime_sources, |src| {
            object::compile_runtime_for_target(src, target, &spec.target_args)
        })?
    };

    object::emit_executable_for_target(
        &object_bytes,
        &runtime_objects,
        &all_runtime_sources,
        output_path,
        target,
        &spec,
        &aggregate_link_flags(plan.extra_link_flags),
    )
}

/// Compile a MIR program to a `no_std` host executable (tier 4.04).
///
/// A no_std build links **without the stdlib C runtime and without the
/// per-package `[system_libs]`** (no `-lc -lm`, no `runtime/*.c`): the user's
/// object is the whole program. This is the honest no-stdlib build — the
/// caller has already (a) skipped the stdlib bootstrap and (b) run the E1400
/// heap-allocation check, so the MIR references no `ruxen_*` runtime symbol.
///
/// Platform note: on macOS, `cc` still implicitly links `libSystem` (the OS
/// mandates it for any dynamic executable — a truly libc-free binary is not
/// possible there), but NO Ruxen stdlib runtime is linked. The strict
/// `-nostdlib`, zero-libc-imports variant is a Linux target (verified in a
/// container) and is the staged remainder for embedded triples. See
/// `docs/decisions/phase4-no-std-wasm.md`.
pub fn compile_no_std(
    program: &MirProgram,
    output_path: &str,
    backend: Backend,
) -> Result<(), String> {
    let object_bytes = backend.emit_object(program, &target::ResolvedTarget::host())?;

    // Link the user object alone — no stdlib runtime objects, no
    // `[system_libs]` aggregation, no FFI flags. `emit_executable` with empty
    // runtime objects + empty link flags is exactly "bare `cc <obj> -o <out>`"
    // (plus the macOS deployment-target pin, which is harmless).
    object::emit_executable(&object_bytes, &[], None, output_path, false, &[])
}

/// Minimal `which`: is `name` an executable on `$PATH`? Used to detect an
/// installed cross gcc (e.g. `aarch64-linux-gnu-gcc`) before falling back to
/// the container link path.
fn which_on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file()
    })
}

#[cfg(test)]
mod prebuilt_archive_tests {
    use super::probe_runtime_archive;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static N: AtomicU64 = AtomicU64::new(0);

    /// Unique empty temp dir per call (no external crates), removed when
    /// the test drops it.
    struct FreshDir(PathBuf);

    impl std::ops::Deref for FreshDir {
        type Target = PathBuf;
        fn deref(&self) -> &PathBuf {
            &self.0
        }
    }

    impl Drop for FreshDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn fresh_dir() -> FreshDir {
        let d = std::env::temp_dir().join(format!(
            "ruxen_ar_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&d).unwrap();
        FreshDir(d)
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
