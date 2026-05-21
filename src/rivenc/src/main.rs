use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process;

use riven_core::borrow_check;
use riven_core::diagnostics::Diagnostic;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::resolve::bootstrap as stdlib_bootstrap;
use riven_core::typeck;

use rivenc::cache;
use rivenc::cache::{
    build as cache_build, clear_global_cache, extract_signature, BuildOptions, CacheStore,
    CompileOutput, FileStatus, SourceFile,
};

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    match args[1].as_str() {
        "fmt" => run_fmt(&args[2..]),
        "clean" => run_clean(&args[2..]),
        "bench" => run_bench(&args[2..]),
        "--version" | "-V" => {
            println!("rivenc {}", env!("CARGO_PKG_VERSION"));
        }
        "--help" | "-h" => {
            print_usage();
        }
        _ => run_compile(&args),
    }
}

fn print_usage() {
    eprintln!("Usage: rivenc <file.rvn> [options]");
    eprintln!("       rivenc fmt [options] [files...]");
    eprintln!("       rivenc clean [--global]");
    eprintln!();
    eprintln!("Compiler options:");
    eprintln!("  -o <output>           Specify output file name");
    eprintln!("  --emit=tokens         Dump lexer tokens");
    eprintln!("  --emit=ast            Dump parsed AST");
    eprintln!("  --emit=hir            Dump typed HIR");
    eprintln!("  --emit=mir            Dump MIR");
    eprintln!("  --release             Use LLVM backend with O2 optimization");
    eprintln!("  --backend=cranelift   Force Cranelift backend");
    eprintln!("  --backend=llvm        Force LLVM backend");
    eprintln!("  --opt-level=0|1|2|3|s|z  Set optimization level");
    eprintln!("  --force               Ignore all caches and recompile from scratch");
    eprintln!("  --verbose             Emit [cache] log lines");
    eprintln!();
    eprintln!("Formatter options:");
    eprintln!("  fmt                     Format all .rvn files recursively from cwd");
    eprintln!("  fmt <file> [<file>...]  Format specific files");
    eprintln!("  fmt --check             Exit 1 if any file would change (CI mode)");
    eprintln!("  fmt --diff              Show diff of what would change");
    eprintln!("  fmt --stdin             Read stdin, write formatted to stdout");
    eprintln!();
    eprintln!("Clean options:");
    eprintln!("  clean                   Delete target/riven/incremental/ for this project");
    eprintln!("  clean --global          Delete the global ~/.cache/riven/ cache");
    eprintln!();
    eprintln!("Bench options:");
    eprintln!("  bench <file.rvn>        Run every `def bench_*(b: &var Bencher)` in the file");
    eprintln!("  bench <file.rvn> --filter <pat>");
    eprintln!("                          Only run benches whose name contains <pat>");
    eprintln!("  bench <file.rvn> --iter-hint <N>");
    eprintln!("                          Initial iteration count (default 100, auto-scales");
    eprintln!("                          until total wall time ≥ 100ms)");
}

// ─── Formatter CLI ──────────────────────────────────────────────────

fn run_fmt(args: &[String]) {
    let mut check_mode = false;
    let mut diff_mode = false;
    let mut stdin_mode = false;
    let mut filepath: Option<String> = None;
    let mut files: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--check" => check_mode = true,
            "--diff" => diff_mode = true,
            "--stdin" => stdin_mode = true,
            s if s.starts_with("--filepath=") => {
                filepath = Some(s[11..].to_string());
            }
            s if s.starts_with("--") => {
                eprintln!("Unknown option: {}", s);
                process::exit(2);
            }
            _ => files.push(args[i].clone()),
        }
        i += 1;
    }

    if stdin_mode {
        run_fmt_stdin(filepath.as_deref());
        return;
    }

    // If no files specified, discover all .rvn files recursively
    if files.is_empty() {
        files = discover_rvn_files(".");
        if files.is_empty() {
            eprintln!("No .rvn files found.");
            process::exit(0);
        }
    }

    let mut any_changed = false;
    let mut any_errors = false;

    for path in &files {
        let source = match fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error reading '{}': {}", path, e);
                any_errors = true;
                continue;
            }
        };

        let result = riven_core::formatter::format(&source);

        if !result.errors.is_empty() {
            for err in &result.errors {
                eprintln!("{}: {}", path, err);
            }
            any_errors = true;
            continue;
        }

        if result.changed {
            any_changed = true;

            if check_mode {
                println!("{}", path);
            } else if diff_mode {
                print_diff(path, &source, &result.output);
            } else {
                // Write formatted output back to file
                if let Err(e) = fs::write(path, &result.output) {
                    eprintln!("Error writing '{}': {}", path, e);
                    any_errors = true;
                } else {
                    println!("Formatted {}", path);
                }
            }
        }
    }

    if any_errors {
        process::exit(2);
    }
    if check_mode && any_changed {
        process::exit(1);
    }
}

fn run_fmt_stdin(filepath: Option<&str>) {
    let mut source = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut source) {
        eprintln!("Error reading stdin: {}", e);
        process::exit(2);
    }

    let result = riven_core::formatter::format(&source);

    if !result.errors.is_empty() {
        let label = filepath.unwrap_or("<stdin>");
        for err in &result.errors {
            eprintln!("{}: {}", label, err);
        }
    }

    print!("{}", result.output);
}

fn discover_rvn_files(dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    discover_rvn_files_recursive(Path::new(dir), &mut files);
    files.sort();
    files
}

fn discover_rvn_files_recursive(dir: &Path, files: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden directories and common ignore patterns
        if name_str.starts_with('.') || name_str == "target" || name_str == "node_modules" {
            continue;
        }

        if path.is_dir() {
            discover_rvn_files_recursive(&path, files);
        } else if path.extension().is_some_and(|ext| ext == "rvn") {
            if let Some(s) = path.to_str() {
                files.push(s.to_string());
            }
        }
    }
}

fn print_diff(path: &str, original: &str, formatted: &str) {
    println!("--- {}", path);
    println!("+++ {}", path);

    let orig_lines: Vec<&str> = original.lines().collect();
    let fmt_lines: Vec<&str> = formatted.lines().collect();

    // Simple line-by-line diff
    let max_lines = orig_lines.len().max(fmt_lines.len());
    let mut in_hunk = false;
    let mut hunk_start = 0;

    for i in 0..max_lines {
        let orig_line = orig_lines.get(i).copied().unwrap_or("");
        let fmt_line = fmt_lines.get(i).copied().unwrap_or("");

        if orig_line != fmt_line {
            if !in_hunk {
                hunk_start = i;
                let context_start = i.saturating_sub(2);
                println!(
                    "@@ -{},{} +{},{} @@",
                    context_start + 1,
                    3.min(orig_lines.len() - context_start),
                    context_start + 1,
                    3.min(fmt_lines.len() - context_start)
                );
                // Context lines before
                for j in context_start..i {
                    if let Some(l) = orig_lines.get(j) {
                        println!(" {}", l);
                    }
                }
                in_hunk = true;
            }
            if i < orig_lines.len() {
                println!("-{}", orig_line);
            }
            if i < fmt_lines.len() {
                println!("+{}", fmt_line);
            }
        } else {
            if in_hunk {
                // Print some context after the hunk
                println!(" {}", orig_line);
                if i - hunk_start > 5 {
                    in_hunk = false;
                }
            }
        }
    }
}

// ─── Clean CLI ──────────────────────────────────────────────────────

fn run_clean(args: &[String]) {
    if args.iter().any(|a| a == "--global") {
        match clear_global_cache() {
            Ok(()) => println!(
                "Cleaned global cache at {}",
                cache::global_cache_dir().display()
            ),
            Err(e) => {
                eprintln!("Failed to clean global cache: {}", e);
                process::exit(1);
            }
        }
        return;
    }

    let store = CacheStore::new(project_target_riven());
    match store.clear() {
        Ok(()) => println!("Cleaned {}", store.incremental_dir().display()),
        Err(e) => {
            eprintln!("Failed to clean cache: {}", e);
            process::exit(1);
        }
    }
}

// ─── Bench CLI ──────────────────────────────────────────────────────
//
// `rivenc bench <file.rvn>` finds every `def bench_*(b: &var Bencher)`
// in the file, appends a synthesised `def main` that runs each one
// against a fresh `Bencher`, and compiles+runs the result. Pure-Riven
// harness — see `library/std/bench/src/lib.rvn`. No compiler-side
// parser/keyword work; bench fns are identified by name convention
// (`bench_*`) per feedback_pure_riven_first.md.

fn run_bench(args: &[String]) {
    if args.is_empty() {
        eprintln!("Usage: rivenc bench <file.rvn> [--filter <pat>] [--iter-hint <N>]");
        process::exit(2);
    }

    let mut path: Option<String> = None;
    let mut filter: Option<String> = None;
    let mut iter_hint: i64 = 100;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--filter" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--filter requires a value");
                    process::exit(2);
                }
                filter = Some(args[i].clone());
            }
            "--iter-hint" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("--iter-hint requires a value");
                    process::exit(2);
                }
                iter_hint = args[i].parse().unwrap_or_else(|_| {
                    eprintln!("--iter-hint must be a positive integer");
                    process::exit(2);
                });
            }
            s if s.starts_with("--") => {
                eprintln!("Unknown bench option: {}", s);
                process::exit(2);
            }
            _ => {
                if path.is_some() {
                    eprintln!("Unexpected positional arg: {}", args[i]);
                    process::exit(2);
                }
                path = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let Some(path) = path else {
        eprintln!("rivenc bench: missing <file.rvn>");
        process::exit(2);
    };

    if !path.ends_with(".rvn") {
        eprintln!("rivenc bench: expected a .rvn file, got: {}", path);
        process::exit(2);
    }

    let source = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("rivenc bench: cannot read {}: {}", path, e);
            process::exit(1);
        }
    };

    // Parse just to collect bench-fn names. Diagnostics here are pure
    // syntax / item-shape — full typeck happens on the synthesised
    // file below, so a typo in a bench body will surface there with
    // the normal codepath.
    let mut lexer = Lexer::new(&source);
    let tokens = match lexer.tokenize() {
        Ok(t) => t,
        Err(ds) => {
            for d in ds {
                eprintln!("{}", d);
            }
            process::exit(1);
        }
    };
    let mut parser = Parser::new(tokens);
    let program = match parser.parse() {
        Ok(p) => p,
        Err(ds) => {
            for d in ds {
                eprintln!("{}", d);
            }
            process::exit(1);
        }
    };

    // Collect bench fns: name convention `bench_*`, exactly one
    // parameter. The Bencher-receiver type-check happens at the
    // real typeck pass — we filter coarsely here so a fn with the
    // wrong signature still surfaces a meaningful error from the
    // synthesised main's call site instead of being silently dropped.
    use riven_core::parser::ast::TopLevelItem;
    let mut bench_names: Vec<String> = Vec::new();
    let mut has_main = false;
    for item in &program.items {
        if let TopLevelItem::Function(f) = item {
            if f.name == "main" {
                has_main = true;
            }
            if f.name.starts_with("bench_") && f.params.len() == 1 {
                if let Some(pat) = &filter {
                    if !f.name.contains(pat.as_str()) {
                        continue;
                    }
                }
                bench_names.push(f.name.clone());
            }
        }
    }

    if has_main {
        eprintln!(
            "rivenc bench: {} declares `def main` — bench files must let the runner provide main. \
             Remove or rename the existing main and re-run.",
            path
        );
        process::exit(1);
    }

    if bench_names.is_empty() {
        eprintln!(
            "rivenc bench: no `def bench_*(b: &var Bencher)` functions found{}.",
            filter
                .as_ref()
                .map(|p| format!(" matching `{}`", p))
                .unwrap_or_default()
        );
        process::exit(1);
    }

    // Synthesise a `def main` that walks each collected bench fn
    // against a fresh `Bencher`. Appended at the end of the source
    // so resolve/typeck sees all bench-fn definitions before the
    // call sites.
    let mut synth = source.clone();
    if !synth.ends_with('\n') {
        synth.push('\n');
    }
    synth.push_str(
        "\n# ── rivenc bench: synthesised runner main ────────────────────\n",
    );
    synth.push_str("def main\n");
    synth.push_str(&format!("  var bencher = Bencher.new({})\n", iter_hint));
    for name in &bench_names {
        synth.push_str(&format!("  {}(&var bencher)\n", name));
    }
    synth.push_str("end\n");

    // Write synth to a tmp file alongside the original so error
    // messages cite a stable path; compile + run through the normal
    // pipeline.
    let tmp_dir = std::env::temp_dir().join("rivenc-bench");
    if let Err(e) = fs::create_dir_all(&tmp_dir) {
        eprintln!("rivenc bench: cannot create {}: {}", tmp_dir.display(), e);
        process::exit(1);
    }
    let stem = Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bench");
    let synth_path = tmp_dir.join(format!("{}.bench.rvn", stem));
    let bin_path = tmp_dir.join(format!("{}.bench", stem));
    if let Err(e) = fs::write(&synth_path, &synth) {
        eprintln!("rivenc bench: cannot write {}: {}", synth_path.display(), e);
        process::exit(1);
    }

    // Forward to the normal compile path, then exec the binary.
    let compile_args = vec![
        "rivenc".to_string(),
        synth_path.to_string_lossy().into_owned(),
        "-o".to_string(),
        bin_path.to_string_lossy().into_owned(),
        "--force".to_string(),
    ];
    run_compile(&compile_args);

    // Safety timeout — a stuck bench (e.g. a fixture that locks
    // without unlocking due to a drop-elaboration regression) would
    // otherwise hang `rivenc bench` indefinitely. Auto-scale within
    // `Bencher.iter` already caps total wall time at ≥ 100 ms per
    // bench, so even a slow real bench finishes in a few seconds;
    // anything past `bench_timeout` is treated as a deadlock or
    // pathological auto-scale path and killed. Override via env
    // `RIVENC_BENCH_TIMEOUT_SECS` (default 60). 0 disables.
    let bench_timeout: u64 = std::env::var("RIVENC_BENCH_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);

    let output = if bench_timeout == 0 {
        process::Command::new(&bin_path).output()
    } else {
        let mut child = match process::Command::new(&bin_path)
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!("rivenc bench: failed to spawn {}: {}", bin_path.display(), e);
                process::exit(1);
            }
        };
        let pid = child.id();
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break child.wait_with_output(),
                Ok(None) => {
                    if start.elapsed().as_secs() >= bench_timeout {
                        let _ = child.kill();
                        let _ = child.wait();
                        eprintln!(
                            "rivenc bench: bench process pid={} exceeded RIVENC_BENCH_TIMEOUT_SECS={}s and was killed. \
                             A common cause is a drop-elaboration bug that leaves a resource (mutex / fd / refcount) \
                             held across iterations — re-run the fixture standalone to bisect.",
                            pid, bench_timeout
                        );
                        process::exit(124);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => break Err(e),
            }
        }
    };

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("rivenc bench: failed to run {}: {}", bin_path.display(), e);
            process::exit(1);
        }
    };
    if !output.stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        process::exit(output.status.code().unwrap_or(1));
    }
}

// ─── Compiler CLI ───────────────────────────────────────────────────

fn run_compile(args: &[String]) {
    let path = &args[1];
    if !path.ends_with(".rvn") {
        eprintln!("Error: expected a .rvn file, got: {}", path);
        process::exit(1);
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

    let source = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading '{}': {}", path, e);
            process::exit(1);
        }
    };

    // Emit modes short-circuit the cache: they don't produce a binary, so
    // caching them is meaningless and would add complexity. Run the
    // classic single-shot pipeline for these.
    if emit_mode.is_some() {
        run_compile_direct(
            path,
            &source,
            &output_path,
            emit_mode.as_deref(),
            release_mode,
            backend_override.as_deref(),
            opt_level_override.as_deref(),
        );
        return;
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

    let result = match cache_build(files, &store, &build_opts, &compile_one) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Build failed: {}", e);
            process::exit(1);
        }
    };

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
        return;
    }

    // Gather object bytes and link.
    // rivenc is single-file today; guard against silent data loss if a
    // multi-file BuildResult ever flows through this path.
    if result.objects.len() > 1 {
        eprintln!(
            "internal error: rivenc CLI received {} objects but only supports single-file linking",
            result.objects.len()
        );
        process::exit(1);
    }
    let (_, obj_path) = match result.objects.first() {
        Some(first) => first,
        None => {
            eprintln!("No objects produced — nothing to link.");
            process::exit(1);
        }
    };
    let object_bytes = match fs::read(obj_path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("Failed to read cached object {}: {}", obj_path.display(), e);
            process::exit(1);
        }
    };

    let runtime_sources = match riven_core::codegen::find_runtime_sources() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e);
            process::exit(1);
        }
    };
    let runtime_objects =
        match riven_core::codegen::object::compile_runtime_sources(&runtime_sources, false) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("Failed to compile runtime: {}", e);
                process::exit(1);
            }
        };

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
        eprintln!("Linking failed: {}", e);
        process::exit(1);
    }
    for o in &runtime_objects {
        let _ = fs::remove_file(o);
    }

    println!("Compiled {} → {}", path, output_path);
    report_statuses(&result.statuses, verbose);
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

/// Run the stdlib bootstrap loader and, on any diagnostic, print every
/// one with a clear `stdlib bootstrap failed:` header and exit
/// non-zero. The compiler cannot make progress without a clean prelude
/// — a missing or broken stdlib file is a fatal install/build issue,
/// not a recoverable user-source error.
///
/// Wave 1.5: returns the parsed `_bootstrap_smoke.rvn` (or whatever
/// [`stdlib_bootstrap::BOOTSTRAP_FILES`] lists). Wave 2+ will return
/// the full self-hosted stdlib prelude.
fn load_bootstrap_or_exit() -> Vec<riven_core::parser::ast::Program> {
    let mut bootstrap_diags: Vec<Diagnostic> = Vec::new();
    let programs = stdlib_bootstrap::run_bootstrap(&mut bootstrap_diags);
    if !bootstrap_diags.is_empty() {
        eprintln!("stdlib bootstrap failed:");
        for d in &bootstrap_diags {
            eprintln!("  {}", d);
        }
        process::exit(1);
    }
    programs
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
    // [`load_bootstrap_or_exit`].
    let bootstrap_programs = load_bootstrap_or_exit();

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

    let backend = resolve_backend(release_mode, backend_override, opt_level_override);
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
/// the cache; emits to stdout and exits.
fn run_compile_direct(
    path: &str,
    source: &str,
    _output_path: &str,
    emit_mode: Option<&str>,
    release_mode: bool,
    backend_override: Option<&str>,
    opt_level_override: Option<&str>,
) {
    // Bootstrap the stdlib prelude before touching user source so the
    // emit modes downstream (hir / mir / object) see the prelude-merged
    // resolver state. A broken stdlib aborts via
    // [`load_bootstrap_or_exit`] — same policy as the cached path.
    let bootstrap_programs = load_bootstrap_or_exit();

    let mut lexer = Lexer::new(source);
    let tokens = match lexer.tokenize() {
        Ok(tokens) => tokens,
        Err(diagnostics) => {
            for diag in &diagnostics {
                eprintln!("{}", diag);
            }
            process::exit(1);
        }
    };

    if emit_mode == Some("tokens") {
        for token in &tokens {
            println!("{:?}", token);
        }
        return;
    }

    let mut parser = Parser::new(tokens);
    let program = match parser.parse() {
        Ok(program) => program,
        Err(diagnostics) => {
            for diag in &diagnostics {
                eprintln!("{}", diag);
            }
            process::exit(1);
        }
    };

    if emit_mode == Some("ast") {
        let printer = riven_core::parser::printer::PrettyPrinter::new();
        println!("{}", printer.print_program(&program));
        return;
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
        eprintln!("Type checking failed.");
        process::exit(1);
    }

    if emit_mode == Some("hir") {
        for item in &type_result.program.items {
            println!("{:#?}", item);
        }
        return;
    }

    let borrow_errors = borrow_check::borrow_check(&type_result.program, &type_result.symbols);
    if !borrow_errors.is_empty() {
        for err in &borrow_errors {
            eprintln!("{}", err);
        }
        eprintln!("\n{} borrow error(s) found.", borrow_errors.len());
        process::exit(1);
    }

    let mut lowerer = riven_core::mir::lower::Lowerer::new(&type_result.symbols);
    let mir_program = match lowerer.lower_program(&type_result.program) {
        Ok(mir) => mir,
        Err(e) => {
            eprintln!("MIR lowering error: {}", e);
            process::exit(1);
        }
    };

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
        return;
    }

    let _ = path;
    let _ = release_mode;
    let _ = backend_override;
    let _ = opt_level_override;
}

/// Resolve which backend to use based on CLI flags.
fn resolve_backend(
    release: bool,
    backend_override: Option<&str>,
    opt_level_str: Option<&str>,
) -> riven_core::codegen::Backend {
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
        Some("cranelift") => riven_core::codegen::Backend::Cranelift,
        Some("llvm") => {
            #[cfg(feature = "llvm")]
            {
                riven_core::codegen::Backend::Llvm {
                    opt_level: _opt_level,
                }
            }
            #[cfg(not(feature = "llvm"))]
            {
                eprintln!(
                    "LLVM backend not available. Install LLVM 18 and rebuild with --features llvm."
                );
                process::exit(1);
            }
        }
        _ => {
            if release {
                #[cfg(feature = "llvm")]
                {
                    riven_core::codegen::Backend::Llvm {
                        opt_level: _opt_level,
                    }
                }
                #[cfg(not(feature = "llvm"))]
                {
                    eprintln!("LLVM backend not available. Install LLVM 18 and rebuild with --features llvm.");
                    process::exit(1);
                }
            } else {
                riven_core::codegen::Backend::Cranelift
            }
        }
    }
}

/// Locate the `target/riven/` directory for the current project.
///
/// We walk upward from the cwd looking for a `Cargo.toml` or `riven.toml` to
/// anchor the project; if none is found, we fall back to `./target/riven/`.
fn project_target_riven() -> PathBuf {
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
