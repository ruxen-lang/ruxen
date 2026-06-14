//! Single-file compile driver — the bare `ruxenc <file.rx>` path.
//!
//! Exposed as `ruxenc::compile::run(&args)` so both binaries (`ruxenc` and
//! `ruxen`) call the same code. All `process::exit(N)` sites from the
//! original main have been converted to `Err(String)`; callers exit on Err.
//!
//! Args layout (caller responsibility): `args[0]` is the program name and is
//! IGNORED — the file path lives at `args[1]`. Remaining slots are options.
//! This matches the legacy `ruxenc <file> [opts...]` shape so the existing
//! call sites (and the bench module's `run` invocation) keep working.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use ruxen_core::borrow_check;
use ruxen_core::diagnostics::Diagnostic;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::ast::Program;
use ruxen_core::parser::Parser;
use ruxen_core::resolve::bootstrap as stdlib_bootstrap;
use ruxen_core::typeck;

use crate::cache;
use crate::cache::{
    build as cache_build, extract_signature, BuildOptions, CacheStore, CompileOutput, FileStatus,
    SourceFile,
};

pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err("missing input file (expected `<program> <file.rx>`)".into());
    }
    let path = &args[1];
    if !path.ends_with(".rx") {
        return Err(format!("expected a .rx file, got: {}", path));
    }

    // Parse CLI options
    let mut output_path: Option<String> = None;
    let mut emit_mode: Option<String> = None;
    let mut release_mode = false;
    let mut backend_override: Option<String> = None;
    let mut opt_level_override: Option<String> = None;
    let mut force = false;
    let mut verbose = false;
    let mut target_flag: Option<String> = None;
    let mut no_std = false;
    let mut extra_runtime_c: Vec<String> = Vec::new();
    let mut extra_link_args: Vec<String> = Vec::new();
    let mut i = 2;
    while i < args.len() {
        if args[i] == "-o" && i + 1 < args.len() {
            output_path = Some(args[i + 1].clone());
            i += 2;
        } else if args[i] == "--target" && i + 1 < args.len() {
            // tier 4.02: cross-compilation target triple. `--target <triple>`
            // (space-separated, matching rustc/cargo) OR `--target=<triple>`.
            target_flag = Some(args[i + 1].clone());
            i += 2;
        } else if args[i].starts_with("--target=") {
            target_flag = Some(args[i]["--target=".len()..].to_string());
            i += 1;
        } else if args[i].starts_with("--runtime-c=") {
            // Additional user C source to compile and link alongside the
            // stdlib runtime (a project's `runtime/*.c`). Repeatable.
            // `ruxen test` passes these so `lib "runtime/foo.c"` decls in
            // library code resolve inside test binaries, mirroring what
            // `ruxen build` does via `find_runtime_sources_in_dir`.
            extra_runtime_c.push(args[i]["--runtime-c=".len()..].to_string());
            i += 1;
        } else if args[i].starts_with("--link-arg=") {
            // Additional raw linker flag (e.g. `-lpthread` from a
            // dependency's `[system_libs]`). Repeatable. `ruxen test`
            // passes one per `-l<lib>` entry of each flat-merged FFI
            // dependency, mirroring the `[system_libs]` aggregation
            // `ruxen build` performs for a directly-declared dep (Q32).
            extra_link_args.push(args[i]["--link-arg=".len()..].to_string());
            i += 1;
        } else if args[i].starts_with("--emit=") {
            emit_mode = Some(args[i][7..].to_string());
            i += 1;
        } else if args[i] == "--no-std" || args[i] == "--no_std" {
            // tier 4.04: no_std mode — don't bootstrap the hosted stdlib,
            // reject heap allocation (E1400), and link without the stdlib
            // C runtime / `[system_libs]`.
            no_std = true;
            i += 1;
        } else if args[i] == "--release" {
            release_mode = true;
            i += 1;
        } else if args[i].starts_with("--backend=") {
            backend_override = Some(args[i]["--backend=".len()..].to_string());
            i += 1;
        } else if args[i].starts_with("--opt-level=") {
            opt_level_override = Some(args[i]["--opt-level=".len()..].to_string());
            i += 1;
        } else if args[i] == "--force" {
            force = true;
            i += 1;
        } else if args[i] == "--verbose" {
            verbose = true;
            i += 1;
        } else {
            i += 1;
        }
    }

    let output_path = output_path.unwrap_or_else(|| path.replace(".rx", ""));

    let source =
        fs::read_to_string(path).map_err(|e| format!("Error reading '{}': {}", path, e))?;

    // Resolve the cross-compilation target (tier 4.02). `None`/host keeps the
    // byte-identical cached host path below; a non-host target takes the
    // dedicated cross path (no incremental cache — one-shot compile + cross
    // link), so the host cache key is never perturbed by this feature.
    let resolved_target =
        ruxen_core::codegen::target::ResolvedTarget::resolve(target_flag.as_deref())?;

    // Emit modes short-circuit the cache: they don't produce a binary, so
    // caching them is meaningless and would add complexity. Run the
    // classic single-shot pipeline for these.
    if emit_mode.is_some() {
        return run_compile_direct(
            path,
            &source,
            &output_path,
            emit_mode.as_deref(),
            release_mode,
            backend_override.as_deref(),
            opt_level_override.as_deref(),
        );
    }

    // ─── Cross-compilation path (tier 4.02) ──────────────────────────────
    // A non-host target bypasses the incremental object cache: cross builds
    // are infrequent and the cross link (local `cc -arch` or two-stage
    // Docker) is one-shot. The full pipeline runs once and hands the MIR to
    // `compile_with_options_for_target`, which selects the ISA, compiles the
    // stdlib runtime FOR THE TARGET, and links via the resolved LinkerSpec.
    if !resolved_target.is_host() {
        return run_cross_compile(
            &source,
            path,
            &output_path,
            release_mode,
            backend_override.as_deref(),
            opt_level_override.as_deref(),
            &resolved_target,
            &extra_runtime_c,
            &extra_link_args,
        );
    }

    // ─── no_std path (tier 4.04) ─────────────────────────────────────
    // A no_std host build bypasses the incremental cache (it's a distinct,
    // infrequent build with no stdlib runtime) and runs a dedicated pipeline:
    // skip the stdlib bootstrap, enforce E1400 (no heap allocation), and link
    // WITHOUT the stdlib C runtime / `[system_libs]`.
    if no_std {
        return run_no_std_compile(&source, path, &output_path, release_mode);
    }

    // ─── Cached compile path ─────────────────────────────────────
    let store = CacheStore::new(project_target_ruxen());
    let opt_level_label = if release_mode { "release" } else { "debug" };
    // Fold the extra runtime C sources (path + content hash) into the cache
    // flags so editing a project's `runtime/*.c` invalidates the cached
    // binary even when no `.rx` source changed.
    let runtime_c_fingerprint = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        for p in &extra_runtime_c {
            p.hash(&mut h);
            if let Ok(bytes) = fs::read(p) {
                bytes.hash(&mut h);
            }
        }
        h.finish()
    };
    let flags = format!(
        "backend={} opt={} release={} runtime_c={:x} link_args={} toolchain={:x}",
        backend_override.as_deref().unwrap_or("default"),
        opt_level_override.as_deref().unwrap_or("default"),
        release_mode,
        runtime_c_fingerprint,
        extra_link_args.join(","),
        toolchain_fingerprint()
    );
    let build_opts = BuildOptions {
        force,
        verbose,
        target: cache::default_target().to_string(),
        opt_level: opt_level_label.to_string(),
        flags,
        parallel: false,
    };

    let backend_override_owned = backend_override.clone();
    let opt_level_override_owned = opt_level_override.clone();
    let compile_one = move |f: &SourceFile| -> Result<CompileOutput, String> {
        compile_to_object(
            &f.source,
            &f.path,
            release_mode,
            backend_override_owned.as_deref(),
            opt_level_override_owned.as_deref(),
        )
    };

    let files = vec![SourceFile {
        path: path.to_string(),
        source: source.clone(),
    }];

    let result = cache_build(files, &store, &build_opts, &compile_one)
        .map_err(|e| format!("Build failed: {}", e))?;

    // ─── Linking / skip-link ─────────────────────────────────────
    // If nothing changed and the output binary already exists, skip the
    // linker entirely.
    let output_exists = Path::new(&output_path).exists();
    if !result.any_object_changed && output_exists {
        if verbose {
            eprintln!("[cache] all objects unchanged, skipping link step");
        }
        println!("Up to date: {}", output_path);
        report_statuses(&result.statuses, verbose);
        return Ok(());
    }

    // Gather object bytes and link.
    // ruxenc is single-file today; guard against silent data loss if a
    // multi-file BuildResult ever flows through this path.
    if result.objects.len() > 1 {
        return Err(format!(
            "internal error: ruxenc CLI received {} objects but only supports single-file linking",
            result.objects.len()
        ));
    }
    let (_, obj_path) = result
        .objects
        .first()
        .ok_or_else(|| "No objects produced — nothing to link.".to_string())?;
    let object_bytes = fs::read(obj_path)
        .map_err(|e| format!("Failed to read cached object {}: {}", obj_path.display(), e))?;

    // Fast path: `RUXEN_RUNTIME_AR=<archive>` lets a caller skip the
    // 30+ `cc -c` forks per fixture by pointing at a prebuilt
    // `libruxenrt.a` (the one `ruxen_repl`'s build script already
    // produces under `target/<profile>/build/ruxen_repl-*/out/` is a
    // ready-made match). When set, we leave the stdlib runtime
    // compilation step empty and pass `-Wl,--whole-archive <ar>
    // -Wl,--no-whole-archive` to the linker so every `ruxen_*` symbol
    // survives — the same shape `ruxen_repl/build.rs` and
    // `src/ruxen_cli/build.rs` use for their own bin links. The
    // release-e2e harness exploits this to drop per-fixture wall-clock
    // from ~4s to under a second. Apple ld doesn't honour
    // `--whole-archive`; it uses `-force_load,<ar>` instead.
    //
    // Default path: each `library/std/<pkg>/runtime/*.c` is still
    // compiled to its own `.o` (post-#06.95 Phase B-2 standalone
    // translation units) and linked individually, preserving the
    // dead-code elimination the production binaries depend on.
    // Prefer a prebuilt `libruxenrt.a` so we skip ~30 `cc -c` forks of the
    // stdlib runtime C sources on every single-file compile (the dominant
    // cost — e.g. `ruxen test` recompiled the runtime once per test file).
    // `find_prebuilt_runtime_archive` checks `RUXEN_RUNTIME_AR` first, then
    // the installed `<exe>/../lib/libruxenrt.a` (~/.ruxen/lib/), so an
    // installed toolchain gets the fast path automatically. Falls back to
    // compiling the runtime sources only when no archive is found.
    let prebuilt_archive: Option<std::path::PathBuf> =
        ruxen_core::codegen::find_prebuilt_runtime_archive();

    let mut runtime_objects: Vec<std::path::PathBuf> = if prebuilt_archive.is_some() {
        Vec::new()
    } else {
        let runtime_sources = ruxen_core::codegen::find_runtime_sources()?;
        ruxen_core::codegen::object::compile_runtime_sources(&runtime_sources, false)
            .map_err(|e| format!("Failed to compile runtime: {}", e))?
    };

    // User-side runtime C (`--runtime-c=`): always compiled, independent of
    // the prebuilt stdlib archive — these are project sources, not stdlib.
    if !extra_runtime_c.is_empty() {
        let extra_paths: Vec<std::path::PathBuf> = extra_runtime_c
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
        let extra_objects =
            ruxen_core::codegen::object::compile_runtime_sources(&extra_paths, false)
                .map_err(|e| format!("Failed to compile project runtime C: {}", e))?;
        runtime_objects.extend(extra_objects);
    }

    let mut link_flags: Vec<String> = Vec::new();
    // Dependency `[system_libs]` flags (`-l<lib>`), forwarded by `ruxen test`
    // so a flat-merged FFI dependency's link needs are satisfied (Q32).
    link_flags.extend(extra_link_args.iter().cloned());
    if let Some(ar) = &prebuilt_archive {
        let ar_str = ar.to_string_lossy().to_string();
        if cfg!(target_os = "macos") || cfg!(target_os = "ios") {
            link_flags.push(format!("-Wl,-force_load,{}", ar_str));
        } else {
            link_flags.push("-Wl,--whole-archive".to_string());
            link_flags.push(ar_str);
            link_flags.push("-Wl,--no-whole-archive".to_string());
        }
    }

    if let Err(e) = ruxen_core::codegen::object::emit_executable(
        &object_bytes,
        &runtime_objects,
        &output_path,
        false,
        &link_flags,
    ) {
        for o in &runtime_objects {
            let _ = fs::remove_file(o);
        }
        return Err(format!("Linking failed: {}", e));
    }
    for o in &runtime_objects {
        let _ = fs::remove_file(o);
    }

    println!("Compiled {} → {}", path, output_path);
    report_statuses(&result.statuses, verbose);
    Ok(())
}

fn report_statuses(statuses: &std::collections::HashMap<String, FileStatus>, verbose: bool) {
    if !verbose {
        return;
    }
    for (p, s) in statuses {
        match s {
            FileStatus::CacheHit => eprintln!("[cache] {}: cache hit", p),
            FileStatus::Recompiled { output_changed } => eprintln!(
                "[cache] {}: recompiled (output_changed={})",
                p, output_changed
            ),
            FileStatus::InvalidatedByDependency { output_changed } => eprintln!(
                "[cache] {}: invalidated by dep (output_changed={})",
                p, output_changed
            ),
        }
    }
}

/// Run the stdlib bootstrap loader and surface any diagnostic as an Err with a
/// clear `stdlib bootstrap failed:` header. The compiler cannot make progress
/// without a clean prelude — a missing or broken stdlib is a fatal install/
/// build issue, not a recoverable user-source error.
fn load_bootstrap_or_err() -> Result<Vec<(String, Program)>, String> {
    let mut bootstrap_diags: Vec<Diagnostic> = Vec::new();
    let programs = stdlib_bootstrap::run_bootstrap_with_package_names(&mut bootstrap_diags);
    if !bootstrap_diags.is_empty() {
        let mut msg = String::from("stdlib bootstrap failed:");
        for d in &bootstrap_diags {
            msg.push_str("\n  ");
            msg.push_str(&d.to_string());
        }
        return Err(msg);
    }
    Ok(programs)
}

/// Curated stdlib bootstrap for the wasm (LLVM) target — tier 4.09.
///
/// The *full* bootstrap can't run on wasm: it pulls in `dispatch runtime` mixin
/// classes (only `future` uses the feature) whose `__rx_classinfo_*` globals the
/// LLVM backend can't lower, plus libc/host-heavy modules the wasm link has no
/// allocator for. So load only the heap-core subset. The selection is
/// overridable via `RUXEN_WASM_BOOTSTRAP` (comma-separated package names) so the
/// viable set can be tuned without recompiling the toolchain as the wasm runtime
/// grows. Load order follows `BOOTSTRAP_FILES` (types-before-use), filtered to
/// the selected packages.
fn load_wasm_bootstrap_or_err() -> Result<Vec<(String, Program)>, String> {
    // Single source of truth for the curated wasm package set lives in
    // ruxen_core (shared with the `ruxen build` path). See
    // `run_wasm_bootstrap_with_package_names` / `WASM_BOOTSTRAP_DEFAULT`.
    let mut diags: Vec<Diagnostic> = Vec::new();
    let programs = stdlib_bootstrap::run_wasm_bootstrap_with_package_names(&mut diags);
    if !diags.is_empty() {
        let mut msg = String::from("wasm stdlib bootstrap failed:");
        for d in &diags {
            msg.push_str("\n  ");
            msg.push_str(&d.to_string());
        }
        return Err(msg);
    }
    Ok(programs)
}

fn type_check_with_package_bootstrap(
    program: &Program,
    bootstrap_packages: &[(String, Program)],
) -> typeck::TypeCheckResult {
    let mut lowered = program.clone();
    let e1112_diags = ruxen_core::async_lowering::collect_block_on_in_async_diagnostics(&lowered);
    let e1116_diags =
        ruxen_core::async_lowering::collect_task_spawn_outside_async_diagnostics(&lowered);
    let e1115_diags = ruxen_core::async_lowering::collect_await_in_loop_diagnostics(&lowered);
    let bootstrap_refs: Vec<&Program> = bootstrap_packages.iter().map(|(_, p)| p).collect();
    ruxen_core::async_lowering::lower_async_defs_with_bootstrap(&mut lowered, &bootstrap_refs);

    let mut result = typeck::type_check_with_bootstrap_packages(&lowered, bootstrap_packages);
    result.diagnostics.extend(e1112_diags);
    result.diagnostics.extend(e1116_diags);
    result.diagnostics.extend(e1115_diags);
    result
}

/// Compile one source string into its object bytes plus a public signature.
///
/// Returns an error string containing all diagnostics on pipeline failure. The
/// caller (the cache driver) propagates this upwards without any recovery —
/// the cache layer is not responsible for compiler error reporting policy.
fn compile_to_object(
    source: &str,
    _path: &str,
    release_mode: bool,
    backend_override: Option<&str>,
    opt_level_override: Option<&str>,
) -> Result<CompileOutput, String> {
    // Stdlib bootstrap is loaded BEFORE user lex/parse so the resolver
    // can merge the prelude in front of the user program. A broken
    // stdlib aborts the process immediately — see
    // [`load_bootstrap_or_err`].
    let bootstrap_programs = load_bootstrap_or_err()?;

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    let type_result = type_check_with_package_bootstrap(&program, &bootstrap_programs);
    let has_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error);
    if has_errors {
        let msg: String = type_result
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(msg);
    }

    let borrow_errors = borrow_check::borrow_check(&type_result.program, &type_result.symbols);
    if !borrow_errors.is_empty() {
        let msg: String = borrow_errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(msg);
    }

    // Extract public signature BEFORE MIR lowering (only typed HIR is needed).
    let signature = extract_signature(&type_result.program);

    let mut lowerer = ruxen_core::mir::lower::Lowerer::new(&type_result.symbols);
    let mir_program = lowerer
        .lower_program(&type_result.program)
        .map_err(|e| format!("MIR lowering error: {}", e))?;

    let backend = resolve_backend(release_mode, backend_override, opt_level_override)?;
    let object_bytes = match backend {
        ruxen_core::codegen::Backend::Cranelift => {
            let mut codegen = ruxen_core::codegen::cranelift::CodeGen::new()?;
            codegen.compile_program(&mir_program)?;
            codegen.finish()?
        }
        #[cfg(feature = "llvm")]
        ruxen_core::codegen::Backend::Llvm { opt_level } => {
            let mut codegen = ruxen_core::codegen::llvm::CodeGen::new(opt_level)?;
            codegen.compile_program(&mir_program)?;
            codegen.finish()?
        }
    };

    // Dependencies: today ruxenc is single-file, so the dep list is empty.
    // When multi-file support is added, extract cross-file imports from the
    // resolver's type registry here.
    Ok(CompileOutput {
        object_bytes,
        signature,
        dependencies: Vec::new(),
    })
}

/// Cross-compilation path (tier 4.02): runs the full front end once, selects a
/// target-compatible backend, and links via `compile_with_options_for_target`.
///
/// Bypasses the incremental object cache (cross builds are infrequent and the
/// cross link is one-shot), so the host cache key is never perturbed by a
/// `--target`. The backend is forced to LLVM when the target requires it
/// (`wasm`/embedded) regardless of `--release`, mirroring spec §5.8.
#[allow(clippy::too_many_arguments)]
fn run_cross_compile(
    source: &str,
    _path: &str,
    output_path: &str,
    release_mode: bool,
    backend_override: Option<&str>,
    opt_level_override: Option<&str>,
    target: &ruxen_core::codegen::target::ResolvedTarget,
    extra_runtime_c: &[String],
    extra_link_args: &[String],
) -> Result<(), String> {
    // Tier 4.03/4.04: a wasm32 target is a no_std reactor — it does NOT
    // bootstrap the hosted stdlib. Bootstrapping would pull in
    // `dispatch runtime` stdlib classes (e.g. `TimeSleepFuture`), whose
    // vtable/class_info globals the LLVM backend does not yet emit, and would
    // also pull libc-dependent runtime the wasm link has no allocator for.
    // The no_std core surface (primitive ops) needs no bootstrap for the
    // math-export v1 path. (Loading `library/std/core` alone is the staged
    // remainder — ADR phase4-no-std-wasm decision #1.)
    // Tier 4.09: wasm bootstraps a CURATED stdlib subset (heap-core: core,
    // array, string, …) — not the empty set (which left `Array`/`String`
    // unresolvable) nor the full bootstrap (which drags in `dispatch runtime`
    // class_info the LLVM backend can't lower). See `load_wasm_bootstrap_or_err`.
    let bootstrap_programs: Vec<(String, Program)> = if target.is_wasm() {
        load_wasm_bootstrap_or_err()?
    } else {
        load_bootstrap_or_err()?
    };

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    let type_result = type_check_with_package_bootstrap(&program, &bootstrap_programs);
    let has_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error);
    if has_errors {
        return Err(type_result
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let borrow_errors = borrow_check::borrow_check(&type_result.program, &type_result.symbols);
    if !borrow_errors.is_empty() {
        return Err(borrow_errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let mut lowerer = ruxen_core::mir::lower::Lowerer::new(&type_result.symbols);
    let mir_program = lowerer
        .lower_program(&type_result.program)
        .map_err(|e| format!("MIR lowering error: {}", e))?;

    // Backend: a target that can't run on Cranelift (wasm/embedded) forces
    // LLVM, regardless of --release. Otherwise honour the same default/override
    // logic as the host path.
    let backend = if target.requires_llvm_backend() {
        if matches!(backend_override, Some("cranelift")) {
            return Err(format!(
                "target '{}' requires the LLVM backend. \
                 Build with --release or pass --backend=llvm.",
                target.canonical()
            ));
        }
        // Force LLVM (silent auto-switch per §5.8).
        resolve_backend(true, Some("llvm"), opt_level_override)?
    } else {
        resolve_backend(release_mode, backend_override, opt_level_override)?
    };

    let extra_runtime_paths: Vec<std::path::PathBuf> = extra_runtime_c
        .iter()
        .map(std::path::PathBuf::from)
        .collect();

    ruxen_core::codegen::compile_with_options_for_target(
        &mir_program,
        output_path,
        false, // no sanitizer on the cross path
        extra_link_args,
        &extra_runtime_paths,
        backend,
        Some(target),
    )?;

    println!(
        "Cross-compiled {} → {} (target {})",
        _path,
        output_path,
        target.canonical()
    );
    Ok(())
}

/// no_std host compile (tier 4.04). Skips the stdlib bootstrap, enforces E1400
/// (no heap allocation), and links without the stdlib C runtime /
/// `[system_libs]`. See `ruxen_core::codegen::compile_no_std` and
/// `docs/decisions/phase4-no-std-wasm.md`.
fn run_no_std_compile(
    source: &str,
    path: &str,
    output_path: &str,
    release_mode: bool,
) -> Result<(), String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    // no_std: empty bootstrap — the hosted stdlib is not available.
    let type_result = type_check_with_package_bootstrap(&program, &[]);
    let mut errors: Vec<String> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .map(|d| d.to_string())
        .collect();

    // E1400: reject heap allocation in the no_std unit.
    for d in ruxen_core::no_std::validate(&type_result.program) {
        errors.push(d.to_string());
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }

    let borrow_errors = borrow_check::borrow_check(&type_result.program, &type_result.symbols);
    if !borrow_errors.is_empty() {
        return Err(borrow_errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join("\n"));
    }

    let mut lowerer = ruxen_core::mir::lower::Lowerer::new_no_std(&type_result.symbols);
    let mir_program = lowerer
        .lower_program(&type_result.program)
        .map_err(|e| format!("MIR lowering error: {}", e))?;

    let backend = resolve_backend(release_mode, None, None)?;
    ruxen_core::codegen::compile_no_std(&mir_program, output_path, backend)?;

    println!("Compiled {} → {} (no_std)", path, output_path);
    Ok(())
}

/// Fallback pipeline used when the user passes an --emit flag. Doesn't touch
/// the cache; emits to stdout and returns.
fn run_compile_direct(
    path: &str,
    source: &str,
    _output_path: &str,
    emit_mode: Option<&str>,
    release_mode: bool,
    backend_override: Option<&str>,
    opt_level_override: Option<&str>,
) -> Result<(), String> {
    // Bootstrap the stdlib prelude before touching user source so the
    // emit modes downstream (hir / mir / object) see the prelude-merged
    // resolver state.
    let bootstrap_programs = load_bootstrap_or_err()?;

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    if emit_mode == Some("tokens") {
        for token in &tokens {
            println!("{:?}", token);
        }
        return Ok(());
    }

    let mut parser = Parser::new(tokens);
    let program = parser.parse().map_err(|ds| {
        ds.iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    })?;

    if emit_mode == Some("ast") {
        let printer = ruxen_core::parser::printer::PrettyPrinter::new();
        println!("{}", printer.print_program(&program));
        return Ok(());
    }

    let type_result = type_check_with_package_bootstrap(&program, &bootstrap_programs);
    let has_type_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error);
    for diag in &type_result.diagnostics {
        eprintln!("{}", diag);
    }
    if has_type_errors {
        return Err("Type checking failed.".into());
    }

    if emit_mode == Some("hir") {
        for item in &type_result.program.items {
            println!("{:#?}", item);
        }
        return Ok(());
    }

    let borrow_errors = borrow_check::borrow_check(&type_result.program, &type_result.symbols);
    if !borrow_errors.is_empty() {
        for err in &borrow_errors {
            eprintln!("{}", err);
        }
        return Err(format!("{} borrow error(s) found.", borrow_errors.len()));
    }

    let mut lowerer = ruxen_core::mir::lower::Lowerer::new(&type_result.symbols);
    let mir_program = lowerer
        .lower_program(&type_result.program)
        .map_err(|e| format!("MIR lowering error: {}", e))?;

    if emit_mode == Some("mir") {
        for func in &mir_program.functions {
            println!("=== MIR function: {} ===", func.name);
            println!("  params: {:?}", func.params);
            println!("  return_ty: {:?}", func.return_ty);
            for local in &func.locals {
                println!(
                    "  local {}: {} ({:?}, mutable={})",
                    local.id, local.name, local.ty, local.mutable
                );
            }
            for block in &func.blocks {
                println!("  block {}:", block.id);
                for inst in &block.instructions {
                    println!("    {:?}", inst);
                }
                println!("    terminator: {:?}", block.terminator);
            }
        }
        return Ok(());
    }

    let _ = path;
    let _ = release_mode;
    let _ = backend_override;
    let _ = opt_level_override;
    Ok(())
}

/// Resolve which backend to use based on CLI flags.
fn resolve_backend(
    release: bool,
    backend_override: Option<&str>,
    opt_level_str: Option<&str>,
) -> Result<ruxen_core::codegen::Backend, String> {
    let _opt_level: u8 = match opt_level_str {
        Some("0") => 0,
        Some("1") => 1,
        Some("2") => 2,
        Some("3") => 3,
        Some("s") => 4,
        Some("z") => 5,
        _ => {
            if release {
                2
            } else {
                0
            }
        }
    };

    match backend_override {
        Some("cranelift") => Ok(ruxen_core::codegen::Backend::Cranelift),
        Some("llvm") => {
            #[cfg(feature = "llvm")]
            {
                Ok(ruxen_core::codegen::Backend::Llvm {
                    opt_level: _opt_level,
                })
            }
            #[cfg(not(feature = "llvm"))]
            {
                Err(
                    "LLVM backend not available. Install LLVM 18 and rebuild with --features llvm."
                        .into(),
                )
            }
        }
        _ => {
            if release {
                #[cfg(feature = "llvm")]
                {
                    Ok(ruxen_core::codegen::Backend::Llvm {
                        opt_level: _opt_level,
                    })
                }
                #[cfg(not(feature = "llvm"))]
                {
                    Err("LLVM backend not available. Install LLVM 18 and rebuild with --features llvm.".into())
                }
            } else {
                Ok(ruxen_core::codegen::Backend::Cranelift)
            }
        }
    }
}

/// Locate the `target/ruxen/` directory for the current project.
///
/// We walk upward from the cwd looking for a `Cargo.toml` or `ruxen.toml` to
/// anchor the project; if none is found, we fall back to `./target/ruxen/`.
pub(crate) fn project_target_ruxen() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut p = cwd.as_path();
    loop {
        if p.join("Cargo.toml").exists() || p.join("ruxen.toml").exists() {
            return p.join("target").join("ruxen");
        }
        match p.parent() {
            Some(parent) => p = parent,
            None => break,
        }
    }
    PathBuf::from("./target/ruxen")
}

/// Fingerprint of the running compiler binary, folded into the cache flags
/// so the incremental cache is keyed on the ACTUAL toolchain identity, not
/// just `CARGO_PKG_VERSION` (Q24).
///
/// `compiler_version()` is derived from the crate version + a schema tag, so
/// it does NOT change when the toolchain is rebuilt from source at the same
/// version (the `ruxen upgrade --from-source` dev loop) or when an embedded-
/// stdlib `.rx`/`.c` body changes (those are baked into the binary). The
/// binary's path + size + mtime DO change across any rebuild, so hashing them
/// invalidates the cache and forces a recompile — which re-runs the new
/// compiler's borrow/move analysis and re-emits fresh diagnostics, instead of
/// replaying a stale object whose `E1001`/`E1009` spans no longer match the
/// current source. Falls back to a constant when the exe path / metadata is
/// unavailable (worst case: behaves like today, no regression).
fn toolchain_fingerprint() -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::UNIX_EPOCH;

    let mut h = DefaultHasher::new();
    // Build-time version constant first, so a normal version bump still
    // participates even if the exe metadata read fails.
    env!("CARGO_PKG_VERSION").hash(&mut h);
    if let Ok(exe) = env::current_exe() {
        exe.hash(&mut h);
        if let Ok(meta) = fs::metadata(&exe) {
            meta.len().hash(&mut h);
            if let Ok(modified) = meta.modified() {
                if let Ok(dur) = modified.duration_since(UNIX_EPOCH) {
                    dur.as_nanos().hash(&mut h);
                }
            }
        }
    }
    h.finish()
}
