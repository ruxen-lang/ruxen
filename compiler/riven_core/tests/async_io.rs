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

use riven_core::codegen;
use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::resolve::symbols::DefKind;
use riven_core::typeck;
use std::process::Command;
use std::time::Instant as StdInstant;

fn typeck_result(source: &str) -> typeck::TypeCheckResult {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    typeck::type_check(&prog)
}

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
/// resulting binary. Returns `(stdout, stderr, exit_ok)`.
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
        output.status.success(),
    )
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

// ─── B1 — TimeSleepFuture class shape ──────────────────────────────

/// `TimeSleepFuture` lifts as a stdlib class with `include Future`,
/// `type Output = ()`, and the three user-bodied methods `init`,
/// `poll`, `drop`. This pin asserts the bootstrap pass-2 routing
/// (commit `b74546d`) actually picks up user-body methods on
/// stdlib-declared classes — without that fix the class would
/// register but the method bodies would be silently dropped.
#[test]
fn time_sleep_future_class_resolves_and_typechecks() {
    let source = rvn("time_sleep_future_typechecks");
    let result = typeck_result(&source);
    let errors = error_messages(&result.diagnostics);
    assert!(
        errors.is_empty(),
        "TimeSleepFuture fixture must typecheck cleanly: {:?}",
        errors
    );

    let class_id = result
        .symbols
        .iter()
        .find(|def| def.name == "TimeSleepFuture" && matches!(def.kind, DefKind::Class { .. }))
        .map(|def| def.id)
        .expect("TimeSleepFuture must be registered after bootstrap");

    for method in ["init", "poll", "drop"] {
        let found = result.symbols.iter().any(|d| {
            d.name == method
                && matches!(&d.kind, DefKind::Method { parent, .. } if *parent == class_id)
        });
        assert!(
            found,
            "TimeSleepFuture.{} must register as a method on the class \
             (bootstrap pass-2 routing — task #14 commit b74546d)",
            method
        );
    }

    // The Async class (std.future.Async) carries the user-facing
    // async runtime surface. After sub-phase 4A consolidation,
    // Async.sleep is a class-static method (not a free fn), so the
    // surface check is "the class is registered" rather than "a
    // top-level Function named sleep exists." Method dispatch is
    // covered end-to-end by `time_sleep_round_trip_via_block_on`.
    result
        .symbols
        .iter()
        .find(|def| def.name == "Async" && matches!(def.kind, DefKind::Class { .. }))
        .expect("class `Async` (std.future.Async) must be registered after bootstrap");
}

// ─── B1 e2e — block_on(sleep(d)) round-trip ────────────────────────

/// Sub-phase 4A B1 end-to-end: `block_on(sleep(Duration.from_millis(50)))`
/// drives the TimeSleepFuture through the OS event reactor and returns
/// after ~50ms. Tolerance window 40-200ms — wide enough for CI jitter,
/// narrow enough to catch a no-op sleep or a runaway spin (the latter
/// would either return instantly or burn CPU until the test harness
/// timeout).
///
/// Mirrors the e2e fixture `725_time_sleep_block_on.rvn` but driven
/// through the in-process compile pipeline so it lives on the narrow
/// async_io.rs run target.
#[test]
fn time_sleep_round_trip_via_block_on() {
    let root = workspace_root();
    let fixture_path = root.join("tests/release-e2e/cases/725_time_sleep_block_on.rvn");
    let source = std::fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("read {}: {}", fixture_path.display(), e));

    let wall_start = StdInstant::now();
    let (stdout, stderr, ok) = compile_and_run(&source, "async_io_time_sleep_round_trip");
    let wall_elapsed = wall_start.elapsed();

    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    // The Riven program prints "ok" iff its in-program tolerance
    // window (40-200ms) was satisfied. The wall-clock check below is
    // a belt-and-braces sanity bound, capturing pathologies like the
    // program returning "ok" without actually parking (which would
    // exit far under 40ms — the Riven-side check protects against
    // that too, but a Rust-side bound makes the failure mode
    // diagnosable from the test output alone).
    assert!(
        stdout.trim() == "ok",
        "expected 'ok' (Riven-side tolerance check passed), got: \
         stdout=[{}] stderr=[{}] wall_elapsed={:?}",
        stdout,
        stderr,
        wall_elapsed
    );
    // Compile+link+spawn dominate wall time; the 50ms sleep is a
    // small fraction. Cap at 30s to catch a hang (e.g. reactor
    // never wakes), but don't lower-bound — the program already does.
    assert!(
        wall_elapsed < std::time::Duration::from_secs(30),
        "round trip exceeded 30s wall — reactor stuck? wall_elapsed={:?}",
        wall_elapsed
    );
}
