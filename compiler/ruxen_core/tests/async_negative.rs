//! Negative pin tests for the async sub-phase 1 surface
//! (`docs/specs/stdlib/async.spec.md` B5 + B6).
//!
//! These document the REJECTION cases the spec demands. Sub-phase 1
//! ships the surface (Future mixin / Poll enum / Context+Waker), but
//! the validation pass that would actually reject the offending
//! includes does NOT yet exist:
//!
//!   * No `include`-time check that every `assoc_types` entry on the
//!     included mixin has a matching `type Output = T` binding in
//!     the includer's body (B5).
//!   * No structural check that an implementor's method signature
//!     (`def var poll(cx: &var Context) -> X`) matches the mixin's
//!     declared signature (B6).
//!
//! `MixinResolver::check_satisfaction` (compiler/ruxen_core/src/
//! typeck/mixins.rs) has the bones for the method-name check but is
//! never called against `include` sites today. Both validations
//! belong to a later slice — flagging them here as `#[ignore]`
//! pinned tests keeps the spec contract visible, the fixtures ready,
//! and a `cargo test` run noiseless until the validator lands.
//!
//! When the validator arrives:
//!   1. Drop the `#[ignore]` attribute.
//!   2. Wire the expected diagnostic code through the assertion
//!      (the spec names E0612 / E0613; if those numbers are taken
//!      by the time the validator lands, pick the next free
//!      `Async-include` slot in `diagnostics/codes.rs`).

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

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

/// Reads a fixture file from `tests/fixtures/ruxen/` per the team's
/// no-inline-rx-source-in-pin-tests rule.
fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

// ─── B5 — `include Future` without `type Output` rejected ───────────

/// Spec B5: a class that `include Future` but omits the
/// `type Output = T` binding must be rejected. Today the resolver
/// happily accepts the bare include and downstream code falls over
/// later with a confusing inference error — sub-phase 2's mixin
/// validator is the proper fix. Tracked separately from this prompt.
#[test]
#[ignore = "validation pass not yet implemented; see test file header"]
fn include_future_without_output_rejected() {
    let source = rx("async_negative_missing_output");
    let errors = typeck_errors(&source);
    assert!(
        errors
            .iter()
            .any(|d| d.message.contains("Output") || d.message.contains("associated type")),
        "expected an error about a missing `type Output` binding, got: {:?}",
        errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// ─── B12 — `.await` outside async context (E1110) ───────────────────

/// Spec B12 (Milestone 2B): `.await` is only valid inside `async def`
/// or `async { ... }`. A bare `.await` inside a synchronous function
/// must be rejected at resolve time with code E1110. The check is
/// `async_scope_depth == 0` in `resolve/exprs.rs`.
#[test]
fn await_outside_async_context_rejected_e1110() {
    let source = rx("async_negative_await_outside_async");
    let errors = typeck_errors(&source);
    assert!(
        errors.iter().any(|d| d.code.as_deref() == Some("E1110")),
        "expected E1110 for `.await` outside async fn, got: {:?}",
        errors
            .iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
}

// ─── B6 — `poll` with wrong signature rejected ──────────────────────

/// Spec B6: a class that overrides `poll` with the wrong return type
/// (e.g. `-> Int` instead of `-> Poll[Self.Output]`) must be
/// rejected by the mixin-signature checker. Same gap as B5 — the
/// signature comparison logic doesn't run against include sites yet.
#[test]
#[ignore = "validation pass not yet implemented; see test file header"]
fn poll_signature_mismatch_rejected() {
    let source = rx("async_negative_poll_wrong_return");
    let errors = typeck_errors(&source);
    assert!(
        errors
            .iter()
            .any(|d| d.message.contains("poll") && d.message.contains("Poll")),
        "expected an error about the poll signature, got: {:?}",
        errors.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// ─── B6 (executor.spec.md) — block_on inside async (E1112) ──────────

/// Spec B6 (sub-phase 3): `block_on` is rejected inside any async
/// context with code E1112. Symmetric to E1110 — the resolver tracks
/// `async_scope_depth`; if it's > 0 at a `block_on(...)` call site,
/// the call is flagged. The async_lowering rewriter intentionally
/// SKIPS bodies of async functions/closures so the raw Call survives
/// to resolve.
#[test]
fn block_on_inside_async_rejected_e1112() {
    let source = rx("async_negative_block_on_inside_async");
    let errors = typeck_errors(&source);
    assert!(
        errors.iter().any(|d| d.code.as_deref() == Some("E1112")),
        "expected E1112 for block_on inside async fn, got: {:?}",
        errors
            .iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
}

// ─── E1115 — `.await` inside loop/while/for body rejected ───────────

/// `.await` inside a `loop { ... }` body is rejected with E1115.
/// Before the dedicated pre-pass, this shape produced a misleading
/// E1110 ("`.await` only valid inside async def") even though the
/// enclosing function WAS async — because the segmenter bailed,
/// `lower_one_async_fn` (no-await path) wrapped the body, and the
/// `.await` ended up inside a sync `poll` method. The pre-pass
/// surfaces the correct E1115 ahead of the rewrite.
#[test]
fn await_in_loop_body_rejected_e1115() {
    let source = rx("async_negative_await_in_loop");
    let errors = typeck_errors(&source);
    assert!(
        errors.iter().any(|d| d.code.as_deref() == Some("E1115")),
        "expected E1115 for `.await` inside `loop` body, got: {:?}",
        errors
            .iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
    assert!(
        !errors.iter().any(|d| d.code.as_deref() == Some("E1110")),
        "must NOT also emit E1110 (would have been the old misleading code), got: {:?}",
        errors
            .iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
}

/// `.await` inside a `while cond { ... }` body — same v2 deferral.
#[test]
fn await_in_while_body_rejected_e1115() {
    let source = rx("async_negative_await_in_while");
    let errors = typeck_errors(&source);
    assert!(
        errors.iter().any(|d| d.code.as_deref() == Some("E1115")),
        "expected E1115 for `.await` inside `while` body, got: {:?}",
        errors
            .iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
}

/// `.await` inside a `for pat in iter { ... }` body — same v2 deferral.
/// The iterable expression itself is evaluated ONCE before the loop,
/// so a `.await` in the iterable position is fine (and lowers via
/// the normal pre-await path); only awaits in the body suspend per
/// iteration.
#[test]
fn await_in_for_body_rejected_e1115() {
    let source = rx("async_negative_await_in_for");
    let errors = typeck_errors(&source);
    assert!(
        errors.iter().any(|d| d.code.as_deref() == Some("E1115")),
        "expected E1115 for `.await` inside `for` body, got: {:?}",
        errors
            .iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
}

/// Bug-by-construction (Phase 3 Task 1): `.await` nested inside an
/// `EnumVariant` argument (`Some(g().await)`) inside a loop body must
/// still raise E1115. The pre-Phase-3 hand-rolled `collect_e1115_in_expr`
/// matched a fixed set of expression forms and ended with a trailing
/// `_ => {}`, so it never descended into `EnumVariant` args — the
/// `.await` was invisible and no diagnostic fired. Migrating the
/// collector onto the exhaustive `parser::visit::walk_expr` closes the
/// drift: every expression form is now traversed.
#[test]
fn await_in_loop_inside_enum_variant_arg_is_diagnosed_e1115() {
    let source = r#"
async def g() -> Int
  1
end

async def run() -> Int
  let var i = 0
  while i < 3
    let _x = Some(g().await)
    i = i + 1
  end
  i
end
"#;
    let errors = typeck_errors(source);
    assert!(
        errors.iter().any(|d| d.code.as_deref() == Some("E1115")),
        "await nested in EnumVariant arg inside a loop must raise E1115; got {:?}",
        errors
            .iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
}
