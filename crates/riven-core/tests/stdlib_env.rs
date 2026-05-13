//! Integration tests for Phase 2 stdlib (#06) `std::env` additions:
//! `vars` (snapshot of process env into HashMap[String, String]) and
//! `current_dir` (Result[String, IoError]).
//!
//! Pin tests for the older pre-#06 surface (`args()` / `var(name)`)
//! live below the #06 group — added 2026-05 to close the gap noted
//! in `docs/specs/stdlib/env.spec.md`.
//!
//! Strategy: compile a tiny Riven program that exercises the new
//! function, run the resulting binary in a controlled environment,
//! and assert on the program's stdout. Avoids embedding non-
//! deterministic env-state into the assertion (we set or read a
//! sentinel rather than dumping the whole map).

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run_with_env<I, K, V>(source: &str, basename: &str, env: I) -> (String, String, bool)
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<std::ffi::OsStr>,
    V: AsRef<std::ffi::OsStr>,
{
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}.bin", basename));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let mut cmd = Command::new(&bin_path);
    cmd.env_clear();
    // PATH is required for the dynamic linker on some systems even when
    // the binary itself does not exec anything; restore the parent's
    // PATH so the produced binary can run.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("run binary");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// `env::vars` snapshots the process environment. We set a sentinel
/// key and assert the snapshot includes it via `contains_key`. We
/// don't extract the value via `get` because interpolation of a
/// `&String` from `Option[&String]` is a pre-existing v1 limitation
/// (HashMap[K, String] has no positive fixture in the repo) — the
/// `Display` interpolation refactor (prompt 06.2) is the appropriate
/// venue for that fix.
#[test]
fn env_vars_snapshots_process_environment() {
    let source = r##"
use std.env.vars

def main
  let m = vars()
  if m.contains_key("RIVEN_TEST_VARS_KEY")
    puts "snapshot_has_key"
  else
    puts "snapshot_missing_key"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run_with_env(
        source,
        "stdlib_env_vars_present",
        [("RIVEN_TEST_VARS_KEY", "expected-vars-value")],
    );
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert!(
        stdout.contains("snapshot_has_key"),
        "expected sentinel key in snapshot, got: stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// `env::vars().len > 0` — even a `cmd.env_clear()` plus a single
/// PATH-only restore should produce a non-empty map.
#[test]
fn env_vars_is_non_empty_when_one_var_set() {
    let source = r##"
use std.env.vars

def main
  let m = vars()
  if m.len > 0
    puts "non_empty"
  else
    puts "empty"
  end
end
"##;
    let (stdout, _stderr, ok) =
        compile_and_run_with_env(source, "stdlib_env_vars_non_empty", [("FOO", "bar")]);
    assert!(ok);
    assert!(
        stdout.contains("non_empty"),
        "expected non_empty, got: {}",
        stdout
    );
}

/// `env::current_dir` returns Result::Ok with a non-empty path on
/// success. We don't assert the exact directory because the test
/// harness may run from an arbitrary cwd; we only assert the
/// happy-path control flow and that the payload is non-empty.
#[test]
fn env_current_dir_returns_ok_path() {
    let source = r##"
use std.env.current_dir

def main
  match current_dir()
    Ok(path)  -> puts "cwd=#{path}"
    Err(_)    -> puts "err"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run_with_env(
        source,
        "stdlib_env_current_dir",
        std::iter::empty::<(&str, &str)>(),
    );
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.starts_with("cwd=") && stdout.len() > "cwd=\n".len(),
        "expected non-empty cwd path, got: {}",
        stdout
    );
}

// ─── env spec B1 / B2 direct pins (gap fill 2026-05) ──────────────────

/// `env::var("KEY")` returns `Result::Ok(value)` when the key is set.
/// Pins the spec's B2 happy path.
#[test]
fn env_var_returns_ok_for_set_key() {
    let source = r##"
use std.env.var

def main
  match var("RIVEN_SENTINEL")
    Ok(v)  -> puts "got=#{v}"
    Err(_) -> puts "missing"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run_with_env(
        source,
        "stdlib_env_var_set",
        [("RIVEN_SENTINEL", "hello")],
    );
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("got=hello"), "stdout: {}", stdout);
}

/// `env::var` returns `Result::Err(...)` for an unset key.  Pins
/// spec B2 negative path.
#[test]
fn env_var_returns_err_for_missing_key() {
    let source = r##"
use std.env.var

def main
  match var("RIVEN_DEFINITELY_NOT_SET_12345")
    Ok(v)  -> puts "got=#{v}"
    Err(_) -> puts "missing"
  end
end
"##;
    let (stdout, stderr, ok) =
        compile_and_run_with_env(source, "stdlib_env_var_missing", std::iter::empty::<(&str, &str)>());
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("missing"), "stdout: {}", stdout);
}

/// `args()` returns a non-empty Vec — element 0 is the program name.
/// Pins the spec's B1 lower-bound guarantee.
#[test]
fn env_args_includes_program_name() {
    let source = r##"
use std.env.args

def main
  let a = args()
  if a.len() > 0
    puts "ok len=#{a.len()}"
  else
    puts "fail empty"
  end
end
"##;
    let (stdout, stderr, ok) =
        compile_and_run_with_env(source, "stdlib_env_args_present", std::iter::empty::<(&str, &str)>());
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("ok"), "expected non-empty args, got: {}", stdout);
}
