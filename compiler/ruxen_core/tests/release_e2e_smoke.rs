//! Walk every fixture in `tests/release-e2e/cases/*.rx`, compile with
//! the in-process codegen pipeline, run the binary, and diff stdout
//! against the matching `tests/release-e2e/expected/*.out`. Acts as a
//! cargo-test stand-in for the shell-based release harness so changes
//! that affect codegen (drop elaboration, runtime layout, …) are caught
//! by `cargo test -p ruxen-core`.

use ruxen_core::codegen;
use ruxen_core::diagnostics::Diagnostic;
use ruxen_core::driver::{self, Options};
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
        // Redirect stdin from /dev/null. Without this the fixture
        // inherits the test runner's stdin (a tty under `cargo test`,
        // or whatever cargo attaches), so a fixture that reads stdin
        // expecting EOF — e.g. 740_async_stdin_read_line_eof — never
        // sees EOF and reports `eof_fail`. A null stdin gives every
        // fixture a deterministic, immediately-EOF input and a clean
        // fd 0.
        .stdin(Stdio::null())
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

fn compile_and_run(case_name: &str) -> CaseOutcome {
    let root = workspace_root();
    let src_path = root.join(format!("tests/release-e2e/cases/{}.rx", case_name));
    let expected_path = root.join(format!("tests/release-e2e/expected/{}.out", case_name));
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!(
        "e2e_{}-{}-{}.bin",
        case_name,
        std::process::id(),
        ruxen_unique_id()
    ));

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

    let program = match driver::parse(&source) {
        Ok(p) => p,
        Err(e) => {
            return CaseOutcome {
                name: case_name.to_string(),
                ok: false,
                detail: format!("parse: {}", driver::render_diagnostics(&e, None)),
            }
        }
    };
    // A pinned case is a program `ruxen build` accepts: it passes the whole
    // front end, both borrow checkers included (I-20: a case the CLI
    // rejected used to pass here).
    let mut frontend = driver::check(&program, &Options::default());
    let Some(mir) = frontend.mir.take() else {
        return CaseOutcome {
            name: case_name.to_string(),
            ok: false,
            detail: format!("front end: {}", frontend.render(None)),
        };
    };

    let test_runtime =
        ruxen_core::resolve::bootstrap::test_stdlib_runtime_sources(&root.join("library/test_std"));
    if let Err(e) = codegen::compile_with_options(
        &mir,
        bin_path.to_str().unwrap(),
        false,
        &[],
        &test_runtime,
        e2e_backend(),
    ) {
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
    let _ = std::fs::remove_file(&bin_path);
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
// Set `RUXEN_E2E_BACKEND=llvm` (with `--features llvm`) to build every
// case with LLVM instead of Cranelift.
//
// Workflow:
//   - per-commit: skip the harness entirely (it's `#[ignore]`-gated).
//   - new fixture: `RUXEN_E2E_CASES=NAME cargo test --test
//     release_e2e_smoke -- --ignored` runs just that one (~1s).
//   - phase / tier completion: `cargo test --test release_e2e_smoke
//     -- --ignored` runs the full sweep (~3 min).
/// Locate the `libruxenrt.a` that `ruxen_repl`'s build script
/// produced under the current cargo target dir, so that the
/// in-process `codegen::compile` path takes its `RUXEN_RUNTIME_AR`
/// fast path instead of forking `cc -c` 30+ times per fixture. The
/// archive is content-identical across fixtures (the `.c` sources
/// don't change between runs), so reusing it is sound and drops the
/// 308-fixture sweep from minutes to seconds. Picks the most
/// recently modified candidate so re-runs after `cargo clean
/// -p ruxen_repl` always pick up the fresh archive.
fn find_prebuilt_runtime_archive() -> Option<PathBuf> {
    let root = workspace_root();
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for profile in ["debug", "release"] {
        let build_dir = root.join("target").join(profile).join("build");
        let entries = match std::fs::read_dir(&build_dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for ent in entries.flatten() {
            let name = ent.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("ruxen_repl-") {
                continue;
            }
            let ar = ent.path().join("out").join("libruxenrt.a");
            if let Ok(meta) = std::fs::metadata(&ar) {
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                match &best {
                    Some((t, _)) if *t >= modified => {}
                    _ => best = Some((modified, ar)),
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// The backend the sweep builds with, chosen by `Backend::select` as a
/// debug build: `RUXEN_E2E_BACKEND=llvm` selects LLVM at -O0 (needs
/// `--features llvm`); unset or `cranelift` selects Cranelift.
fn e2e_backend() -> codegen::Backend {
    let choice = std::env::var("RUXEN_E2E_BACKEND").ok();
    codegen::Backend::select(
        false,
        choice.as_deref(),
        None,
        &codegen::target::ResolvedTarget::host(),
    )
    .unwrap_or_else(|e| panic!("RUXEN_E2E_BACKEND: {e}"))
}

#[test]
#[ignore]
fn release_e2e_all_fixtures() {
    // Resolve the backend up front so a bad `RUXEN_E2E_BACKEND` fails once.
    e2e_backend();

    // Opt into the fast-path link: skip the per-fixture `cc -c`
    // forks for stdlib runtime sources and whole-archive a prebuilt
    // libruxenrt.a instead. Mirrors what `tests/release-e2e/run.sh`
    // does for the shell harness. Caller can pre-set the env var to
    // override (e.g. point at a sanitized build); we only auto-set
    // when it's empty.
    //
    // SAFETY: `set_var` is `unsafe` from Rust 2024+ because env-var
    // writes race with `getenv` across threads. This `#[ignore]`d
    // test runs single-threaded under `cargo test -- --ignored`, no
    // other reader is on the env var, and the value is set once
    // before any fixture spins up — so the documented hazard does
    // not apply here.
    if std::env::var_os("RUXEN_RUNTIME_AR").is_none() {
        if let Some(ar) = find_prebuilt_runtime_archive() {
            // SAFETY: see comment above.
            unsafe { std::env::set_var("RUXEN_RUNTIME_AR", &ar) };
            eprintln!("[fast-path] linking via {}", ar.display());
        }
    }

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

    // The stdlib parse is cached per process; loading it here, once, fails
    // the sweep up front on a broken stdlib instead of once per fixture.
    //
    // Some fixtures (434/436 stream drop, 750 trio-leak pin) use the
    // test-only stdlib; this sweep opts into it for every case, and
    // `compile_and_run` links its runtime C. `run.sh` opts in per case.
    std::env::set_var(
        ruxen_core::resolve::bootstrap::TEST_STDLIB_ENV,
        workspace_root().join("library/test_std"),
    );
    let mut bootstrap_diagnostics: Vec<Diagnostic> = Vec::new();
    ruxen_core::resolve::bootstrap::run_bootstrap_with_package_names(&mut bootstrap_diagnostics);
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
    let next_idx = std::sync::atomic::AtomicUsize::new(0);
    let outcomes: Vec<CaseOutcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..worker_count)
            .map(|_| {
                let names = names_arc.clone();
                let next_idx = &next_idx;
                scope.spawn(move || {
                    let mut local: Vec<CaseOutcome> = Vec::new();
                    loop {
                        let i = next_idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= names.len() {
                            break;
                        }
                        // A compiler panic fails its case, not the worker
                        // (and with it every case the worker had left).
                        let outcome = std::panic::catch_unwind(|| compile_and_run(&names[i]))
                            .unwrap_or_else(|panic| CaseOutcome {
                                name: names[i].clone(),
                                ok: false,
                                detail: format!(
                                    "compiler panicked: {}",
                                    panic
                                        .downcast_ref::<String>()
                                        .map(String::as_str)
                                        .or_else(|| panic.downcast_ref::<&str>().copied())
                                        .unwrap_or("(non-string payload)")
                                ),
                            });
                        local.push(outcome);
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

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
