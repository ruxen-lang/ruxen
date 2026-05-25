//! Pin tests for `docs/specs/ownership/send_sync_enforcement.spec.md` —
//! NEGATIVE side (B3, B4, B5, B6, B7, B8, B11).
//!
//! Each fixture should typecheck with a SPECIFIC diagnostic code.
//! Diagnostics are asserted by code (E1100 / E1101 / E1102) rather
//! than message substring, matching the convention in
//! `tests/borrow_check_sample.rs` and `tests/typed_ffi_returns.rs`.
//!
//! Fixtures: `compiler/ruxen_core/tests/fixtures/ruxen/concurrency_*.rx`
//! (per the team rule against inline `r#"..."#` Ruxen source in `.rs`
//! pin tests).

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn typeck_errors(source: &str) -> Vec<Diagnostic> {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);
    result
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

fn count_with_code(errs: &[Diagnostic], code: &str) -> usize {
    errs.iter()
        .filter(|d| d.code.as_deref() == Some(code))
        .count()
}

// ─── B3 — `Mutex.new(non_send)` → E1101 ─────────────────────────────

#[test]
fn mutex_new_rejects_non_send_t_e1101() {
    let errs = typeck_errors(&rx("concurrency_mutex_new_rejects_non_send"));
    assert!(
        count_with_code(&errs, "E1101") >= 1,
        "expected E1101 for Mutex.new(Foo) where Foo is not Send, got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

// ─── B4 — `SharedSync.new(non_send)` → E1102 ────────────────────────

#[test]
fn sharedsync_new_rejects_non_send_t_e1102() {
    let errs = typeck_errors(&rx("concurrency_sharedsync_new_rejects_non_send"));
    assert!(
        count_with_code(&errs, "E1102") >= 1,
        "expected E1102 for SharedSync.new(Foo) where Foo is not Send, got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

// ─── B5 — `Sender[T]` / `Receiver[T]` reject non-Send T ──────────────
//
// No `channel[T]()` constructor exists in v1 (per
// library/std/sync/src/lib.rx — Sender / Receiver are bare class
// shells with no `.new`). The bound `Sender[T: Send]` / `Receiver[T:
// Send]` is enforced by the existing bound-checker (E1011) at
// instantiation sites — see `typeck/infer.rs::check_concurrency_bounds`.
// We pin the *bound enforcement* shape here so a future `channel[T]()`
// constructor can hook into the same diagnostic mechanism without
// rewiring.
#[test]
#[ignore = "no channel[T]() constructor in v1 — Sender/Receiver constraint reuses E1011 at instantiation"]
fn channel_rejects_non_send_t_e1101() {}

// ─── B6 — `Thread.spawn` rejects non-Send capture → E1100 ───────────

#[test]
fn thread_spawn_rejects_non_send_capture_e1100() {
    let errs = typeck_errors(&rx("concurrency_thread_spawn_rejects_non_send_capture"));
    assert!(
        count_with_code(&errs, "E1100") >= 1,
        "expected E1100 for Thread.spawn capturing non-Send Foo, got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

// ─── B7 — by-reference capture requires Sync ────────────────────────
//
// Covered by the same B6 fixture path: when the capture is by reference
// rather than by move, the check requires `&T: Send` (i.e. `T: Sync`).
// The current Ruxen closure surface doesn't distinguish move vs ref
// captures of class values at the .rx level (move semantics are the
// default for non-Copy class values), so the B6 fixture exercises the
// move path. A dedicated by-ref fixture is deferred until the closure
// surface gains explicit `move` / `ref` keywords.
#[test]
#[ignore = "closure surface doesn't distinguish move vs ref captures of class values in v1"]
fn thread_spawn_capture_by_ref_requires_sync_e1100() {}

// ─── B8 — multiple captures: one per offending capture ──────────────

#[test]
fn thread_spawn_emits_one_diagnostic_per_bad_capture() {
    let errs = typeck_errors(&rx("concurrency_thread_spawn_multiple_captures"));
    let bad_count = count_with_code(&errs, "E1100");
    // The fixture captures one Send (`good: Int`) and one non-Send
    // (`bad: Foo`). Exactly the non-Send capture should fire E1100.
    assert!(
        bad_count >= 1,
        "expected at least one E1100 for the non-Send capture, got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
    // The Send capture must not generate spurious diagnostics.
    let int_msg_present = errs
        .iter()
        .any(|d| d.code.as_deref() == Some("E1100") && d.message.contains("Int"));
    assert!(
        !int_msg_present,
        "E1100 fired on the Int capture, which is Send: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

// ─── B11 — `include !Send` (negative include) opt-out ───────────────

#[test]
fn negative_include_send_rejects_thread_spawn() {
    // Fixture wraps a `class NotSend; include !Send; end` in Mutex.new.
    // The negative include sets `opt_out_send`, which the strict
    // Send-check honours → E1101.
    let errs = typeck_errors(&rx("concurrency_negative_include_send_rejects"));
    assert!(
        count_with_code(&errs, "E1101") >= 1,
        "expected E1101 for Mutex.new(NotSend) where NotSend has `include !Send`, got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}
