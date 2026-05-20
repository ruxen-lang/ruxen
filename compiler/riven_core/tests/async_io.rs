//! Pin tests for the async sub-phase 4A surface
//! (`docs/specs/stdlib/async_io.spec.md` partial — see the
//! report-back accompanying the 4A commit for the bootstrap-merge
//! gap that blocks the full `time.sleep(d)` future hand-off).
//!
//! What 4A ships in this commit:
//!   * The OS-level reactor (epoll on Linux, kqueue on macOS) in
//!     `library/std/future/runtime/reactor.c`, with a thread-local
//!     pointer + lifecycle hooks (`riven_reactor_acquire` /
//!     `_release` / `_park_current`).
//!   * `Context.executor` / `Context.drop` install/release the
//!     per-thread reactor (lib decls in
//!     `library/std/future/src/lib.rvn`).
//!   * `Thread.yield_now`'s C body (`riven_thread_yield` in
//!     `library/std/time/runtime/time.c`) now delegates to
//!     `riven_reactor_park_current`, which blocks on the reactor
//!     when registrations are pending and falls back to
//!     `sched_yield` otherwise. The AST-level block_on rewriter
//!     emission is UNCHANGED — Pending arm still emits
//!     `Thread.yield_now` (per the coordinator's "zero
//!     compiler/src/ edits" constraint).
//!   * Reactor primitive free-fn lib decls land in
//!     `library/std/time/src/lib.rvn` (the wire surface only —
//!     no user-side `time.sleep` ships in this commit).
//!
//! What's deferred:
//!   * `time.sleep(d) -> TimeSleepFuture` (Milestone 4A B1) — the
//!     hand-written future has user-bodied Riven methods (`def init`,
//!     `def var poll`, `def drop`) that the bootstrap-merge does not
//!     currently route through `resolve_item`. Adding that routing is
//!     a one-shot compiler/src/ edit; for 4A it's reported back to
//!     the coordinator rather than slipped in unilaterally.
//!   * The e2e `cases/725_time_sleep_block_on.rvn` fixture is held
//!     until the gap above lands.
//!
//! What's covered here:
//!   * B3 — `Context` carries a `drop` lib decl (per-thread reactor
//!     teardown at Context scope exit).

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::resolve::symbols::DefKind;
use riven_core::typeck;

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

// ─── B3 — Context.drop lib decl after sub-phase 4A ─────────────────

/// Sub-phase 4A extends the `Context` surface with `drop` (per-thread
/// reactor teardown at Context scope exit). The reactor itself is
/// thread-local in the C runtime, not a field on Context, so no other
/// Context-side methods are needed for 4A — `Thread.yield_now`'s
/// existing emission in the block_on rewriter drives the reactor via
/// the C-side `riven_thread_yield -> riven_reactor_park_current`
/// chain.
#[test]
fn context_has_drop_lib_decl_after_subphase4a() {
    let result = typeck_result("def main\n  puts \"ok\"\nend\n");
    let errors = error_messages(&result.diagnostics);
    assert!(
        errors.is_empty(),
        "trivial program must typecheck: {:?}",
        errors
    );

    let ctx_id = result
        .symbols
        .iter()
        .find(|def| def.name == "Context" && matches!(def.kind, DefKind::Class { .. }))
        .map(|def| def.id)
        .expect("Context class must be registered by bootstrap");

    let has_drop = result.symbols.iter().any(|d| {
        d.name == "drop"
            && matches!(&d.kind, DefKind::Method { parent, .. } if *parent == ctx_id)
    });
    assert!(
        has_drop,
        "Context.drop must be registered (lib decl in future/src/lib.rvn) \
         so MIR drop elaboration tears down the per-thread reactor"
    );
}

// ─── Reactor free-fn lib decls registered ──────────────────────────

/// The four reactor primitives — `riven_reactor_current_handle`,
/// `_register_timer`, `_check_fired`, `_deregister` — are declared as
/// free-fn lib decls in `library/std/time/src/lib.rvn`. Surface check:
/// each registers as a Function symbol after bootstrap.
#[test]
fn reactor_free_fn_lib_decls_registered() {
    let result = typeck_result("def main\n  puts \"ok\"\nend\n");
    let errors = error_messages(&result.diagnostics);
    assert!(
        errors.is_empty(),
        "trivial program must typecheck: {:?}",
        errors
    );

    for fn_name in [
        "riven_reactor_current_handle",
        "riven_reactor_register_timer",
        "riven_reactor_check_fired",
        "riven_reactor_deregister",
    ] {
        let found = result
            .symbols
            .iter()
            .any(|d| d.name == fn_name && matches!(d.kind, DefKind::Function { .. }));
        assert!(
            found,
            "{} must be registered as a free fn (lib decl in time/src/lib.rvn)",
            fn_name
        );
    }
}
