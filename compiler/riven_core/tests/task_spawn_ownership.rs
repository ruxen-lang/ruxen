//! Pin: `Task.spawn_raw` ownership-move (closes
//! `project_riven_task_spawn_ownership_gap.md`).
//!
//! `library/std/future/runtime/scheduler.c:150-152` documents that
//! `Task.spawn_raw(future)` takes ownership of the future heap
//! block. Riven's drop elaboration must recognise this move so the
//! caller's local is NOT freed at scope exit — otherwise a
//! short-lived spawner fn returns, its `fut` local is drop-
//! elaborated, and the next `riven_executor_pump_tasks()` call in
//! the outer driver dereferences freed memory (SIGSEGV / exit 139).
//!
//! The fix lives in `compiler/riven_core/src/mir/lower/drops.rs`:
//! the `is_move_by_ffi_callee` predicate excludes
//! `riven_executor_spawn` from the runtime-borrow-helper list, so
//! the default-taint path on the per-arg loop runs and the
//! spawned future's local is removed from `alloc_rooted`.
//!
//! Discipline: all Riven source lives in the
//! `tests/fixtures/riven/` tree, never inline in this `.rs` file
//! (see `feedback_no_inline_rvn_in_pin_tests`).

use riven_core::codegen;
use riven_core::diagnostics::DiagnosticLevel;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::process::Command;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Compile `source` through the in-process pipeline and run the
/// resulting binary. Returns `(stdout, stderr, exit_code)` where
/// `exit_code` is the raw OS status — we want to assert exit 0
/// rather than only "exit_ok" because the regression we're
/// pinning is exit 139 (SIGSEGV) specifically.
fn compile_and_run(source: &str, basename: &str) -> (String, String, Option<i32>) {
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
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code(),
    )
}

/// The canonical regression: a short-lived `async def spawner`
/// allocates a `PayloadFuture`, calls `Task.spawn_raw(fut)`, and
/// returns the handle. `main` then pumps until the spawned task
/// completes and prints "ok" if the payload (42) round-trips.
///
/// Before the fix: spawner's `fut` was drop-elaborated at scope
/// exit; the next `riven_executor_pump_tasks()` from `main`
/// followed a freed pointer and SIGSEGV'd (exit 139).
///
/// After the fix: `riven_executor_spawn` is excluded from the
/// runtime-borrow-helper analysis in `drops.rs`, so the
/// default-taint path removes `fut` from `alloc_rooted` and the
/// scope-exit drop pass skips it. The future stays live for the
/// executor's queue, the pump runs it to Ready(42), and main
/// reads the payload cleanly.
#[test]
fn task_spawn_raw_transfers_ownership_to_executor() {
    let source = rvn("task_spawn_move_by_ffi");
    let (stdout, stderr, exit_code) =
        compile_and_run(&source, "task_spawn_move_by_ffi");

    // Specifically assert exit 0 (not just "not crashed"). Exit
    // 139 (SIGSEGV) was the precise pre-fix failure mode; making
    // the assertion exact catches a regression that only mutates
    // the failure shape (e.g. exit 134 from abort()).
    assert_eq!(
        exit_code,
        Some(0),
        "binary must exit cleanly (exit 139 / SIGSEGV indicates the \
         Task.spawn_raw ownership-move regression has returned); \
         stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(
        stdout.contains("ok"),
        "expected payload 42 round-trip; stdout={stdout:?} stderr={stderr:?}"
    );
}
