//! Pin tests for the async sub-phase 4A surface
//! (`docs/specs/stdlib/async_io.spec.md` partial — see the
//! report-back accompanying the 4A commit for the bootstrap-merge
//! gap that blocks the full `time.sleep(d)` future hand-off).
//!
//! What 4A ships in this commit:
//!   * The OS-level reactor (epoll on Linux, kqueue on macOS) in
//!     `library/std/future/runtime/reactor.c`, with a thread-local
//!     pointer + lifecycle hooks (`ruxen_reactor_acquire` /
//!     `_release` / `_park_current`).
//!   * `Context.executor` / `Context.drop` install/release the
//!     per-thread reactor (lib decls in
//!     `library/std/future/src/lib.rx`).
//!   * `Thread.yield_now`'s C body (`ruxen_thread_yield` in
//!     `library/std/time/runtime/time.c`) now delegates to
//!     `ruxen_reactor_park_current`, which blocks on the reactor
//!     when registrations are pending and falls back to
//!     `sched_yield` otherwise. The AST-level block_on rewriter
//!     emission is UNCHANGED — Pending arm still emits
//!     `Thread.yield_now` (per the coordinator's "zero
//!     compiler/src/ edits" constraint).
//!   * Reactor primitive free-fn lib decls land in
//!     `library/std/time/src/lib.rx` (the wire surface only —
//!     no user-side `time.sleep` ships in this commit).
//!
//! What's deferred:
//!   * `time.sleep(d) -> TimeSleepFuture` (Milestone 4A B1) — the
//!     hand-written future has user-bodied Ruxen methods (`def init`,
//!     `def var poll`, `def drop`) that the bootstrap-merge does not
//!     currently route through `resolve_item`. Adding that routing is
//!     a one-shot compiler/src/ edit; for 4A it's reported back to
//!     the coordinator rather than slipped in unilaterally.
//!   * The e2e `cases/725_time_sleep_block_on.rx` fixture is held
//!     until the gap above lands.
//!
//! What's covered here:
//!   * B3 — `Context` carries a `drop` lib decl (per-thread reactor
//!     teardown at Context scope exit).

use ruxen_core::codegen;
use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::resolve::symbols::DefKind;
use ruxen_core::typeck;
use std::process::Command;
use std::time::Instant as StdInstant;

fn typeck_result(source: &str) -> typeck::TypeCheckResult {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    typeck::type_check(&prog)
}

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

/// Compile `source` through the in-process pipeline and run the
/// resulting binary. Returns `(stdout, stderr, exit_ok)`.
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
        .filter(|d| d.level == DiagnosticLevel::Error)
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
/// the C-side `ruxen_thread_yield -> ruxen_reactor_park_current`
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
        d.name == "drop" && matches!(&d.kind, DefKind::Method { parent, .. } if *parent == ctx_id)
    });
    assert!(
        has_drop,
        "Context.drop must be registered (lib decl in future/src/lib.rx) \
         so MIR drop elaboration tears down the per-thread reactor"
    );
}

// ─── Reactor free-fn lib decls registered ──────────────────────────

/// The four reactor primitives — `ruxen_reactor_current_handle`,
/// `_register_timer`, `_check_fired`, `_deregister` — are declared as
/// free-fn lib decls in `library/std/time/src/lib.rx`. Surface check:
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
        "ruxen_reactor_current_handle",
        "ruxen_reactor_register_timer",
        "ruxen_reactor_check_fired",
        "ruxen_reactor_deregister",
    ] {
        let found = result
            .symbols
            .iter()
            .any(|d| d.name == fn_name && matches!(d.kind, DefKind::Function { .. }));
        assert!(
            found,
            "{} must be registered as a free fn (lib decl in time/src/lib.rx)",
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
    let source = rx("time_sleep_future_typechecks");
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
/// Mirrors the e2e fixture `725_time_sleep_block_on.rx` but druxen
/// through the in-process compile pipeline so it lives on the narrow
/// async_io.rs run target.
#[test]
fn time_sleep_round_trip_via_block_on() {
    let root = workspace_root();
    let fixture_path = root.join("tests/release-e2e/cases/725_time_sleep_block_on.rx");
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
    // The Ruxen program prints "ok" iff its in-program tolerance
    // window (40-200ms) was satisfied. The wall-clock check below is
    // a belt-and-braces sanity bound, capturing pathologies like the
    // program returning "ok" without actually parking (which would
    // exit far under 40ms — the Ruxen-side check protects against
    // that too, but a Rust-side bound makes the failure mode
    // diagnosable from the test output alone).
    assert!(
        stdout.trim() == "ok",
        "expected 'ok' (Ruxen-side tolerance check passed), got: \
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

// ─── B4-B6 e2e — AsyncFile round-trip via block_on ─────────────────

/// Sub-phase 4B B4+B5+B6 end-to-end: open-for-write, write contents,
/// open-for-read, read_to_string, assert equality. Drives all three
/// async file futures through the per-thread reactor (though on a
/// regular file the reads don't actually EAGAIN — they complete in
/// one syscall — so this is a smoke test for the surface assembly
/// rather than a stress test for reactor wake-on-readable. The
/// reactor-wake path is covered by tests/725 for timers and by 4C
/// fixtures for sockets where EAGAIN is the common case).
///
/// Expected output: `write_ok\nok\n`. Drives the same fixture as
/// `tests/release-e2e/cases/726_async_file_round_trip.rx`.
#[test]
fn async_file_round_trip_via_block_on() {
    let root = workspace_root();
    let fixture_path = root.join("tests/release-e2e/cases/726_async_file_round_trip.rx");
    let source = std::fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("read {}: {}", fixture_path.display(), e));

    let (stdout, stderr, ok) = compile_and_run(&source, "async_io_async_file_round_trip");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert_eq!(
        stdout.trim(),
        "write_ok\nok",
        "expected 'write_ok\\nok'; got stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

// ─── Phase 3 — linear N-await state-machine runtime equivalence gate ──
//
// EQUIVALENCE GATE for the async-lowering CFG unification (Phase 3 Path
// A). Every other in-process `cargo test -p ruxen_core` async test is
// typeck-only (async_lowering / async_negative) or drives the reactor
// `block_on` path (the tests above). NONE compiles-and-runs the linear
// `.await`-chain state machine that `lower_one_async_fn_with_await` +
// `build_multi_state_poll_body` produce — so deleting/replacing that
// builder could regress runtime behaviour with the suite still green.
//
// This pin closes that hole: an `async def chain()` with TWO chained
// `.await`s is hand-polled N+1 (=3) times, forcing the synthesised
// `self.__state` if-chain through state 0 (await make_int) → Pending,
// state 1 (await make_other) → Pending, state 2 (terminal) →
// Ready(42 + 35). The asserted stdout `77` only holds if the poll
// skeleton advances `__state`, stores each await result in its field,
// and folds the tail correctly. Mirrors e2e fixture
// `722_async_def_chained_await_handpoll.rx`, inlined so the gate lives
// in-crate and runs under `cargo test -p ruxen_core --test async_io`.
//
// The hand-poll driver (not `block_on`) is deliberate: it exercises the
// state-machine poll builder directly, with no reactor in the loop, so
// any divergence in the emitted `__state` skeleton shows up here.
#[test]
fn linear_chained_await_runtime_gate() {
    let source = "\
async def make_int() -> Int
  42
end

async def make_other() -> Int
  35
end

async def chain() -> Int
  let a = make_int().await
  let b = make_other().await
  a + b
end

def main
  var fut = chain()
  var ctx = Context.test_dummy
  var result = 0
  match (&var fut).poll(&var ctx)
    Poll.Ready(v) -> result = v
    Poll.Pending -> result = result
  end
  match (&var fut).poll(&var ctx)
    Poll.Ready(v) -> result = v
    Poll.Pending -> result = result
  end
  match (&var fut).poll(&var ctx)
    Poll.Ready(v) -> result = v
    Poll.Pending -> result = result
  end
  puts \"#{result}\"
end
";
    let (stdout, stderr, ok) = compile_and_run(source, "async_io_linear_chained_await_gate");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert_eq!(
        stdout.trim(),
        "77",
        "linear two-await chain must fold to 42 + 35 = 77; got stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// FIX 2 regression — crossing-local promotion when the only tail read is
/// inside an `EnumVariant`.
///
/// `expr_references_name` (which drives crossing-local promotion via
/// `stmts_reference_name`) used to have the same `_ => false` gap as the
/// rewriter: it missed `EnumVariant` (and `Yield`, etc.). So a pre-await
/// `let base = ...` whose ONLY post-await read is inside the tail
/// `Result.Ok(base + v)` was not seen as crossing, was not promoted to a
/// state-machine field, and so did not survive the suspend — yielding the
/// wrong fold (or an undefined local) after the resume.
///
/// `base = 100` is computed before the await, `v = 5` comes from the
/// awaited future, and the tail is `Result.Ok(base + v)`. With promotion,
/// the result is 105.
#[test]
fn linear_enum_variant_tail_promotes_crossing_local() {
    let source = "\
async def make_five() -> Int
  5
end

async def with_base -> Result[Int, Int]
  let base: Int = 100
  let v = make_five().await
  Result.Ok(base + v)
end

def main
  let res = block_on(with_base())
  match res
    Ok(v)  -> puts \"#{v}\"
    Err(e) -> puts \"err #{e}\"
  end
end
";
    let (stdout, stderr, ok) = compile_and_run(source, "async_io_linear_enum_variant_promote");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert_eq!(
        stdout.trim(),
        "105",
        "pre-await `base` read only inside the `Result.Ok(base + v)` tail must \
         be promoted to a crossing-local field; fold is 100 + 5 = 105. \
         got stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

// ─── Phase 3B — LOOP state-machine runtime equivalence gates ─────────
//
// EQUIVALENCE GATES for the async-lowering CFG unification of the LOOP
// shapes (Phase 3B). The linear gate above pins the non-loop path; these
// two pin the two loop shapes that Phase 3B folds into the unified
// `build_poll_body_cfg` (via `Edge::Loop`):
//
//   * `loop_single_await_runtime_gate` — a `while` loop with ONE `.await`
//     per iteration (mirrors e2e `728d_async_loop_minimal.rx`),
//   * `loop_multi_await_runtime_gate` — a `while` loop with TWO `.await`s
//     per iteration (mirrors e2e `728f_async_loop_multi_await.rx`).
//
// Both drive the synthesised state machine through `block_on`, which
// polls the future repeatedly: each `TickFuture` returns Pending once
// (no reactor I/O — `Thread.yield_now` just spins) then Ready, so the
// outer loop machine must advance across multiple iterations AND
// re-init its per-iteration sub-future each pass. The asserted `sum`
// only holds if the loop machine: (a) runs pre-loop init once, (b)
// re-runs body_pre_await each iteration, (c) folds each await result
// into the crossing-local accumulator, (d) re-evaluates the loop cond
// on the back-edge, and (e) produces the post-loop tail. Any divergence
// in the emitted loop skeleton (state guard, keep_iterating/pending_exit
// flags, __sub_ready re-init, __phase advance) shows up as a wrong sum.
//
// These gate the deletion of the four hand-specialized loop builders:
// they MUST stay green when loop lowering is rerouted through the
// unified Cfg path.

const TICK_FUTURE_PRELUDE: &str = "\
class TickFuture
  ticks: Int
  payload: Int
  include Future
  type Output = Int

  def init(ticks: Int, payload: Int)
    self.ticks = ticks
    self.payload = payload
  end

  def self.make(ticks: Int, payload: Int) -> TickFuture
    TickFuture.new(ticks, payload)
  end

  def var poll(cx: &var Context) -> Poll[Int]
    if self.ticks == 0
      Poll.Ready(self.payload)
    else
      self.ticks = self.ticks - 1
      Poll.Pending
    end
  end
end
";

/// Single-await loop: `while i < 2 { let v = TickFuture.make(1, 10).await; sum += v; i += 1 }`.
/// Mirrors `728d_async_loop_minimal.rx`. Two iterations, each TickFuture
/// goes Pending→Ready, so sum = 10 + 10 = 20.
#[test]
fn loop_single_await_runtime_gate() {
    let source = format!(
        "{TICK_FUTURE_PRELUDE}
async def loop_two -> Int
  var i: Int = 0
  var sum: Int = 0
  while i < 2
    let v = TickFuture.make(1, 10).await
    sum = sum + v
    i = i + 1
  end
  sum
end

def main
  let total = block_on(loop_two())
  puts \"#{{total}}\"
end
"
    );
    let (stdout, stderr, ok) = compile_and_run(&source, "async_io_loop_single_await_gate");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert_eq!(
        stdout.trim(),
        "20",
        "single-await loop must fold to 10 + 10 = 20 over two iterations; \
         got stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// Multi-await loop: two `.await`s per iteration, the second reading the
/// first's result. Mirrors `728f_async_loop_multi_await.rx`. Two
/// iterations:
///   iter 0 (i=0): a=100, b=110, sum=210
///   iter 1 (i=1): a=110, b=120, sum=440
#[test]
fn loop_multi_await_runtime_gate() {
    let source = format!(
        "{TICK_FUTURE_PRELUDE}
async def loop_two_awaits -> Int
  var i: Int = 0
  var sum: Int = 0
  while i < 2
    let a = TickFuture.make(1, 100 + i * 10).await
    let b = TickFuture.make(1, a + 10).await
    sum = sum + a + b
    i = i + 1
  end
  sum
end

def main
  let total = block_on(loop_two_awaits())
  puts \"#{{total}}\"
end
"
    );
    let (stdout, stderr, ok) = compile_and_run(&source, "async_io_loop_multi_await_gate");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert_eq!(
        stdout.trim(),
        "440",
        "multi-await loop must fold to 210 then 440 over two iterations; \
         got stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

/// FIX 1 regression — crossing-local read inside an `EnumVariant` tail.
///
/// `rewrite_arg_refs_in_block`/`_in_expr` used to be a hand-rolled walker
/// whose `_ => {}` arm silently dropped `ExprKind::EnumVariant` (among
/// others: MacroCall, Yield, SafeNav(Call), IfLet, WhileLet, While, For,
/// Loop, UnsafeBlock). So a crossing-local read inside e.g. `Result.Ok(acc)`
/// in a poll-body tail was NOT rewritten to `self.acc`, emitting an
/// undefined-local in the generated state machine.
///
/// This fn folds an accumulator across a two-iteration await loop, then
/// produces the terminal tail `Result.Ok(acc)` — an `EnumVariant` whose
/// argument reads the crossing-local `acc`. With the gap, `acc` stays a
/// bare identifier (not `self.acc`) and the binary either fails to compile
/// or prints the wrong value. Expected: acc = 10 + 10 = 20.
#[test]
fn loop_enum_variant_tail_crossing_local_runtime_gate() {
    let source = format!(
        "{TICK_FUTURE_PRELUDE}
async def fold_to_result -> Result[Int, Int]
  var i: Int = 0
  var acc: Int = 0
  while i < 2
    let v = TickFuture.make(1, 10).await
    acc = acc + v
    i = i + 1
  end
  Result.Ok(acc)
end

def main
  let res = block_on(fold_to_result())
  match res
    Ok(v)  -> puts \"#{{v}}\"
    Err(e) -> puts \"err #{{e}}\"
  end
end
"
    );
    let (stdout, stderr, ok) = compile_and_run(&source, "async_io_loop_enum_variant_tail_gate");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    assert_eq!(
        stdout.trim(),
        "20",
        "crossing-local `acc` inside the `Result.Ok(acc)` EnumVariant tail \
         must be rewritten to `self.acc`; fold is 10 + 10 = 20. \
         got stdout=[{}] stderr=[{}]",
        stdout,
        stderr
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
