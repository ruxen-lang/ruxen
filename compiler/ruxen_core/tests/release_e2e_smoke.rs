//! Walk every fixture in `tests/release-e2e/cases/*.rx`, compile with
//! the in-process codegen pipeline, run the binary, and diff stdout
//! against the matching `tests/release-e2e/expected/*.out`. Acts as a
//! cargo-test stand-in for the shell-based release harness so changes
//! that affect codegen (drop elaboration, runtime layout, …) are caught
//! by `cargo test -p ruxen-core`.

use ruxen_core::codegen;
use ruxen_core::diagnostics::Diagnostic;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::ast::Program;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Hard wall-clock budget for any single fixture's binary. A hung
/// runtime (e.g. spawn-join with an unfinished executor) must surface
/// as a test FAILURE, not as a six-hour CI job. 30s leaves comfortable
/// margin for the slowest legitimate fixture.
const FIXTURE_WALL_CLOCK_TIMEOUT: Duration = Duration::from_secs(30);

struct RunOutcome {
    stdout: String,
    stderr: String,
    exit: Option<i32>,
    timed_out: bool,
}

fn run_with_timeout(bin_path: &PathBuf) -> std::io::Result<RunOutcome> {
    let mut child = Command::new(bin_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let started = Instant::now();
    let mut timed_out = false;
    let exit = loop {
        match child.try_wait()? {
            Some(status) => break Some(status),
            None => {
                if started.elapsed() >= FIXTURE_WALL_CLOCK_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    let mut stdout = String::new();
    if let Some(mut h) = child.stdout.take() {
        let _ = h.read_to_string(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut h) = child.stderr.take() {
        let _ = h.read_to_string(&mut stderr);
    }
    Ok(RunOutcome {
        stdout,
        stderr,
        exit: exit.and_then(|s| s.code()),
        timed_out,
    })
}

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

struct CaseOutcome {
    name: String,
    ok: bool,
    detail: String,
}

/// `typeck::type_check`-equivalent that takes a pre-loaded bootstrap
/// package list. The harness loads BOOTSTRAP_FILES once (27 stdlib
/// files × ~25 ms = ~700 ms) and reuses the parse tree across every
/// fixture; calling `type_check` directly would re-parse the stdlib on
/// every iteration (~6 min wasted in the 291-fixture sweep).
fn type_check_with_shared_bootstrap(
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

fn compile_and_run(case_name: &str, bootstrap_packages: &[(String, Program)]) -> CaseOutcome {
    let root = workspace_root();
    let src_path = root.join(format!("tests/release-e2e/cases/{}.rx", case_name));
    let expected_path = root.join(format!("tests/release-e2e/expected/{}.out", case_name));
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("e2e_{}.bin", case_name));

    let source = match std::fs::read_to_string(&src_path) {
        Ok(s) => s,
        Err(e) => {
            return CaseOutcome {
                name: case_name.to_string(),
                ok: false,
                detail: format!("read source: {}", e),
            }
        }
    };

    let mut lexer = Lexer::new(&source);
    let tokens = match lexer.tokenize() {
        Ok(t) => t,
        Err(e) => {
            return CaseOutcome {
                name: case_name.to_string(),
                ok: false,
                detail: format!("lex: {:?}", e),
            }
        }
    };
    let mut parser = Parser::new(tokens);
    let program = match parser.parse() {
        Ok(p) => p,
        Err(e) => {
            return CaseOutcome {
                name: case_name.to_string(),
                ok: false,
                detail: format!("parse: {:?}", e),
            }
        }
    };
    let result = type_check_with_shared_bootstrap(&program, bootstrap_packages);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    if !errors.is_empty() {
        let msgs: Vec<String> = errors
            .iter()
            .take(5)
            .map(|d| d.message.to_string())
            .collect();
        return CaseOutcome {
            name: case_name.to_string(),
            ok: false,
            detail: format!("typecheck: {} errors: {}", errors.len(), msgs.join(" | ")),
        };
    }

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = match lowerer.lower_program(&result.program) {
        Ok(m) => m,
        Err(e) => {
            return CaseOutcome {
                name: case_name.to_string(),
                ok: false,
                detail: format!("MIR lowering: {}", e),
            }
        }
    };

    if let Err(e) = codegen::compile(&mir, bin_path.to_str().unwrap()) {
        return CaseOutcome {
            name: case_name.to_string(),
            ok: false,
            detail: format!("codegen: {}", e),
        };
    }

    // Compile-only when no expected file exists.
    let expected = match std::fs::read_to_string(&expected_path) {
        Ok(s) => s,
        Err(_) => {
            return CaseOutcome {
                name: case_name.to_string(),
                ok: true,
                detail: String::from("compile-only"),
            }
        }
    };

    let outcome = match run_with_timeout(&bin_path) {
        Ok(o) => o,
        Err(e) => {
            return CaseOutcome {
                name: case_name.to_string(),
                ok: false,
                detail: format!("run: {}", e),
            }
        }
    };
    if outcome.timed_out {
        return CaseOutcome {
            name: case_name.to_string(),
            ok: false,
            detail: format!(
                "timed out after {:?}\n  partial stdout: {}\n  partial stderr: {}",
                FIXTURE_WALL_CLOCK_TIMEOUT,
                outcome.stdout.escape_debug(),
                outcome.stderr.escape_debug()
            ),
        };
    }
    if outcome.stdout != expected {
        return CaseOutcome {
            name: case_name.to_string(),
            ok: false,
            detail: format!(
                "stdout mismatch (exit={:?})\n  got: {}\n  expected: {}\n  stderr: {}",
                outcome.exit,
                outcome.stdout.escape_debug(),
                expected.escape_debug(),
                outcome.stderr.escape_debug()
            ),
        };
    }
    CaseOutcome {
        name: case_name.to_string(),
        ok: true,
        detail: String::new(),
    }
}

// Gated with `#[ignore]`: 223 fixtures × in-process compile-and-run
// pushes a workspace `cargo test` to ~1h on PR runners, so this is
// kept off the default suite (PRs) and is only invoked on the
// post-merge `release-e2e` job in `.github/workflows/ci.yml` (and
// any time a developer runs `cargo test ... -- --ignored` locally).
//
// Selective runs:
//
// Set `RUXEN_E2E_CASES=072_const_generic_array_arithmetic,073_…` to
// restrict the harness to a comma-separated subset of case stems
// (the filename without `.rx`).  Whitespace around entries is
// trimmed; empty entries are ignored.  An unknown case name fails
// the run with a clear message so typos don't silently skip cases.
//
// Workflow:
//   - per-commit: skip the harness entirely (it's `#[ignore]`-gated).
//   - new fixture: `RUXEN_E2E_CASES=NAME cargo test --test
//     release_e2e_smoke -- --ignored` runs just that one (~1s).
//   - phase / tier completion: `cargo test --test release_e2e_smoke
//     -- --ignored` runs the full sweep (~3 min).
#[test]
#[ignore]
fn release_e2e_all_fixtures() {
    let cases_dir = workspace_root().join("tests/release-e2e/cases");
    let mut names: Vec<String> = std::fs::read_dir(&cases_dir)
        .unwrap_or_else(|e| panic!("read {}: {}", cases_dir.display(), e))
        .filter_map(|e| e.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rx") {
                return None;
            }
            path.file_stem().and_then(|s| s.to_str()).map(String::from)
        })
        .collect();
    names.sort();

    // Apply the RUXEN_E2E_CASES filter, if set.  An unknown
    // requested case is a hard error — better to surface a typo
    // immediately than to silently skip the case the developer was
    // trying to verify.
    if let Ok(filter) = std::env::var("RUXEN_E2E_CASES") {
        let requested: Vec<String> = filter
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !requested.is_empty() {
            let discovered: std::collections::HashSet<&str> =
                names.iter().map(|s| s.as_str()).collect();
            let missing: Vec<&str> = requested
                .iter()
                .map(|s| s.as_str())
                .filter(|n| !discovered.contains(n))
                .collect();
            if !missing.is_empty() {
                panic!(
                    "RUXEN_E2E_CASES requested unknown case(s): {}\n\
                     (case stem is the filename without `.rx`; \
                     check `tests/release-e2e/cases/`)",
                    missing.join(", ")
                );
            }
            let keep: std::collections::HashSet<String> = requested.into_iter().collect();
            names.retain(|n| keep.contains(n));
            eprintln!(
                "release-e2e: RUXEN_E2E_CASES filter active, running {} case(s): {}",
                names.len(),
                names.join(", ")
            );
        }
    }

    // Load BOOTSTRAP_FILES ONCE up-front. `type_check` would otherwise
    // re-parse the entire stdlib (~27 files) for every fixture — a
    // ~6-minute fixed cost on the 291-fixture sweep.
    let mut bootstrap_diagnostics: Vec<Diagnostic> = Vec::new();
    let bootstrap_packages = ruxen_core::resolve::bootstrap::run_bootstrap_with_package_names(
        &mut bootstrap_diagnostics,
    );
    if !bootstrap_diagnostics.is_empty() {
        panic!(
            "release-e2e: stdlib bootstrap emitted diagnostics during \
             pre-load — fix the stdlib before re-running. Got: {:?}",
            bootstrap_diagnostics
        );
    }

    // Run fixtures in parallel. Each `compile_and_run` is independent:
    // unique bin path keyed on case name, codegen creates a fresh
    // Cranelift module per call, link step spawns subprocesses. Worker
    // count caps at min(num_cpus, fixtures, 16) — diminishing returns
    // past ~16 because the bottleneck is the linker (which itself
    // spawns subprocesses).
    let worker_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(names.len().max(1))
        .min(16);
    let names_arc = std::sync::Arc::new(names.clone());
    let bootstrap_arc = std::sync::Arc::new(bootstrap_packages);
    let next_idx = std::sync::atomic::AtomicUsize::new(0);
    let outcomes: Vec<CaseOutcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..worker_count)
            .map(|_| {
                let names = names_arc.clone();
                let bootstrap = bootstrap_arc.clone();
                let next_idx = &next_idx;
                scope.spawn(move || {
                    let mut local: Vec<CaseOutcome> = Vec::new();
                    loop {
                        let i = next_idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= names.len() {
                            break;
                        }
                        local.push(compile_and_run(&names[i], &bootstrap));
                    }
                    local
                })
            })
            .collect();
        let mut all: Vec<CaseOutcome> = Vec::with_capacity(names.len());
        for h in handles {
            all.extend(h.join().expect("worker panicked"));
        }
        all
    });

    let pass = outcomes.iter().filter(|o| o.ok).count();
    let fail = outcomes.iter().filter(|o| !o.ok).count();
    eprintln!(
        "release-e2e: {} pass, {} fail / {} total",
        pass,
        fail,
        outcomes.len()
    );
    if fail > 0 {
        let failures: Vec<String> = outcomes
            .iter()
            .filter(|o| !o.ok)
            .map(|o| format!("  - {}: {}", o.name, o.detail))
            .collect();
        panic!(
            "{} release-e2e fixture(s) failed:\n{}",
            fail,
            failures.join("\n")
        );
    }
}
