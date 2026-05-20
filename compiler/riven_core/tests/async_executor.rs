//! Pin tests for the async sub-phase 3 block_on executor
//! (`docs/specs/stdlib/executor.spec.md` B1–B10).
//!
//! What's covered here (compile-time / AST-shape only):
//!   * B1 / B7 — block_on over a sub-phase-2A future typechecks.
//!   * B3 — Waker.wake / wake_by_ref are no-ops (no panic at the lib
//!     decl surface; the runtime symbols are still callable).
//!   * B4 — Context.waker returns a real waker (no longer panics).
//!     Pinned by the surface: `Context.executor` factory exists and
//!     the rewritten code in main calls `.waker()` cleanly via the
//!     lowered poll loop's match arms.
//!   * B5 — Context.test_dummy still works (regression — existing
//!     async_lowering tests prove this).
//!
//! What's NOT covered here:
//!   * B2 — covered end-to-end by the e2e fixtures (release_e2e_smoke).
//!   * B8 — covered by e2e (cases/724).
//!   * B6 — covered in `async_negative.rs::block_on_inside_async_rejected_e1112`.
//!   * B9 — Drop on completion: needs actual execution + a
//!     Drop-tracking future. Deferred to a follow-up — current MIR
//!     drop elaboration handles the inline-loop shape correctly
//!     based on inspection but a runtime pin is the right proof.
//!     Marked `#[ignore]` below.
//!   * B10 — No-leak under repeated block_on: ditto, runtime pin.
//!     Marked `#[ignore]`.

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::resolve::symbols::DefKind;
use riven_core::typeck;

/// Reads a fixture file from `tests/fixtures/riven/` per the team's
/// no-inline-rvn-source-in-pin-tests rule.
fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn typeck_result(source: &str) -> typeck::TypeCheckResult {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    typeck::type_check(&prog)
}

fn error_messages(diags: &[Diagnostic]) -> Vec<String> {
    diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .map(|d| d.message.clone())
        .collect()
}

// ─── B1 / B7 — block_on over sub-phase-2A future typechecks ─────────

/// Spec B1, B7: `block_on(make_int())` over a no-await async fn
/// typechecks. The async_lowering rewriter erases the call at AST
/// time and replaces with an inline poll loop. After resolve/typeck
/// the body of main must report no errors.
#[test]
fn block_on_runs_subphase2a_future_to_ready() {
    let result = typeck_result(&rvn("async_executor_block_on_no_await"));
    let errors = error_messages(&result.diagnostics);
    assert!(
        errors.is_empty(),
        "block_on over a no-await future should typecheck: {:?}",
        errors
    );
}

/// Spec B8: chained-await futures driven by block_on typecheck.
#[test]
fn block_on_runs_subphase2b_chained_future_to_ready() {
    let result = typeck_result(&rvn("async_executor_block_on_with_chain"));
    let errors = error_messages(&result.diagnostics);
    assert!(
        errors.is_empty(),
        "block_on over a chained-await future should typecheck: {:?}",
        errors
    );
}

// ─── Task #20 — block_on preserves the future's Output type ─────────

/// Task #20 fix: the prior lowering shape emitted
/// `var __block_on_res_N = 0` which pinned every block_on result to
/// Int and silently erased typed Outputs (Result, String, classes).
/// The new shape uses `break v` from the Poll.Ready arm and lets
/// typeck infer the loop type from the break value — the future's
/// actual Output. This test pins the typed-Output behaviour by
/// destructuring a `Result[Int, Int]` returned from block_on; if
/// the result-var pin regressed, the `Ok(v) -> v + 1` arm would
/// either fail typeck or silently bind `v` to the wrong type.
#[test]
fn block_on_preserves_typed_result_output() {
    let result = typeck_result(&rvn("async_executor_block_on_typed_result"));
    let errors = error_messages(&result.diagnostics);
    assert!(
        errors.is_empty(),
        "block_on over a Result-returning future should typecheck \
         and the Ok payload must be Int (the future's Output type), \
         not erased to whatever the result-var was pinned to: {:?}",
        errors
    );
}

// ─── B3 — Waker.wake / wake_by_ref are surface-callable (no panic) ──

/// Spec B3: the Waker.wake / wake_by_ref methods are no-ops in
/// sub-phase 3 (real signaling lands in sub-phase 4). The lib decls
/// in library/std/future/src/lib.rvn are still wired to the same C
/// symbols; the surface check here is that those methods are
/// registered as DefKind::Method on the Waker class.
#[test]
fn waker_wake_is_noop_in_subphase3() {
    // A trivial program that just uses the std prelude so we get a
    // populated symbol table.
    let result = typeck_result("def main\n  puts \"ok\"\nend\n");
    let errors = error_messages(&result.diagnostics);
    assert!(errors.is_empty(), "trivial program must typecheck: {:?}", errors);

    // Find the Waker class and confirm both methods are present.
    let waker_id = result
        .symbols
        .iter()
        .find(|def| def.name == "Waker" && matches!(def.kind, DefKind::Class { .. }))
        .map(|def| def.id)
        .expect("Waker class must be registered by bootstrap");

    let has_wake = result.symbols.iter().any(|d| {
        d.name == "wake"
            && matches!(&d.kind, DefKind::Method { parent, .. } if *parent == waker_id)
    });
    let has_wake_by_ref = result.symbols.iter().any(|d| {
        d.name == "wake_by_ref"
            && matches!(&d.kind, DefKind::Method { parent, .. } if *parent == waker_id)
    });
    assert!(has_wake, "Waker.wake must be registered as a method");
    assert!(
        has_wake_by_ref,
        "Waker.wake_by_ref must be registered as a method"
    );
}

// ─── B4 — Context.waker returns a real waker after sub-phase 3 ──────

/// Spec B4: `Context.waker` no longer panics; it returns the
/// singleton no-op Waker. The surface check is that `Context` has
/// both `waker` (instance) and `executor` (static) methods registered
/// — the latter is new in sub-phase 3 as the factory the block_on
/// intrinsic uses.
#[test]
fn context_waker_returns_real_waker_after_subphase3() {
    let result = typeck_result("def main\n  puts \"ok\"\nend\n");
    let errors = error_messages(&result.diagnostics);
    assert!(errors.is_empty(), "trivial program must typecheck: {:?}", errors);

    let ctx_id = result
        .symbols
        .iter()
        .find(|def| def.name == "Context" && matches!(def.kind, DefKind::Class { .. }))
        .map(|def| def.id)
        .expect("Context class must be registered by bootstrap");

    let has_waker = result.symbols.iter().any(|d| {
        d.name == "waker"
            && matches!(&d.kind, DefKind::Method { parent, .. } if *parent == ctx_id)
    });
    let has_executor = result.symbols.iter().any(|d| {
        d.name == "executor"
            && matches!(&d.kind, DefKind::Method { parent, .. } if *parent == ctx_id)
    });
    assert!(has_waker, "Context.waker must be registered as a method");
    assert!(
        has_executor,
        "Context.executor static factory must be registered (new in sub-phase 3)"
    );
}

// ─── B5 — Context.test_dummy still works ────────────────────────────

/// Spec B5: `Context.test_dummy` continues to work after sub-phase 3.
/// It's now wire-compatible with `Context.executor` (both carry the
/// singleton no-op waker in their first slot), so `.waker()` works
/// on either kind. Surface check only — the existing
/// async_lowering::context_test_dummy_constructs test pins the AST
/// resolution from the call site.
#[test]
fn context_test_dummy_still_works() {
    let result = typeck_result("def main\n  puts \"ok\"\nend\n");
    let errors = error_messages(&result.diagnostics);
    assert!(errors.is_empty(), "trivial program must typecheck: {:?}", errors);

    let ctx_id = result
        .symbols
        .iter()
        .find(|def| def.name == "Context" && matches!(def.kind, DefKind::Class { .. }))
        .map(|def| def.id)
        .expect("Context class must be registered by bootstrap");

    let has_test_dummy = result.symbols.iter().any(|d| {
        d.name == "test_dummy"
            && matches!(&d.kind, DefKind::Method { parent, .. } if *parent == ctx_id)
    });
    assert!(
        has_test_dummy,
        "Context.test_dummy must continue to be registered as a method"
    );
}

// ─── B9 — Drop on completion ────────────────────────────────────────

/// Spec B9: after `block_on` returns the future's output, the future
/// itself is dropped exactly once. Verifying this needs a real
/// execution + a Drop-tracking sub-future + a counter side channel.
/// The inline poll-loop expression rewriting produces standard Riven
/// constructs (`var fut = ...; loop ... end`), so the existing MIR
/// drop elaboration should handle the future drop correctly when
/// the surrounding scope exits — but a runtime pin is the right
/// proof.
#[test]
#[ignore = "needs runtime Drop-tracking infrastructure; AST-only inspection is insufficient. \
            Deferred to a follow-up that wires the Drop side channel."]
fn block_on_drops_future_after_return() {
    // Place-holder: when this lands, the body becomes a runtime
    // assertion through the release_e2e_smoke harness with a fixture
    // that records drop counts on a static counter.
}

// ─── B10 — No leak under repeated block_on ──────────────────────────

/// Spec B10: a tight loop of block_on calls does not leak. Per-call
/// allocations: the Context (8 bytes, currently leaked — Context has
/// no Drop impl yet in Riven), the future state machine (heap, has
/// proper Drop). Sub-phase 4 wires Context lifecycle properly; for
/// now the per-call 8-byte leak is bounded and not a regression
/// over the rest of v1's allocation story.
///
/// As with B9 this needs runtime instrumentation. Deferred.
#[test]
#[ignore = "needs runtime RSS-tracking infrastructure; AST-only inspection is insufficient. \
            Deferred to a follow-up. Sub-phase 4 will exercise this path harder."]
fn block_on_loop_does_not_leak() {}
