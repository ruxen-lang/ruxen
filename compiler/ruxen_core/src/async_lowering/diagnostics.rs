//! Async surface diagnostics (E1112 block_on-in-async, E1115 await-in-loop,
//! E1116 task-spawn-outside-async), expressed as `parser::visit::Visit` impls.
//!
//! The ONE exhaustive `walk_expr` (Phase 1) replaces three hand-rolled
//! `*_in_expr` recursions that each had a `_ => {}` catch-all and had drifted:
//! `.await` / `block_on(...)` / `Task.spawn(...)` nested inside expression
//! forms the old walkers never matched (`EnumVariant`, `SafeNav`/`SafeNavCall`,
//! `IfLet`, `WhileLet`, `Yield`, `MacroCall`, ...) were invisible. Routing
//! through the exhaustive `walk_expr` means adding an AST variant is a compile
//! error in the visitor, not a silent drift.
//!
//! The three checks share one visitor, [`AsyncScopeVisitor`], parameterised
//! by [`AsyncCheck`]: it carries the `in_async` / `in_loop` scope state and
//! flags the node the selected check looks for. `Visit` covers
//! expr/stmt/block/pattern/type but NOT `TopLevelItem`, so one small
//! top-level item walk (flipping `in_async` at fn/method boundaries) hands
//! each body to the visitor.

use crate::parser::ast::*;
use crate::parser::visit::{walk_expr, Visit};

/// The async-scope rule a [`AsyncScopeVisitor`] enforces.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AsyncCheck {
    /// E1115: `.await` inside a `loop`/`while`/`for`/`while let` body that
    /// the lowering does not handle.
    ///
    /// Spec: docs/specs/syntax/async_lowering_loop_await.spec.md.
    /// Error doc: docs/errors/E1115.md.
    AwaitInLoop,
    /// E1112: a `block_on(...)` call inside an async function or closure.
    ///
    /// This MUST run BEFORE `lower_async_defs` rewrites async fn bodies into
    /// synthesised state-machine classes: once the rewrite fires, the
    /// original `block_on(...)` call ends up inside the generated `poll`
    /// method (which is itself NOT marked async), so the resolver's
    /// async_scope_depth check would not find it.
    ///
    /// Spec: docs/specs/stdlib/executor.spec.md B6.
    /// Error doc: docs/errors/E1112.md.
    BlockOnInAsync,
    /// E1116: a `Task.spawn(...)` call outside an async function or closure.
    /// There is no executor to enqueue into in sync context.
    /// `Task.spawn_raw(...)` stays available to sync runtime code that
    /// establishes an executor by driving `block_on` itself (Rondo's accept
    /// loop). The parser folds `Task.spawn(x)` into `MethodCall { object:
    /// Identifier("Task"), method: "spawn", args }`.
    ///
    /// Spec: docs/specs/stdlib/task_spawn.spec.md §B7.
    /// Error doc: docs/errors/E1116.md.
    TaskSpawnOutsideAsync,
}

/// One walker for the three async-scope diagnostics. `in_async` and
/// `in_loop` are visitor state: the loop forms set `in_loop` for their
/// per-iteration parts, and a nested closure opens a new scope (see the
/// `Closure` arm).
struct AsyncScopeVisitor<'a> {
    check: AsyncCheck,
    in_async: bool,
    in_loop: bool,
    diags: &'a mut Vec<crate::diagnostics::Diagnostic>,
}

impl AsyncScopeVisitor<'_> {
    /// Push this check's diagnostic when `e` is the node it rejects in the
    /// current scope.
    fn flag(&mut self, e: &Expr) {
        let (message, code) = match self.check {
            AsyncCheck::AwaitInLoop => {
                if !(self.in_async && self.in_loop && matches!(e.kind, ExprKind::Await(_))) {
                    return;
                }
                (
                    "`.await` inside a `loop` / `while` / `for` body is not yet \
                     supported; v1 lowering does not build per-iteration state \
                     machines. Hand-poll the future with `match fut.poll(&var cx)` \
                     inside the loop instead, or restructure to chained `.await`s \
                     outside the loop.",
                    "E1115",
                )
            }
            AsyncCheck::BlockOnInAsync => {
                // No arity gate: even a malformed `block_on()` should produce
                // E1112 rather than a confusing arity error first.
                let is_block_on = matches!(
                    &e.kind,
                    ExprKind::Call { callee, .. }
                        if matches!(&callee.kind, ExprKind::Identifier(name) if name == "block_on")
                );
                if !(self.in_async && is_block_on) {
                    return;
                }
                (
                    "`block_on` cannot be called inside an `async` function or closure — use `.await` to await a future in an async context",
                    "E1112",
                )
            }
            AsyncCheck::TaskSpawnOutsideAsync => {
                let is_task_spawn = matches!(
                    &e.kind,
                    ExprKind::MethodCall { object, method, .. }
                        if method == "spawn"
                            && matches!(&object.kind, ExprKind::Identifier(name) if name == "Task")
                );
                if self.in_async || !is_task_spawn {
                    return;
                }
                (
                    "`Task.spawn` can only be called inside an `async` function or closure — there is no executor to enqueue into in sync context",
                    "E1116",
                )
            }
        };
        self.diags
            .push(crate::diagnostics::Diagnostic::error_with_code(
                message,
                e.span.clone(),
                code,
            ));
    }

    /// Walk `parts` with `in_loop` set, restoring the outer value after.
    fn in_loop_scope(&mut self, parts: impl FnOnce(&mut Self)) {
        let prev = self.in_loop;
        self.in_loop = true;
        parts(self);
        self.in_loop = prev;
    }
}

impl Visit for AsyncScopeVisitor<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        // Every match keeps walking, so each offending node in one body gets
        // its own diagnostic in a single pass.
        self.flag(e);
        match &e.kind {
            // `While`/`WhileLet` evaluate their condition/value once per
            // iteration, so an `.await` there is still a per-iteration
            // suspend point: `in_loop` covers them too.
            ExprKind::Loop(LoopExpr { body, .. }) => {
                self.in_loop_scope(|v| v.visit_block(body));
            }
            ExprKind::While(WhileExpr {
                condition, body, ..
            }) => {
                self.in_loop_scope(|v| {
                    v.visit_expr(condition);
                    v.visit_block(body);
                });
            }
            ExprKind::WhileLet(WhileLetExpr {
                pattern,
                value,
                body,
                ..
            }) => {
                self.visit_pattern(pattern);
                self.in_loop_scope(|v| {
                    v.visit_expr(value);
                    v.visit_block(body);
                });
            }
            // The iterable is evaluated ONCE outside the loop, so `.await`
            // there is NOT in_loop: it is a normal pre-loop suspend the
            // segmenter can handle. Only the body runs per iteration.
            ExprKind::For(ForExpr {
                pattern,
                iterable,
                body,
                ..
            }) => {
                self.visit_pattern(pattern);
                self.visit_expr(iterable);
                self.in_loop_scope(|v| v.visit_block(body));
            }
            ExprKind::Closure(c) => match self.check {
                // A closure's `.await`s are scoped to the closure body and
                // handled by the lowering of THAT body, so E1115 does not
                // descend into it.
                AsyncCheck::AwaitInLoop => {}
                // The closure's async-ness OVERRIDES the surrounding scope:
                // a sync closure inside an async fn is a sync scope, an async
                // closure inside a sync fn is an async one.
                AsyncCheck::BlockOnInAsync | AsyncCheck::TaskSpawnOutsideAsync => {
                    let prev = self.in_async;
                    self.in_async = c.is_async;
                    match &c.body {
                        ClosureBody::Expr(ex) => self.visit_expr(ex),
                        ClosureBody::Block(b) => self.visit_block(b),
                    }
                    self.in_async = prev;
                }
            },
            // Everything else: recurse through the exhaustive `walk_expr`,
            // carrying the current flags.
            _ => walk_expr(self, e),
        }
    }
}

/// Walk `program` and collect an E1115 diagnostic for every `.await` found
/// inside a `loop`/`while`/`for` body in an async scope, EXCEPT when the
/// enclosing async fn body matches a loop shape the lowering handles.
///
/// Spec: docs/specs/syntax/async_lowering_loop_await.spec.md.
pub fn collect_await_in_loop_diagnostics(program: &Program) -> Vec<crate::diagnostics::Diagnostic> {
    collect(program, AsyncCheck::AwaitInLoop)
}

/// Walk `program` BEFORE `lower_async_defs` runs and collect a diagnostic for
/// every `block_on(...)` call found inside an async function or async closure.
pub fn collect_block_on_in_async_diagnostics(
    program: &Program,
) -> Vec<crate::diagnostics::Diagnostic> {
    collect(program, AsyncCheck::BlockOnInAsync)
}

/// Walk `program` and collect a diagnostic for every `Task.spawn(...)` call
/// found OUTSIDE an async function or async closure.
pub fn collect_task_spawn_outside_async_diagnostics(
    program: &Program,
) -> Vec<crate::diagnostics::Diagnostic> {
    collect(program, AsyncCheck::TaskSpawnOutsideAsync)
}

fn collect(program: &Program, check: AsyncCheck) -> Vec<crate::diagnostics::Diagnostic> {
    let mut diags = Vec::new();
    for item in &program.items {
        collect_in_item(item, check, /*in_async=*/ false, &mut diags);
    }
    diags
}

fn collect_in_item(
    item: &TopLevelItem,
    check: AsyncCheck,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    match item {
        TopLevelItem::Function(func) => {
            let scope_async = in_async || func.is_async;
            // E1115 skips a body the async lowering handles. The source of
            // truth is `segment_cfg`: if it accepts the body (no-await /
            // linear-N / single- or multi-await loop), the lowering produces
            // a valid state machine and there is no unsupported
            // await-in-loop to diagnose. Bodies it REJECTS (e.g. await in
            // the loop cond) are scanned and still get E1115.
            if check == AsyncCheck::AwaitInLoop
                && scope_async
                && super::cfg::segment_cfg(&func.body).is_some()
            {
                return;
            }
            scan_block(&func.body, check, scope_async, diags);
        }
        TopLevelItem::Class(class) => {
            for m in class.methods.iter() {
                scan_block(&m.body, check, in_async || m.is_async, diags);
            }
            for inner_impl in class.inner_impls.iter() {
                for inner in inner_impl.items.iter() {
                    if let ImplItem::Method(m) = inner {
                        scan_block(&m.body, check, in_async || m.is_async, diags);
                    }
                }
            }
        }
        TopLevelItem::Impl(impl_block) => {
            for inner in impl_block.items.iter() {
                if let ImplItem::Method(m) = inner {
                    scan_block(&m.body, check, in_async || m.is_async, diags);
                }
            }
        }
        TopLevelItem::Module(module) => {
            for nested in module.items.iter() {
                collect_in_item(nested, check, in_async, diags);
            }
        }
        _ => {}
    }
}

fn scan_block(
    body: &Block,
    check: AsyncCheck,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    let mut visitor = AsyncScopeVisitor {
        check,
        in_async,
        in_loop: false,
        diags,
    };
    visitor.visit_block(body);
}
