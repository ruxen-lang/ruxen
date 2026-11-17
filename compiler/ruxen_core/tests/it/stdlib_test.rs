//! Integration tests for the `std.test` stdlib package that must read the
//! runner's own stdout (hook order, the summary line, fork isolation and
//! `it_panics`), so they cannot run inside the framework they check.
//! Matcher, TestCase and Runner-slot tests live in
//! `library/std/test/tests/matcher_test.rx`.
//!
//! Fixture .rx files live under `tests/fixtures/ruxen/test_*.rx` per
//! the no-inline-rx convention (feedback_no_inline_rx_in_pin_tests.md).

use crate::common;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

#[test]
fn tester_context_inherits_parent_hooks() {
    let (stdout, stderr, code) = common::compile_and_run(
        &rx("test_tester_context_hooks"),
        "stdlib_test_tester_context_hooks",
    );
    assert_eq!(code, Some(0), "stderr: {}", stderr);
    // outer case sees only outer hooks (in order):
    let outer_idx = stdout.find("outer_case_body").expect("outer body");
    let outer_before = stdout[..outer_idx]
        .find("outer_before")
        .expect("outer_before before body");
    assert!(outer_before < outer_idx);
    // inner case sees outer_before THEN inner_before, then body, then outer_after:
    let inner_body_idx = stdout.find("inner_case_body").expect("inner body");
    let inner_outer_before = stdout[..inner_body_idx]
        .rfind("outer_before")
        .expect("outer_before for inner case");
    let inner_inner_before = stdout[..inner_body_idx]
        .rfind("inner_before")
        .expect("inner_before for inner case");
    assert!(inner_outer_before < inner_inner_before);
    assert!(inner_inner_before < inner_body_idx);
    assert!(
        stdout[inner_body_idx..].contains("outer_after"),
        "context dropped the parent's after hook: {}",
        stdout
    );
    // 2 passing, 0 failing, 0 pending
    assert!(
        stdout.contains("2 passed, 0 failed, 0 pending"),
        "got: {}",
        stdout
    );
}

#[test]
fn tester_summary_counts_pass_fail_pending() {
    let (stdout, stderr, _code) = common::compile_and_run(
        &rx("test_tester_xit_and_fail"),
        "stdlib_test_tester_xit_and_fail",
    );
    // Binary may exit non-zero because one test failed — that's expected.
    assert!(
        stdout.contains("1 passed, 1 failed, 1 pending"),
        "got stdout={} stderr={}",
        stdout,
        stderr
    );
}

#[test]
fn runner_fork_isolates_panic() {
    // The fixture's middle case calls Runner.panic which `_exit(101)`s
    // the child. The fork-per-test isolation means cases 1 and 3 still
    // run; counts should be 2 passed, 1 failed.
    let (stdout, _stderr, _code) = common::compile_and_run(
        &rx("test_runner_fork_isolates_panic"),
        "stdlib_test_runner_fork_isolation",
    );
    assert!(
        stdout.contains("2 passed, 1 failed, 0 pending"),
        "got: {}",
        stdout
    );
}

#[test]
fn it_panics_passes_only_when_body_panics_with_expected_message() {
    let (stdout, _stderr, _code) =
        common::compile_and_run(&rx("test_expect_panic"), "stdlib_test_expect_panic");
    assert!(
        stdout.contains("1 passed, 2 failed, 0 pending"),
        "got: {}",
        stdout
    );
}
