//! Walk every fixture in `tests/release-e2e/cases/*.rvn`, compile with
//! the in-process codegen pipeline, run the binary, and diff stdout
//! against the matching `tests/release-e2e/expected/*.out`. Acts as a
//! cargo-test stand-in for the shell-based release harness so changes
//! that affect codegen (drop elaboration, runtime layout, …) are caught
//! by `cargo test -p riven-core`.

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::path::PathBuf;
use std::process::Command;

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
    let src_path = root.join(format!("tests/release-e2e/cases/{}.rvn", case_name));
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
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    if !errors.is_empty() {
        let msgs: Vec<String> = errors
            .iter()
            .take(5)
            .map(|d| format!("{}", d.message))
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

    let output = match Command::new(&bin_path).output() {
        Ok(o) => o,
        Err(e) => {
            return CaseOutcome {
                name: case_name.to_string(),
                ok: false,
                detail: format!("run: {}", e),
            }
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit = output.status.code();

    if stdout != expected {
        return CaseOutcome {
            name: case_name.to_string(),
            ok: false,
            detail: format!(
                "stdout mismatch (exit={:?})\n  got: {}\n  expected: {}\n  stderr: {}",
                exit,
                stdout.escape_debug(),
                expected.escape_debug(),
                stderr.escape_debug()
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
// Set `RIVEN_E2E_CASES=072_const_generic_array_arithmetic,073_…` to
// restrict the harness to a comma-separated subset of case stems
// (the filename without `.rvn`).  Whitespace around entries is
// trimmed; empty entries are ignored.  An unknown case name fails
// the run with a clear message so typos don't silently skip cases.
//
// Workflow:
//   - per-commit: skip the harness entirely (it's `#[ignore]`-gated).
//   - new fixture: `RIVEN_E2E_CASES=NAME cargo test --test
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
            if path.extension().and_then(|e| e.to_str()) != Some("rvn") {
                return None;
            }
            path.file_stem().and_then(|s| s.to_str()).map(String::from)
        })
        .collect();
    names.sort();

    // Apply the RIVEN_E2E_CASES filter, if set.  An unknown
    // requested case is a hard error — better to surface a typo
    // immediately than to silently skip the case the developer was
    // trying to verify.
    if let Ok(filter) = std::env::var("RIVEN_E2E_CASES") {
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
                    "RIVEN_E2E_CASES requested unknown case(s): {}\n\
                     (case stem is the filename without `.rvn`; \
                     check `tests/release-e2e/cases/`)",
                    missing.join(", ")
                );
            }
            let keep: std::collections::HashSet<String> = requested.into_iter().collect();
            names.retain(|n| keep.contains(n));
            eprintln!(
                "release-e2e: RIVEN_E2E_CASES filter active, running {} case(s): {}",
                names.len(),
                names.join(", ")
            );
        }
    }

    let mut outcomes = Vec::with_capacity(names.len());
    for name in &names {
        outcomes.push(compile_and_run(name));
    }

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
