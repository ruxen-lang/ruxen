//! Pin tests for async sub-phase 5 — `Task.spawn` task scheduler.
//! Spec: `docs/specs/stdlib/task_spawn.spec.md`.
//!
//! Commit 1 scope (this file): compile-time / AST-shape coverage of
//! the surface introduced in this commit. Runtime end-to-end pins
//! land in `tests/release-e2e/cases/728_*` once commit 2 ships the
//! `task_join(h).await` desugar.
//!
//! | Behaviour | Test                                                       | Spec       |
//! |-----------|------------------------------------------------------------|------------|
//! | B1        | `task_spawn_inside_async_typechecks_clean`                 | §B1        |
//! | B2        | `task_yield_now_constructs_taskyieldfuture`                | §B2        |
//! | B3        | `block_on_inline_loop_pumps_task_queue`                    | §B3        |
//! | B4 (c2)   | `task_join_constructs_and_typechecks`                      | §B4        |
//! | B4 await  | `task_join_await_via_method_call_is_deferred` (`#[ignore]`)| §B4        |
//! | B7 (E1116)| `task_spawn_outside_async_rejected_e1116`                  | §B7        |
//! | E1116 reg | `e1116_registered_in_diagnostic_codes`                     | §B7        |
//!
//! Runtime tests (B3 round-robin polling, B6 drain-on-block_on-exit,
//! B10 drop semantics) land via the e2e fixture in commit 3.
//!
//! Discipline: all Riven source goes through `.rvn` fixtures
//! (`feedback_no_inline_rvn_in_pin_tests`).

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::ast::{ExprKind, LoopExpr, MatchExpr, Statement, TopLevelItem};
use riven_core::parser::Parser;
use riven_core::typeck;

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

fn errors(diags: &[Diagnostic]) -> Vec<(Option<String>, String)> {
    diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .map(|d| (d.code.clone(), d.message.clone()))
        .collect()
}

// ─── B1 — Task.spawn inside async typechecks ────────────────────────

/// Spec B1: `Task.spawn_raw(future)` inside an `async def` body
/// typechecks without diagnostics. The fixture also exercises a
/// `block_on(driver())` at sync top-level to ensure the surrounding
/// shape is valid.
#[test]
fn task_spawn_inside_async_typechecks_clean() {
    let result = typeck_result(&rvn("task_spawn_inside_async_accepted"));
    let errs = errors(&result.diagnostics);
    assert!(
        errs.is_empty(),
        "Task.spawn_raw inside async def should typecheck clean, got: {:?}",
        errs
    );
}

// ─── B2 — Task.yield_now constructs a TaskYieldFuture ───────────────

/// Spec B2: `Task.yield_now` is a no-arg class-static method that
/// returns a `TaskYieldFuture`. The TaskYieldFuture's `poll` returns
/// Pending then Ready. This compile-time pin verifies the
/// construction + .poll surface typechecks (without running it).
#[test]
fn task_yield_now_constructs_taskyieldfuture() {
    let result = typeck_result(&rvn("task_yield_now_constructs"));
    let errs = errors(&result.diagnostics);
    assert!(
        errs.is_empty(),
        "Task.yield_now construction should typecheck clean, got: {:?}",
        errs
    );
}

// ─── B3 — block_on inline loop pumps the task queue ─────────────────

/// Spec B3: the AST-level block_on rewriter inserts a
/// `riven_executor_pump_tasks()` call as the first statement of each
/// iteration of the inline poll loop. This pin walks the rewritten
/// AST and asserts the pump call is present.
///
/// Why a structural check: the runtime behaviour (round-robin
/// polling of every queued task) is exercised by the e2e fixture
/// (cases/728_*) in commit 3. The AST pin protects against accidental
/// removal of the pump call — that would silently drop sub-phase 5
/// without breaking the v1 block_on regression suite, since pre-spawn
/// fixtures never use Task.spawn.
#[test]
fn block_on_inline_loop_pumps_task_queue() {
    // Reuse an existing block_on fixture — the pump call is emitted
    // at every block_on rewrite site regardless of whether the
    // surrounded future ever uses Task.spawn (zero-overhead by
    // riven_executor_pump_tasks's thread-local nullcheck).
    let source = rvn("async_executor_block_on_no_await");
    let mut lx = Lexer::new(&source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let mut prog = p.parse().expect("parse");

    // Run the same async-lowering pass typeck would run.
    riven_core::async_lowering::lower_async_defs(&mut prog);

    // Find the rewritten `main` body. The block_on inline loop is a
    // nested Block → Loop expression somewhere inside.
    let main_fn = prog
        .items
        .iter()
        .find_map(|i| match i {
            TopLevelItem::Function(f) if f.name == "main" => Some(f),
            _ => None,
        })
        .expect("main fn present in fixture");

    let mut pump_calls = 0usize;
    let mut loops_with_pump_first = 0usize;
    walk_block(&main_fn.body, &mut pump_calls, &mut loops_with_pump_first);

    assert!(
        pump_calls >= 1,
        "expected at least one `riven_executor_pump_tasks()` call \
         in the rewritten block_on loop body, found 0",
    );
    assert!(
        loops_with_pump_first >= 1,
        "expected at least one Loop whose first statement is the pump call",
    );
}

fn walk_block(
    block: &riven_core::parser::ast::Block,
    pump_calls: &mut usize,
    loops_with_pump_first: &mut usize,
) {
    for stmt in &block.statements {
        match stmt {
            Statement::Let(lb) => {
                if let Some(v) = &lb.value {
                    walk_expr(v, pump_calls, loops_with_pump_first);
                }
            }
            Statement::Expression(e) => walk_expr(e, pump_calls, loops_with_pump_first),
        }
    }
}

fn walk_expr(
    expr: &riven_core::parser::ast::Expr,
    pump_calls: &mut usize,
    loops_with_pump_first: &mut usize,
) {
    if is_pump_call(expr) {
        *pump_calls += 1;
    }
    match &expr.kind {
        ExprKind::Loop(LoopExpr { body, .. }) => {
            // Loop body's first statement should be the pump call.
            if let Some(Statement::Expression(first)) = body.statements.first() {
                if is_pump_call(first) {
                    *loops_with_pump_first += 1;
                }
            }
            walk_block(body, pump_calls, loops_with_pump_first);
        }
        ExprKind::Block(b) => walk_block(b, pump_calls, loops_with_pump_first),
        ExprKind::If(if_expr) => {
            walk_expr(&if_expr.condition, pump_calls, loops_with_pump_first);
            walk_block(&if_expr.then_body, pump_calls, loops_with_pump_first);
            for el in &if_expr.elsif_clauses {
                walk_expr(&el.condition, pump_calls, loops_with_pump_first);
                walk_block(&el.body, pump_calls, loops_with_pump_first);
            }
            if let Some(b) = &if_expr.else_body {
                walk_block(b, pump_calls, loops_with_pump_first);
            }
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            walk_expr(subject, pump_calls, loops_with_pump_first);
            for a in arms {
                match &a.body {
                    riven_core::parser::ast::MatchArmBody::Expr(e) => {
                        walk_expr(e, pump_calls, loops_with_pump_first)
                    }
                    riven_core::parser::ast::MatchArmBody::Block(b) => {
                        walk_block(b, pump_calls, loops_with_pump_first)
                    }
                }
            }
        }
        ExprKind::While(w) => {
            walk_expr(&w.condition, pump_calls, loops_with_pump_first);
            walk_block(&w.body, pump_calls, loops_with_pump_first);
        }
        ExprKind::MethodCall { object, args, .. } => {
            walk_expr(object, pump_calls, loops_with_pump_first);
            for a in args {
                walk_expr(a, pump_calls, loops_with_pump_first);
            }
        }
        ExprKind::Call { callee, args, .. } => {
            walk_expr(callee, pump_calls, loops_with_pump_first);
            for a in args {
                walk_expr(a, pump_calls, loops_with_pump_first);
            }
        }
        _ => {}
    }
}

fn is_pump_call(expr: &riven_core::parser::ast::Expr) -> bool {
    if let ExprKind::Call { callee, args, .. } = &expr.kind {
        if args.is_empty() {
            if let ExprKind::Identifier(name) = &callee.kind {
                return name == "riven_executor_pump_tasks";
            }
        }
    }
    false
}

// ─── B7 — Task.spawn outside async rejected with E1116 ──────────────

/// Spec B7: `Task.spawn_raw(...)` from a sync `def main` body must
/// be rejected with code E1116. Symmetric to E1112 (block_on inside
/// async).
#[test]
fn task_spawn_outside_async_rejected_e1116() {
    let result = typeck_result(&rvn("task_spawn_outside_async_rejected"));
    let codes: Vec<String> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .filter_map(|d| d.code.clone())
        .collect();
    assert!(
        codes.iter().any(|c| c == "E1116"),
        "expected E1116 for Task.spawn_raw in sync scope, got codes: {:?} (messages: {:?})",
        codes,
        errors(&result.diagnostics),
    );
}

// ─── B4 (commit 2) — Task.join + TaskJoinFuture hand-poll surface ───

/// Spec B4 (commit 2 surface): `Task.join(handle)` typechecks and
/// returns a `TaskJoinFuture` value the caller can hand-poll via
/// `(&var join).poll(&var cx)`. v1 user surface — the `.await`
/// sugar is gated on the deferred async-lowering fix (see
/// `task_join_await_via_method_call_is_deferred` below).
///
/// The fixture exercises:
///   1. `Task.join(h)` constructs cleanly inside an `async def`.
///   2. The returned value satisfies the Future mixin (has a
///      `.poll(&var Context) -> Poll[Int]` method).
///   3. The match arms on `Poll.Ready(v)` / `Poll.Pending`
///      typecheck against `Poll[Int]` — proving TaskJoinFuture's
///      `type Output = Int` is wired through correctly.
#[test]
fn task_join_constructs_and_typechecks() {
    let result = typeck_result(&rvn("task_join_constructs"));
    let errs = errors(&result.diagnostics);
    assert!(
        errs.is_empty(),
        "Task.join(h) construction + hand-poll should typecheck clean, got: {:?}",
        errs
    );
}

/// Deferred pin — `Task.join(h).await` requires async-lowering to
/// see bootstrap-loaded stdlib classes. TaskJoinFuture lives in
/// `library/std/future/src/lib.rvn`, invisible to the user-program
/// AST walkers `collect_class_static_returns` /
/// `collect_future_outputs`.
///
/// Same architectural gap defers `tests/release-e2e/cases/
/// 731_class_static_call_await.rvn.deferred` (the Async.sleep()
/// .await case). When the desugar learns to walk bootstrap
/// programs too:
///   1. Drop the `#[ignore]` here.
///   2. Verify the synth state-machine class for `driver` carries
///      a `__sub_0: TaskJoinFuture` field.
///   3. Drop the `.deferred` suffix on 731.
#[test]
#[ignore = "Task.join(h).await needs async-lowering to see bootstrap stdlib classes; same gap as 731_class_static_call_await.rvn.deferred"]
fn task_join_await_via_method_call_is_deferred() {
    let source = rvn("task_join_await_deferred");
    let mut lx = Lexer::new(&source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let mut prog = p.parse().expect("parse");

    riven_core::async_lowering::lower_async_defs(&mut prog);

    let synth_class = prog
        .items
        .iter()
        .find_map(|i| match i {
            TopLevelItem::Class(c) if c.name.contains("Driver") && c.name.starts_with("__") => {
                Some(c)
            }
            _ => None,
        })
        .expect(
            "WHEN UNIGNORED: expected a synth state-machine class for `driver` after lowering",
        );

    let has_taskjoin_sub = synth_class.fields.iter().any(|f| {
        f.name.starts_with("__sub_")
            && matches!(
                &f.type_expr,
                riven_core::parser::ast::TypeExpr::Named(path)
                    if path.segments.last().map(|s| s.as_str()) == Some("TaskJoinFuture")
            )
    });
    assert!(
        has_taskjoin_sub,
        "WHEN UNIGNORED: expected `__sub_*: TaskJoinFuture` field, got fields: {:?}",
        synth_class
            .fields
            .iter()
            .map(|f| (&f.name, &f.type_expr))
            .collect::<Vec<_>>()
    );

    let result = typeck_result(&source);
    let errs = errors(&result.diagnostics);
    assert!(
        errs.is_empty(),
        "WHEN UNIGNORED: Task.join(h).await fixture should typecheck clean, got: {:?}",
        errs
    );
}

// ─── Diagnostic registry sanity ─────────────────────────────────────

/// E1116 must appear in the diagnostic code registry so the
/// `compiler/riven_core/tests/error_code_registry.rs` walker (which
/// verifies every emitted code has a registered title + docs file)
/// stays happy. This is a thin sanity test that the registration
/// landed alongside the emission.
#[test]
fn e1116_registered_in_diagnostic_codes() {
    use riven_core::diagnostics::codes::lookup;
    let info = lookup("E1116").expect("E1116 must be registered in codes.rs");
    assert!(
        info.title.contains("Task.spawn"),
        "E1116 title should reference Task.spawn, got: {:?}",
        info.title
    );
}
