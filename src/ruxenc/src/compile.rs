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

use ruxen_core::codegen::{self, target::ResolvedTarget, Backend, LinkPlan};
use ruxen_core::driver::{self, Frontend, Options, Stage};
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::nodes::MirProgram;
use ruxen_core::parser::ast::Program;

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
    let mut emit_mode: Option<EmitMode> = None;
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
        } else if let Some(mode) = args[i].strip_prefix("--emit=") {
            emit_mode = Some(EmitMode::parse(mode)?);
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
            return Err(format!(
                "unknown `compile` option `{}` (expected -o <path>, --target <triple>, \
                 --emit=<{}>, --release, --backend=, --opt-level=, --no-std, --runtime-c=, \
                 --link-arg=, --force, --verbose)",
                args[i],
                EmitMode::NAMES.join("|")
            ));
        }
    }

    let output_path = output_path.unwrap_or_else(|| path.replace(".rx", ""));

    let entry = PathBuf::from(path);
    let current_dir = env::current_dir()
        .map_err(|error| format!("failed to get current directory: {}", error))?;
    let entry = if entry.is_absolute() {
        entry
    } else {
        current_dir.join(entry)
    };
    let entry_dir = entry
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let mut load_paths = vec![entry_dir];
    if !load_paths.contains(&current_dir) {
        load_paths.push(current_dir);
    }
    let source = ruxen_core::module_loader::ModuleLoader::new(load_paths)
        .load(&entry)?
        .source;

    // Resolve the cross-compilation target (tier 4.02). `None`/host keeps the
    // byte-identical cached host path below; a non-host target takes the
    // dedicated cross path (no incremental cache — one-shot compile + cross
    // link), so the host cache key is never perturbed by this feature.
    let resolved_target = ResolvedTarget::resolve(target_flag.as_deref())?;
    let options = Options::for_target(&resolved_target, no_std);

    // Emit modes short-circuit the cache: they don't produce a binary, so
    // caching them is meaningless and would add complexity. Run the
    // classic single-shot pipeline for these.
    if let Some(mode) = emit_mode {
        return run_compile_direct(&source, path, mode, &options);
    }
    let backend = Backend::select(
        release_mode,
        backend_override.as_deref(),
        opt_level_override.as_deref(),
        &resolved_target,
    )?;

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
            &options,
            &output_path,
            backend,
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
        return run_no_std_compile(&source, path, &options, &output_path, backend);
    }

    // ─── Cached compile path ─────────────────────────────────────
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let store = CacheStore::new(project_target_ruxen(&cwd));
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
        "backend={:?} release={} runtime_c={:x} link_args={} toolchain={:x}",
        backend,
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
    };

    let compile_one = move |f: &SourceFile| -> Result<CompileOutput, String> {
        compile_to_object(&f.source, &f.path, &options, backend)
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

    // User-side runtime C (`--runtime-c=`) and dependency `[system_libs]`
    // flags (`--link-arg=`, Q32), forwarded by `ruxen test`. A process that
    // opts into the test-only stdlib (`RUXEN_TEST_STDLIB`) links that
    // stdlib's fixture runtime C the same way.
    let mut extra_runtime_sources: Vec<PathBuf> =
        extra_runtime_c.iter().map(PathBuf::from).collect();
    if let Some(root) = ruxen_core::resolve::bootstrap::test_stdlib_root() {
        extra_runtime_sources.extend(ruxen_core::resolve::bootstrap::test_stdlib_runtime_sources(
            &root,
        ));
    }
    codegen::link_host(
        &object_bytes,
        &output_path,
        &LinkPlan {
            extra_link_flags: &extra_link_args,
            extra_runtime_sources: &extra_runtime_sources,
            ..LinkPlan::default()
        },
    )
    .map_err(|e| format!("Linking failed: {}", e))?;

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
        }
    }
}

/// Parse `source`; the errors are rendered with `path` on each line.
pub fn parse(source: &str, path: &str) -> Result<Program, String> {
    driver::parse(source).map_err(|ds| driver::render_diagnostics(&ds, Some(Path::new(path))))
}

/// Report what [`driver::check`] found: every error of every stage, rendered
/// by [`Frontend::render`] with `path` on each line, is the `Err`; with no
/// error the warnings go to stderr.
fn report(frontend: &Frontend, path: &str) -> Result<(), String> {
    let rendered = frontend.render(Some(Path::new(path)));
    if frontend.has_errors() {
        return Err(rendered);
    }
    if !rendered.is_empty() {
        eprintln!("{rendered}");
    }
    Ok(())
}

/// Run the whole front end on `program` and take its MIR program: the one
/// front-end step of `ruxen compile`, `ruxen check` and `ruxen build`, so
/// all three report the same errors in the same format.
pub fn front_end(
    program: &Program,
    path: &str,
    options: &Options,
) -> Result<(Frontend, MirProgram), String> {
    let mut frontend = driver::check(program, options);
    report(&frontend, path)?;
    let mir = frontend
        .mir
        .take()
        .ok_or("internal error: the front end reported no error but produced no MIR")?;
    Ok((frontend, mir))
}

/// Compile one source string into its object bytes plus a public signature.
///
/// Returns an error string containing all diagnostics on pipeline failure. The
/// caller (the cache driver) propagates this upwards without any recovery —
/// the cache layer is not responsible for compiler error reporting policy.
fn compile_to_object(
    source: &str,
    path: &str,
    options: &Options,
    backend: Backend,
) -> Result<CompileOutput, String> {
    let (frontend, mir_program) = front_end(&parse(source, path)?, path, options)?;
    Ok(CompileOutput {
        object_bytes: backend.emit_object(&mir_program, &ResolvedTarget::host())?,
        signature: extract_signature(frontend.hir()),
    })
}

/// Cross-compilation path (tier 4.02): runs the full front end once and links
/// via `compile_with_options_for_target`.
///
/// Bypasses the incremental object cache (cross builds are infrequent and the
/// cross link is one-shot), so the host cache key is never perturbed by a
/// `--target`.
#[allow(clippy::too_many_arguments)]
fn run_cross_compile(
    source: &str,
    path: &str,
    options: &Options,
    output_path: &str,
    backend: Backend,
    target: &ResolvedTarget,
    extra_runtime_c: &[String],
    extra_link_args: &[String],
) -> Result<(), String> {
    // The stdlib is `Options::for_target`'s choice: the curated wasm
    // subset for a wasm target (tier 4.09), none for a no_std unit.
    let (_, mir_program) = front_end(&parse(source, path)?, path, options)?;
    let extra_runtime_paths: Vec<PathBuf> = extra_runtime_c.iter().map(PathBuf::from).collect();

    codegen::compile_with_options_for_target(
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
        path,
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
    options: &Options,
    output_path: &str,
    backend: Backend,
) -> Result<(), String> {
    let (_, mir_program) = front_end(&parse(source, path)?, path, options)?;
    codegen::compile_no_std(&mir_program, output_path, backend)?;

    println!("Compiled {} → {} (no_std)", path, output_path);
    Ok(())
}

/// An `--emit=<mode>` value: which intermediate form to print to stdout
/// instead of producing a binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EmitMode {
    Tokens,
    Ast,
    Hir,
    Mir,
}

impl EmitMode {
    /// Every accepted `--emit=` value, in pipeline order.
    const NAMES: [&'static str; 4] = ["tokens", "ast", "hir", "mir"];

    /// Parse an `--emit=` value; an unknown one is an error, never a no-op.
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "tokens" => Ok(Self::Tokens),
            "ast" => Ok(Self::Ast),
            "hir" => Ok(Self::Hir),
            "mir" => Ok(Self::Mir),
            other => Err(format!(
                "unknown --emit value `{}` (expected one of: {})",
                other,
                Self::NAMES.join(", ")
            )),
        }
    }
}

/// Pipeline used when the user passes an --emit flag. Doesn't touch the
/// cache; prints the requested stage to stdout and returns.
fn run_compile_direct(
    source: &str,
    path: &str,
    emit_mode: EmitMode,
    options: &Options,
) -> Result<(), String> {
    if emit_mode == EmitMode::Tokens {
        let tokens = Lexer::new(source)
            .tokenize()
            .map_err(|ds| driver::render_diagnostics(&ds, Some(Path::new(path))))?;
        for token in &tokens {
            println!("{:?}", token);
        }
        return Ok(());
    }

    let program = parse(source, path)?;
    if emit_mode == EmitMode::Ast {
        println!("{program:#?}");
        return Ok(());
    }

    if emit_mode == EmitMode::Hir {
        let typeck_only = Options {
            stop_after: Stage::Typeck,
            ..*options
        };
        let frontend = driver::check(&program, &typeck_only);
        report(&frontend, path)?;
        for item in &frontend.hir().items {
            println!("{:#?}", item);
        }
        return Ok(());
    }

    let (_, mir_program) = front_end(&program, path, options)?;
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
    Ok(())
}

/// Find the project root: the nearest ancestor of the cwd holding a
/// `Ruxen.toml`, which inside a workspace is the nearest member, not the
/// workspace root. The one finder behind `ruxen build`/`run`/`check`/`clean`
/// and `ruxen test`.
pub fn find_project_root() -> Result<PathBuf, String> {
    let cwd = env::current_dir().map_err(|e| format!("failed to get current directory: {}", e))?;
    nearest_ancestor_with(&cwd, &["Ruxen.toml"]).ok_or_else(|| {
        "could not find `Ruxen.toml` in this directory or any parent directory".to_string()
    })
}

/// Locate the `target/ruxen/` directory (the incremental cache) for the
/// project containing `start`.
///
/// The anchor is the nearest ancestor with a `Ruxen.toml`, the same root
/// `ruxen clean` removes `target/` under, or a `Cargo.toml` for a single file
/// compiled inside a Cargo checkout; with neither, `./target/ruxen/`.
pub(crate) fn project_target_ruxen(start: &Path) -> PathBuf {
    match nearest_ancestor_with(start, &["Ruxen.toml", "Cargo.toml"]) {
        Some(root) => root.join("target").join("ruxen"),
        None => PathBuf::from("./target/ruxen"),
    }
}

/// The nearest of `start` and its ancestors that contains any of `manifests`.
fn nearest_ancestor_with(start: &Path, manifests: &[&str]) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| manifests.iter().any(|m| dir.join(m).exists()))
        .map(Path::to_path_buf)
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

#[cfg(test)]
mod tests {
    use super::run;

    fn args(extra: &[&str]) -> Vec<String> {
        ["ruxenc", "missing.rx"]
            .iter()
            .chain(extra)
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn unknown_option_is_an_error() {
        let err = run(&args(&["--bogus"])).expect_err("unknown flag accepted");
        assert!(err.contains("unknown `compile` option `--bogus`"), "{err}");
    }

    #[test]
    fn unknown_emit_value_is_an_error() {
        let err = run(&args(&["--emit=object"])).expect_err("unknown --emit accepted");
        assert!(err.contains("unknown --emit value `object`"), "{err}");
    }

    #[test]
    fn cache_root_is_the_nearest_ruxen_toml_project() {
        // Created under the repo's `tmp/`, so an outer `Cargo.toml` is an
        // ancestor too: the nearer `Ruxen.toml` must win.
        let repo_tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
        std::fs::create_dir_all(&repo_tmp).unwrap();
        let project = tempfile::tempdir_in(&repo_tmp).unwrap();
        std::fs::write(project.path().join("Ruxen.toml"), "[package]\n").unwrap();
        let nested = project.path().join("src").join("nested");
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(
            super::project_target_ruxen(&nested),
            project.path().join("target").join("ruxen")
        );
    }
}
