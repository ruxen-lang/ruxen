//! Single-file compile driver — the bare `rivenc <file.rvn>` path.
//!
//! Exposed as `rivenc::compile::run(&args)` so both binaries (`rivenc` and
//! `riven`) call the same code. All `process::exit(N)` sites from the
//! original main have been converted to `Err(String)`; callers exit on Err.
//!
//! Args layout (caller responsibility): `args[0]` is the program name and is
//! IGNORED — the file path lives at `args[1]`. Remaining slots are options.
//! This matches the legacy `rivenc <file> [opts...]` shape so the existing
//! call sites (and the bench module's `run` invocation) keep working.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use riven_core::borrow_check;
use riven_core::diagnostics::Diagnostic;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::resolve::bootstrap as stdlib_bootstrap;
use riven_core::typeck;

use crate::cache;
use crate::cache::{
    build as cache_build, extract_signature, BuildOptions, CacheStore, CompileOutput, FileStatus,
    SourceFile,
};

pub fn run(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err("missing input file (expected `<program> <file.rvn>`)".into());
    }
    let path = &args[1];
    if !path.ends_with(".rvn") {
        return Err(format!("expected a .rvn file, got: {}", path));
    }

    // Parse CLI options
    let mut output_path: Option<String> = None;
    let mut emit_mode: Option<String> = None;
    let mut release_mode = false;
    let mut backend_override: Option<String> = None;
    let mut opt_level_override: Option<String> = None;
    let mut force = false;
    let mut verbose = false;
    let mut i = 2;
    while i < args.len() {
        if args[i] == "-o" && i + 1 < args.len() {
            output_path = Some(args[i + 1].clone());
            i += 2;
        } else if args[i].starts_with("--emit=") {
            emit_mode = Some(args[i][7..].to_string());
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

    let output_path = output_path.unwrap_or_else(|| path.replace(".rvn", ""));

    let source =
        fs::read_to_string(path).map_err(|e| format!("Error reading '{}': {}", path, e))?;

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

    // ─── Cached compile path ─────────────────────────────────────
    let store = CacheStore::new(project_target_riven());
    let opt_level_label = if release_mode { "release" } else { "debug" };
    let flags = format!(
        "backend={} opt={} release={}",
        backend_override.as_deref().unwrap_or("default"),
        opt_level_override.as_deref().unwrap_or("default"),
        release_mode
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
    // rivenc is single-file today; guard against silent data loss if a
    // multi-file BuildResult ever flows through this path.
    if result.objects.len() > 1 {
        return Err(format!(
            "internal error: rivenc CLI received {} objects but only supports single-file linking",
            result.objects.len()
        ));
    }
    let (_, obj_path) = result
        .objects
        .first()
        .ok_or_else(|| "No objects produced — nothing to link.".to_string())?;
    let object_bytes = fs::read(obj_path)
        .map_err(|e| format!("Failed to read cached object {}: {}", obj_path.display(), e))?;

    let runtime_sources = riven_core::codegen::find_runtime_sources()?;
    let runtime_objects =
        riven_core::codegen::object::compile_runtime_sources(&runtime_sources, false)
            .map_err(|e| format!("Failed to compile runtime: {}", e))?;

    if let Err(e) = riven_core::codegen::object::emit_executable(
        &object_bytes,
        &runtime_objects,
        &output_path,
        false,
        &[],
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
fn load_bootstrap_or_err() -> Result<Vec<riven_core::parser::ast::Program>, String> {
    let mut bootstrap_diags: Vec<Diagnostic> = Vec::new();
    let programs = stdlib_bootstrap::run_bootstrap(&mut bootstrap_diags);
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

    let type_result = typeck::type_check_with_bootstrap(&program, &bootstrap_programs);
    let has_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error);
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

    let mut lowerer = riven_core::mir::lower::Lowerer::new(&type_result.symbols);
    let mir_program = lowerer
        .lower_program(&type_result.program)
        .map_err(|e| format!("MIR lowering error: {}", e))?;

    let backend = resolve_backend(release_mode, backend_override, opt_level_override)?;
    let object_bytes = match backend {
        riven_core::codegen::Backend::Cranelift => {
            let mut codegen = riven_core::codegen::cranelift::CodeGen::new()?;
            codegen.compile_program(&mir_program)?;
            codegen.finish()?
        }
        #[cfg(feature = "llvm")]
        riven_core::codegen::Backend::Llvm { opt_level } => {
            let mut codegen = riven_core::codegen::llvm::CodeGen::new(opt_level)?;
            codegen.compile_program(&mir_program)?;
            codegen.finish()?
        }
    };

    // Dependencies: today rivenc is single-file, so the dep list is empty.
    // When multi-file support is added, extract cross-file imports from the
    // resolver's type registry here.
    Ok(CompileOutput {
        object_bytes,
        signature,
        dependencies: Vec::new(),
    })
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
        let printer = riven_core::parser::printer::PrettyPrinter::new();
        println!("{}", printer.print_program(&program));
        return Ok(());
    }

    let type_result = typeck::type_check_with_bootstrap(&program, &bootstrap_programs);
    let has_type_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error);
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

    let mut lowerer = riven_core::mir::lower::Lowerer::new(&type_result.symbols);
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
) -> Result<riven_core::codegen::Backend, String> {
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
        Some("cranelift") => Ok(riven_core::codegen::Backend::Cranelift),
        Some("llvm") => {
            #[cfg(feature = "llvm")]
            {
                Ok(riven_core::codegen::Backend::Llvm {
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
                    Ok(riven_core::codegen::Backend::Llvm {
                        opt_level: _opt_level,
                    })
                }
                #[cfg(not(feature = "llvm"))]
                {
                    Err("LLVM backend not available. Install LLVM 18 and rebuild with --features llvm.".into())
                }
            } else {
                Ok(riven_core::codegen::Backend::Cranelift)
            }
        }
    }
}

/// Locate the `target/riven/` directory for the current project.
///
/// We walk upward from the cwd looking for a `Cargo.toml` or `riven.toml` to
/// anchor the project; if none is found, we fall back to `./target/riven/`.
pub(crate) fn project_target_riven() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut p = cwd.as_path();
    loop {
        if p.join("Cargo.toml").exists() || p.join("riven.toml").exists() {
            return p.join("target").join("riven");
        }
        match p.parent() {
            Some(parent) => p = parent,
            None => break,
        }
    }
    PathBuf::from("./target/riven")
}
