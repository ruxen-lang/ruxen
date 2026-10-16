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
//! | B4 await  | `task_join_await_via_method_call_lowers`                   | §B4        |
//! | B7 (E1116)| `task_spawn_outside_async_rejected_e1116`                  | §B7        |
//! | E1116 reg | `e1116_registered_in_diagnostic_codes`                     | §B7        |
//!
//! Runtime tests (B3 round-robin polling, B6 drain-on-block_on-exit,
//! B10 drop semantics) land via the e2e fixture in commit 3.
//!
//! Discipline: all Ruxen source goes through `.rx` fixtures
//! (`feedback_no_inline_rx_in_pin_tests`).

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::ast::{ExprKind, LoopExpr, MatchExpr, Statement, TopLevelItem};
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
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
    let result = typeck_result(&rx("task_spawn_inside_async_accepted"));
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
    let result = typeck_result(&rx("task_yield_now_constructs"));
    let errs = errors(&result.diagnostics);
    assert!(
        errs.is_empty(),
        "Task.yield_now construction should typecheck clean, got: {:?}",
        errs
    );
}

// ─── B3 — block_on inline loop pumps the task queue ─────────────────

/// Spec B3: the AST-level block_on rewriter inserts a
/// `ruxen_executor_pump_tasks()` call as the first statement of each
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
    // ruxen_executor_pump_tasks's thread-local nullcheck).
    let source = rx("async_executor_block_on_no_await");
    let mut lx = Lexer::new(&source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let mut prog = p.parse().expect("parse");

    // Run the same async-lowering pass typeck would run.
    ruxen_core::async_lowering::lower_async_defs(&mut prog);

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
    let mut pending_arms_pump_before_yield = 0usize;
    walk_block(
        &main_fn.body,
        &mut pump_calls,
        &mut loops_with_pump_first,
        &mut pending_arms_pump_before_yield,
    );

    assert!(
        pump_calls >= 2,
        "expected at least one `ruxen_executor_pump_tasks()` call \
         at loop start and one in the Pending arm, found {pump_calls}",
    );
    assert!(
        loops_with_pump_first >= 1,
        "expected at least one Loop whose first statement is the pump call",
    );
    assert!(
        pending_arms_pump_before_yield >= 1,
        "expected at least one Poll.Pending arm to pump before Thread.yield_now",
    );
}

fn walk_block(
    block: &ruxen_core::parser::ast::Block,
    pump_calls: &mut usize,
    loops_with_pump_first: &mut usize,
    pending_arms_pump_before_yield: &mut usize,
) {
    for stmt in &block.statements {
        match stmt {
            Statement::Let(lb) => {
                if let Some(v) = &lb.value {
                    walk_expr(
                        v,
                        pump_calls,
                        loops_with_pump_first,
                        pending_arms_pump_before_yield,
                    );
                }
            }
            Statement::Expression(e) => walk_expr(
                e,
                pump_calls,
                loops_with_pump_first,
                pending_arms_pump_before_yield,
            ),
        }
    }
}

fn walk_expr(
    expr: &ruxen_core::parser::ast::Expr,
    pump_calls: &mut usize,
    loops_with_pump_first: &mut usize,
    pending_arms_pump_before_yield: &mut usize,
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
            walk_block(
                body,
                pump_calls,
                loops_with_pump_first,
                pending_arms_pump_before_yield,
            );
        }
        ExprKind::Block(b) => walk_block(
            b,
            pump_calls,
            loops_with_pump_first,
            pending_arms_pump_before_yield,
        ),
        ExprKind::If(if_expr) => {
            walk_expr(
                &if_expr.condition,
                pump_calls,
                loops_with_pump_first,
                pending_arms_pump_before_yield,
            );
            walk_block(
                &if_expr.then_body,
                pump_calls,
                loops_with_pump_first,
                pending_arms_pump_before_yield,
            );
            for el in &if_expr.elsif_clauses {
                walk_expr(
                    &el.condition,
                    pump_calls,
                    loops_with_pump_first,
                    pending_arms_pump_before_yield,
                );
                walk_block(
                    &el.body,
                    pump_calls,
                    loops_with_pump_first,
                    pending_arms_pump_before_yield,
                );
            }
            if let Some(b) = &if_expr.else_body {
                walk_block(
                    b,
                    pump_calls,
                    loops_with_pump_first,
                    pending_arms_pump_before_yield,
                );
            }
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            walk_expr(
                subject,
                pump_calls,
                loops_with_pump_first,
                pending_arms_pump_before_yield,
            );
            for a in arms {
                if is_pending_pattern(&a.pattern) && arm_pumps_before_yield(&a.body) {
                    *pending_arms_pump_before_yield += 1;
                }
                match &a.body {
                    ruxen_core::parser::ast::MatchArmBody::Expr(e) => walk_expr(
                        e,
                        pump_calls,
                        loops_with_pump_first,
                        pending_arms_pump_before_yield,
                    ),
                    ruxen_core::parser::ast::MatchArmBody::Block(b) => walk_block(
                        b,
                        pump_calls,
                        loops_with_pump_first,
                        pending_arms_pump_before_yield,
                    ),
                }
            }
        }
        ExprKind::While(w) => {
            walk_expr(
                &w.condition,
                pump_calls,
                loops_with_pump_first,
                pending_arms_pump_before_yield,
            );
            walk_block(
                &w.body,
                pump_calls,
                loops_with_pump_first,
                pending_arms_pump_before_yield,
            );
        }
        ExprKind::MethodCall { object, args, .. } => {
            walk_expr(
                object,
                pump_calls,
                loops_with_pump_first,
                pending_arms_pump_before_yield,
            );
            for a in args {
                walk_expr(
                    a,
                    pump_calls,
                    loops_with_pump_first,
                    pending_arms_pump_before_yield,
                );
            }
        }
        ExprKind::Call { callee, args, .. } => {
            walk_expr(
                callee,
                pump_calls,
                loops_with_pump_first,
                pending_arms_pump_before_yield,
            );
            for a in args {
                walk_expr(
                    a,
                    pump_calls,
                    loops_with_pump_first,
                    pending_arms_pump_before_yield,
                );
            }
        }
        _ => {}
    }
}

fn is_pump_call(expr: &ruxen_core::parser::ast::Expr) -> bool {
    if let ExprKind::Call { callee, args, .. } = &expr.kind {
        if args.is_empty() {
            if let ExprKind::Identifier(name) = &callee.kind {
                return name == "ruxen_executor_pump_tasks";
            }
        }
    }
    false
}

fn is_pending_pattern(pattern: &ruxen_core::parser::ast::Pattern) -> bool {
    matches!(
        pattern,
        ruxen_core::parser::ast::Pattern::Enum {
            path,
            variant,
            fields,
            ..
        } if path == &vec!["Poll".to_string()] && variant == "Pending" && fields.is_empty()
    )
}

fn arm_pumps_before_yield(body: &ruxen_core::parser::ast::MatchArmBody) -> bool {
    let ruxen_core::parser::ast::MatchArmBody::Block(block) = body else {
        return false;
    };
    let mut saw_pump = false;
    for stmt in &block.statements {
        match stmt {
            Statement::Let(lb) => {
                if lb.value.as_deref().is_some_and(is_pump_call) {
                    saw_pump = true;
                }
            }
            Statement::Expression(expr) => {
                if is_pump_call(expr) {
                    saw_pump = true;
                    continue;
                }
                if expr_contains_thread_yield_now(expr) {
                    return saw_pump;
                }
            }
        }
    }
    false
}

fn expr_contains_thread_yield_now(expr: &ruxen_core::parser::ast::Expr) -> bool {
    if matches!(
        &expr.kind,
        ExprKind::FieldAccess { object, field }
            if field == "yield_now"
                && matches!(&object.kind, ExprKind::Identifier(name) if name == "Thread")
    ) {
        return true;
    }
    match &expr.kind {
        ExprKind::If(if_expr) => {
            expr_contains_thread_yield_now(&if_expr.condition)
                || block_contains_thread_yield_now(&if_expr.then_body)
                || if_expr.elsif_clauses.iter().any(|e| {
                    expr_contains_thread_yield_now(&e.condition)
                        || block_contains_thread_yield_now(&e.body)
                })
                || if_expr
                    .else_body
                    .as_ref()
                    .is_some_and(block_contains_thread_yield_now)
        }
        ExprKind::Block(block) => block_contains_thread_yield_now(block),
        ExprKind::BinaryOp { left, right, .. } => {
            expr_contains_thread_yield_now(left) || expr_contains_thread_yield_now(right)
        }
        ExprKind::Call { callee, args, .. } => {
            expr_contains_thread_yield_now(callee)
                || args.iter().any(expr_contains_thread_yield_now)
        }
        ExprKind::MethodCall { object, args, .. } => {
            expr_contains_thread_yield_now(object)
                || args.iter().any(expr_contains_thread_yield_now)
        }
        _ => false,
    }
}

fn block_contains_thread_yield_now(block: &ruxen_core::parser::ast::Block) -> bool {
    block.statements.iter().any(|stmt| match stmt {
        Statement::Let(lb) => lb
            .value
            .as_deref()
            .is_some_and(expr_contains_thread_yield_now),
        Statement::Expression(expr) => expr_contains_thread_yield_now(expr),
    })
}

// ─── B7 — Task.spawn outside async rejected with E1116 ──────────────

/// Spec B7: `Task.spawn(...)` from a sync `def main` body must
/// be rejected with code E1116. Symmetric to E1112 (block_on inside
/// async).
#[test]
fn task_spawn_outside_async_rejected_e1116() {
    let result = typeck_result(&rx("task_spawn_outside_async_rejected"));
    let codes: Vec<String> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .filter_map(|d| d.code.clone())
        .collect();
    assert!(
        codes.iter().any(|c| c == "E1116"),
        "expected E1116 for Task.spawn in sync scope, got codes: {:?} (messages: {:?})",
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
    let result = typeck_result(&rx("task_join_constructs"));
    let errs = errors(&result.diagnostics);
    assert!(
        errs.is_empty(),
        "Task.join(h) construction + hand-poll should typecheck clean, got: {:?}",
        errs
    );
}

/// `Task.join(h).await` desugars through the class-static-method
/// awaitee shape (`describe_await` shape 2) now that async-lowering
/// walks bootstrap stdlib programs. TaskJoinFuture lives in
/// `library/std/future/src/lib.rx`; before the bootstrap-aware
/// lowering landed, the user-program walkers
/// `collect_class_static_returns_into` /
/// `collect_future_outputs_into` couldn't see it.
///
/// Same architectural change unblocks
/// `tests/release-e2e/cases/731_class_static_call_await.rx`
/// (the Async.sleep().await case).
///
/// Assertions:
///   1. The synth state-machine class for `driver` carries a
///      `__sub_0: TaskJoinFuture` field, proving the awaitee was
///      classified against the stdlib's TaskJoinFuture, not left
///      un-rewritten.
///   2. The fixture typechecks clean end-to-end (via the production
///      `type_check` path, which is bootstrap-aware).
#[test]
fn task_join_await_via_method_call_lowers() {
    let source = rx("task_join_await_via_method_call");
    let mut lx = Lexer::new(&source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let mut prog = p.parse().expect("parse");

    // Mirror the production lowering — pass bootstrap stdlib
    // programs in so `Task.join` / `TaskJoinFuture` are visible to
    // the awaitee classifier.
    let mut diags = Vec::new();
    let bootstrap_programs = ruxen_core::resolve::bootstrap::run_bootstrap(&mut diags);
    let bootstrap_refs: Vec<&ruxen_core::parser::ast::Program> =
        bootstrap_programs.iter().collect();
    ruxen_core::async_lowering::lower_async_defs_with_bootstrap(&mut prog, &bootstrap_refs);

    let synth_class = prog
        .items
        .iter()
        .find_map(|i| match i {
            TopLevelItem::Class(c) if c.name.contains("Driver") && c.name.starts_with("__") => {
                Some(c)
            }
            _ => None,
        })
        .expect("expected a synth state-machine class for `driver` after lowering");

    let has_taskjoin_sub = synth_class.fields.iter().any(|f| {
        f.name.starts_with("__sub_")
            && matches!(
                &f.type_expr,
                ruxen_core::parser::ast::TypeExpr::Named(path)
                    if path.segments.last().map(|s| s.as_str()) == Some("TaskJoinFuture")
            )
    });
    assert!(
        has_taskjoin_sub,
        "expected `__sub_*: TaskJoinFuture` field, got fields: {:?}",
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
        "Task.join(h).await fixture should typecheck clean, got: {:?}",
        errs
    );
}

// ─── Diagnostic registry sanity ─────────────────────────────────────

/// E1116 must appear in the diagnostic code registry so the
/// `compiler/ruxen_core/tests/error_code_registry.rs` walker (which
/// verifies every emitted code has a registered title + docs file)
/// stays happy. This is a thin sanity test that the registration
/// landed alongside the emission.
#[test]
fn e1116_registered_in_diagnostic_codes() {
    use ruxen_core::diagnostics::codes::lookup;
    let info = lookup("E1116").expect("E1116 must be registered in codes.rs");
    assert!(
        info.title.contains("Task.spawn"),
        "E1116 title should reference Task.spawn, got: {:?}",
        info.title
    );
}
