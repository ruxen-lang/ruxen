//! Integration test for Phase 3 `std::time` module.
//!
//! Verifies that `time.now_ns()` and `time.unix_ns()` are reachable
//! through the resolver, lower to the right runtime calls, and produce
//! sensible monotonic values at runtime.

use riven_core::codegen;
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

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
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

    let output = Command::new(&bin_path).output().expect("run binary");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// `time.now_ns()` returns positive monotonic nanoseconds and a second
/// call returns a value greater than or equal to the first (monotonic
/// clock never moves backwards).
#[test]
fn time_now_ns_is_monotonic() {
    let source = rvn("time_now_ns_is_monotonic");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_monotonic");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert!(
        stdout.contains("ok"),
        "expected monotonic ordering, got: [{}]",
        stdout
    );
}

/// `time.unix_ns()` returns nanoseconds since the Unix epoch — sanity
/// check that the value is in the post-2020 range (a reasonable lower
/// bound that any modern system clock will exceed).
#[test]
fn time_unix_ns_is_post_2020() {
    let source = rvn("time_unix_ns_is_post_2020");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_unix");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert!(
        stdout.contains("ok"),
        "expected unix_ns post-2020, got: [{}]",
        stdout
    );
}

// ── Phase 2 stdlib (#06.5 T4): Duration / Instant / sleep ──────────────
//
// Each test compiles a small `.rvn` fixture that exercises one slice of
// the new surface. The fixtures print "ok" or "fail <diagnostic>" so
// each test can detect both binary-level failure (panic, abort) and
// assertion-level failure (wrong value) without re-implementing the
// numeric comparison in Rust.

/// `Duration.from_secs(5).as_millis() == 5000`.
#[test]
fn duration_from_secs_as_millis() {
    let source = rvn("duration_from_secs_as_millis");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_dur_secs_ms");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// `Duration.from_millis(1500).as_secs() == 1` — integer floor.
#[test]
fn duration_from_millis_as_secs_floors() {
    let source = rvn("duration_from_millis_as_secs");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_dur_ms_secs");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// Round-trip every `from_*` × `as_*` combination at 2 seconds.
#[test]
fn duration_from_as_round_trip_matrix() {
    let source = rvn("duration_round_trip_matrix");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_dur_matrix");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// `Duration + Duration` sums nanos via both the binop and named-method
/// paths (the binop is a hard-coded special-case in mir/lower/expr/
/// binops.rs; `.add()` is the survives-generic-code fallback).
#[test]
fn duration_add_via_binop_and_named() {
    let source = rvn("duration_add");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_dur_add");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// `Duration - Duration` saturates to zero on underflow — `from_secs(1)
/// - from_secs(5)` is a zero-Duration.
#[test]
fn duration_sub_saturates_to_zero() {
    let source = rvn("duration_saturating_sub");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_dur_sat_sub");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// `Instant.now()` after a `sleep(from_millis(1))` strictly increases.
/// Implicitly verifies CLOCK_MONOTONIC ordering — if the kernel ever
/// returned a non-monotonic sample the subtract would panic and the
/// binary would exit non-zero.
#[test]
fn instant_monotonic_after_sleep() {
    let source = rvn("instant_monotonic");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_inst_mono");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// `Instant.elapsed()` returns a non-negative Duration even without an
/// explicit sleep.
#[test]
fn instant_elapsed_non_negative() {
    let source = rvn("instant_elapsed");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_inst_elapsed");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// `Instant.duration_since(earlier)` returns the wall-clock delta. The
/// fixture only enforces non-negative (tight tolerance is fragile on
/// CI); cross-checked against `Instant - Instant` (same runtime fn).
#[test]
fn instant_duration_since_returns_delta() {
    let source = rvn("instant_duration_since");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_inst_dur_since");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// `Instant.duration_since(later)` where `later > self` MUST panic.
/// The fixture deliberately reverses the order and expects the binary
/// to exit non-zero. If the panic ever stops firing, `stdout` contains
/// "did_not_panic" and the assertion below catches it.
#[test]
fn instant_duration_since_future_panics() {
    let source = rvn("instant_duration_since_future_panics");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_inst_panic");
    assert!(
        !ok,
        "expected panic-driven non-zero exit, but binary succeeded. \
         stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(
        !stdout.contains("did_not_panic"),
        "panic guard did not fire; reached the println past the panic site: [{stdout}]"
    );
    assert!(
        stderr.contains("earlier is in the future")
            || stderr.contains("Instant.duration_since"),
        "expected panic message in stderr, got: [{stderr}]"
    );
}

/// `sleep(from_millis(50))` + `Instant.elapsed()` lands in 40–200 ms.
/// Wide tolerance is deliberate — CI hosts (esp. macOS shared runners)
/// have unpredictable scheduler granularity.
#[test]
fn sleep_duration_elapses_in_tolerance_band() {
    let source = rvn("sleep_duration_elapses");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_time_sleep_band");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}
