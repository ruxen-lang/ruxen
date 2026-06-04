//! Pin tests for Phase 2 #06.5 T8: `std::rand` — kernel CSPRNG-backed
//! `random_bytes` / `random_u64` / `random_fill`.
//!
//! Each test compiles a small Ruxen program that exercises one slice of
//! the surface and reads the printed verdict (`ok` / `fail …`) — same
//! convention as `stdlib_time.rs`. The CSPRNG hard-failure path
//! (D3 in the spec) is not exercised here: forcing the kernel to fail
//! `getrandom` / `SecRandomCopyBytes` is platform-specific and would
//! require fault injection. The negative-`n` InvalidInput case below
//! is the closest in-process proxy and exercises the same Err shape
//! that a CSPRNG failure would surface through.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::process::Command;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
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
    let bin_path = tmp_dir.join(format!("{}-{}-{}.bin", basename, std::process::id(), ruxen_unique_id()));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// D1: `random_bytes(32)` returns Ok(buf) with `buf.len == 32`. We
/// additionally assert that not all bytes are identical — a healthy
/// CSPRNG over 32 samples will produce at least two distinct values
/// with probability > 1 - 256 * (1/256)^31, which is overwhelmingly
/// close to 1. (The "all-same" failure mode is what a stuck-at
/// implementation would produce — worth catching.)
#[test]
fn rand_random_bytes_length() {
    let source = rx("rand_random_bytes_length");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_rand_bytes_len");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// D2 (zero): `random_bytes(0)` returns Ok(buf) with `buf.len == 0`
/// without invoking the kernel.
#[test]
fn rand_random_bytes_zero_returns_empty() {
    let source = rx("rand_random_bytes_zero_returns_empty");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_rand_bytes_zero");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// D2 (negative): `random_bytes(-1)` returns `Err(_)` rather than
/// panicking or returning an empty Ok. We assert the Err arm fires
/// (the kind/message inspection is exercised by the IoError pin tests
/// directly — `?T::kind` inference does not see through a generic
/// `e: ?T` here, so a deeper match would need explicit annotation
/// and adds nothing to what's already pinned elsewhere).
#[test]
fn rand_random_bytes_negative_returns_err() {
    let source = rx("rand_random_bytes_negative_returns_err");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_rand_bytes_neg");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// D4: two consecutive `random_u64()` calls return different values.
/// The 2^-64 collision probability is well below any other test's
/// false-negative budget.
#[test]
fn rand_random_u64_changes() {
    let source = rx("rand_random_u64_changes");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_rand_u64_changes");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// D5: `random_fill(&var buf)` overwrites every slot. We seed the buf
/// with a known sentinel (0xAA across 16 slots), call fill, and assert
/// at least one slot now differs — same "stuck-at" guard as the
/// length test above.
#[test]
fn rand_random_fill_overwrites() {
    let source = rx("rand_random_fill_overwrites");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_rand_fill");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

/// Local mirror of the two release-e2e fixtures (590, 593) — the
/// `release_e2e_smoke` harness is `#[ignore]`d and only runs in the
/// dedicated phase-completion test pass. Mirroring the same `.rx`
/// content here gives us in-suite verification that the fixtures
/// themselves compile and produce their expected stdout, without
/// pulling in the full e2e harness.
#[test]
fn rand_e2e_fixture_590_length_matches_expected() {
    let root = workspace_root();
    let src = std::fs::read_to_string(
        root.join("tests/release-e2e/cases/590_rand_random_bytes_length.rx"),
    )
    .expect("read 590 fixture");
    let expected = std::fs::read_to_string(
        root.join("tests/release-e2e/expected/590_rand_random_bytes_length.out"),
    )
    .expect("read 590 expected");
    let (stdout, stderr, ok) = compile_and_run(&src, "stdlib_rand_e2e_590");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert_eq!(stdout, expected, "stdout mismatch for 590");
}

#[test]
fn rand_e2e_fixture_593_fill_matches_expected() {
    let root = workspace_root();
    let src = std::fs::read_to_string(
        root.join("tests/release-e2e/cases/593_rand_random_fill_overwrites.rx"),
    )
    .expect("read 593 fixture");
    let expected = std::fs::read_to_string(
        root.join("tests/release-e2e/expected/593_rand_random_fill_overwrites.out"),
    )
    .expect("read 593 expected");
    let (stdout, stderr, ok) = compile_and_run(&src, "stdlib_rand_e2e_593");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert_eq!(stdout, expected, "stdout mismatch for 593");
}

/// Resolver-level check: the three free fns are reachable through
/// `std.rand.*`. A regression that drops `rand_id` from `std_id`'s
/// items would surface here as a resolve error rather than a runtime
/// behaviour change.
#[test]
fn rand_module_is_importable_via_std_rand() {
    let source = rx("rand_module_is_importable_via_std_rand");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_rand_imports");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{stdout}] stderr=[{stderr}]"
    );
    assert!(stdout.contains("ok"), "expected ok, got: [{stdout}]");
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
