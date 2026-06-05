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
//! `Visit` covers expr/stmt/block/pattern/type but NOT `TopLevelItem`; each
//! public collector keeps the same small top-level item walk it had before
//! (flipping `in_async` at fn/method boundaries) and hands each body to a
//! `Visit` impl.

use crate::parser::ast::*;
use crate::parser::visit::{walk_expr, Visit};

// ─── E1115 — `.await` inside a loop/while/for body ──────────────────
//
// Spec: docs/specs/syntax/async_lowering_loop_await.spec.md.
// Error doc: docs/errors/E1115.md.

/// E1115: `.await` inside a `loop`/`while`/`for`/`while let` body that the v1
/// lowering does not handle (it does not build per-iteration state machines).
/// `in_async`/`in_loop` are carried as visitor state; the loop forms set
/// `in_loop` for the duration of their body walk.
struct AwaitInLoopScan<'a> {
    in_async: bool,
    in_loop: bool,
    diags: &'a mut Vec<crate::diagnostics::Diagnostic>,
}

impl Visit for AwaitInLoopScan<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Await(_) => {
                if self.in_async && self.in_loop {
                    self.diags
                        .push(crate::diagnostics::Diagnostic::error_with_code(
                            "`.await` inside a `loop` / `while` / `for` body is not yet \
                             supported; v1 lowering does not build per-iteration state \
                             machines. Hand-poll the future with `match fut.poll(&var cx)` \
                             inside the loop instead, or restructure to chained `.await`s \
                             outside the loop.",
                            e.span.clone(),
                            "E1115",
                        ));
                    // Continue walking — multiple awaits in the same loop each
                    // get their own diagnostic so the user sees them all in
                    // one pass.
                }
                walk_expr(self, e);
            }
            // Entering a loop body sets `in_loop` for everything nested inside.
            // `While`/`WhileLet` evaluate their condition/value once per
            // iteration, so an `.await` there is still a per-iteration suspend
            // point — keep `in_loop` set for them too.
            ExprKind::Loop(LoopExpr { body, .. }) => {
                let prev = self.in_loop;
                self.in_loop = true;
                self.visit_block(body);
                self.in_loop = prev;
            }
            ExprKind::While(WhileExpr {
                condition, body, ..
            }) => {
                let prev = self.in_loop;
                self.in_loop = true;
                self.visit_expr(condition);
                self.visit_block(body);
                self.in_loop = prev;
            }
            ExprKind::WhileLet(WhileLetExpr { value, body, .. }) => {
                let prev = self.in_loop;
                self.in_loop = true;
                self.visit_expr(value);
                self.visit_block(body);
                self.in_loop = prev;
            }
            // The iterable is evaluated ONCE outside the loop, so `.await`
            // there is NOT in_loop — it's a normal pre-loop suspend the
            // segmenter can handle. Only the body runs per-iteration.
            ExprKind::For(ForExpr { iterable, body, .. }) => {
                self.visit_expr(iterable);
                let prev = self.in_loop;
                self.in_loop = true;
                self.visit_block(body);
                self.in_loop = prev;
            }
            // A nested closure has its own loop / async scope. Don't propagate
            // the outer `in_loop` / `in_async` flags into it — its `.await`s
            // are scoped to the closure body and handled as part of the
            // lowering for THAT body. (Matches the old collector, which did
            // not recurse into closures at all.)
            ExprKind::Closure(_) => {}
            // Everything else: recurse, carrying the current flags. The
            // exhaustive `walk_expr` reaches EnumVariant/SafeNav/IfLet/Yield/
            // MacroCall/... that the old hand-rolled walker dropped.
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
    let mut diags = Vec::new();
    for item in &program.items {
        collect_e1115_in_item(item, /*in_async=*/ false, &mut diags);
    }
    diags
}

fn collect_e1115_in_item(
    item: &TopLevelItem,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    match item {
        TopLevelItem::Function(func) => {
            let scope_async = in_async || func.is_async;
            // Crossing a fn boundary resets the loop context — a closure-free
            // nested async fn doesn't inherit loops from its lexical
            // surroundings (and at this point there are no nested async fn
            // defs anyway, only methods).
            //
            // Skip the E1115 check entirely when the body matches a shape the
            // async lowering handles correctly. Spec:
            // docs/specs/syntax/async_lowering_loop_await.spec.md.
            //
            // The source of truth is now `segment_cfg`: if it accepts the body
            // (no-await / linear-N / single- or multi-await loop), the lowering
            // produces a valid state machine and there is no unsupported
            // await-in-loop to diagnose. For the no-await / linear shapes the
            // subsequent `scan_block_e1115` would find nothing anyway (no await
            // inside a loop), so the skip is behaviour-preserving; for the loop
            // shapes it suppresses the E1115 the accepted loop would otherwise
            // trip. Bodies `segment_cfg` REJECTS (e.g. await in the loop cond)
            // fall through to the scan and still get E1115.
            if scope_async && super::cfg::segment_cfg(&func.body).is_some() {
                return;
            }
            scan_block_e1115(&func.body, scope_async, diags);
        }
        TopLevelItem::Class(class) => {
            for m in class.methods.iter() {
                let scope_async = in_async || m.is_async;
                scan_block_e1115(&m.body, scope_async, diags);
            }
            for inner_impl in class.inner_impls.iter() {
                for inner in inner_impl.items.iter() {
                    if let ImplItem::Method(m) = inner {
                        let scope_async = in_async || m.is_async;
                        scan_block_e1115(&m.body, scope_async, diags);
                    }
                }
            }
        }
        TopLevelItem::Impl(impl_block) => {
            for inner in impl_block.items.iter() {
                if let ImplItem::Method(m) = inner {
                    let scope_async = in_async || m.is_async;
                    scan_block_e1115(&m.body, scope_async, diags);
                }
            }
        }
        TopLevelItem::Module(module) => {
            for nested in module.items.iter() {
                collect_e1115_in_item(nested, in_async, diags);
            }
        }
        _ => {}
    }
}

fn scan_block_e1115(body: &Block, in_async: bool, diags: &mut Vec<crate::diagnostics::Diagnostic>) {
    let mut scan = AwaitInLoopScan {
        in_async,
        in_loop: false,
        diags,
    };
    scan.visit_block(body);
}

// ─── E1112 — `block_on` inside an async function/closure ────────────
//
// Spec: docs/specs/stdlib/executor.spec.md B6.
// Error doc: docs/errors/E1112.md.
//
// This MUST run BEFORE `lower_async_defs` rewrites async fn bodies into
// synthesised state-machine classes — once the async-fn rewrite fires, the
// original `block_on(...)` call ends up inside the generated `poll` method
// (which is itself NOT marked async), so the resolver's async_scope_depth
// check would not find it.

/// E1112: a `block_on(...)` call inside an async scope. Descending into a
/// nested closure may CHANGE the async scope (sync closure inside async fn →
/// inner scope is sync; async closure inside sync fn → inner scope is async),
/// so the `Closure` arm overrides `in_async` from the closure's own marker.
struct BlockOnScan<'a> {
    in_async: bool,
    diags: &'a mut Vec<crate::diagnostics::Diagnostic>,
}

impl Visit for BlockOnScan<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        // Flag this node if it is `block_on(_)` and we're in an async scope.
        // We don't gate on arg count — even malformed `block_on()` calls
        // should produce E1112 rather than a confusing arity error first.
        if self.in_async {
            if let ExprKind::Call { callee, .. } = &e.kind {
                if let ExprKind::Identifier(name) = &callee.kind {
                    if name == "block_on" {
                        self.diags
                            .push(crate::diagnostics::Diagnostic::error_with_code(
                                "`block_on` cannot be called inside an `async` function or closure — use `.await` to await a future in an async context",
                                e.span.clone(),
                                "E1112",
                            ));
                    }
                }
            }
        }

        match &e.kind {
            ExprKind::Closure(c) => {
                // The closure's async-ness OVERRIDES the surrounding scope.
                let prev = self.in_async;
                self.in_async = c.is_async;
                match &c.body {
                    ClosureBody::Expr(ex) => self.visit_expr(ex),
                    ClosureBody::Block(b) => self.visit_block(b),
                }
                self.in_async = prev;
            }
            _ => walk_expr(self, e),
        }
    }
}

/// Walk `program` BEFORE `lower_async_defs` runs and collect a diagnostic for
/// every `block_on(...)` call found inside an async function or async closure.
pub fn collect_block_on_in_async_diagnostics(
    program: &Program,
) -> Vec<crate::diagnostics::Diagnostic> {
    let mut diags = Vec::new();
    for item in &program.items {
        collect_e1112_in_item(item, /*in_async=*/ false, &mut diags);
    }
    diags
}

fn collect_e1112_in_item(
    item: &TopLevelItem,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    match item {
        TopLevelItem::Function(func) => {
            let scope_async = in_async || func.is_async;
            scan_block_e1112(&func.body, scope_async, diags);
        }
        TopLevelItem::Class(class) => {
            for m in class.methods.iter() {
                let scope_async = in_async || m.is_async;
                scan_block_e1112(&m.body, scope_async, diags);
            }
            for inner_impl in class.inner_impls.iter() {
                for inner in inner_impl.items.iter() {
                    if let ImplItem::Method(m) = inner {
                        let scope_async = in_async || m.is_async;
                        scan_block_e1112(&m.body, scope_async, diags);
                    }
                }
            }
        }
        TopLevelItem::Impl(impl_block) => {
            for inner in impl_block.items.iter() {
                if let ImplItem::Method(m) = inner {
                    let scope_async = in_async || m.is_async;
                    scan_block_e1112(&m.body, scope_async, diags);
                }
            }
        }
        TopLevelItem::Module(module) => {
            for nested in module.items.iter() {
                collect_e1112_in_item(nested, in_async, diags);
            }
        }
        _ => {}
    }
}

fn scan_block_e1112(body: &Block, in_async: bool, diags: &mut Vec<crate::diagnostics::Diagnostic>) {
    let mut scan = BlockOnScan { in_async, diags };
    scan.visit_block(body);
}

// ─── E1116 — `Task.spawn` outside an executor context ───────────────
//
// Spec: docs/specs/stdlib/task_spawn.spec.md §B7.
// Error doc: docs/errors/E1116.md.
//
// `Task.spawn(fut)` only makes sense inside a Ruxen executor — i.e. inside an
// `async def` or `async { ... }` closure. `Task.spawn_raw` is the lower-level
// escape hatch used by runtimes that establish an executor by driving
// `block_on` themselves (Rondo's accept loop is the canonical case).
//
// Polarity is inverted vs. E1112: flag the call when in_async == false. The
// scan reuses the same scope-tracking shape (toggling in_async on async
// function/closure bodies) so a Task.spawn inside a sync closure inside an
// async fn correctly fires.
//
// Surface match: this only rejects `Task.spawn(...)` (MethodCall with receiver
// `Task`). `Task.spawn_raw(...)` remains available in sync runtime code.

/// E1116: a `Task.spawn(...)` call outside an async scope. The parser folds
/// `Task.spawn(x)` into `MethodCall { object: Identifier("Task"), method:
/// "spawn", args }`. The `Closure` arm overrides `in_async` (matches E1112).
struct TaskSpawnScan<'a> {
    in_async: bool,
    diags: &'a mut Vec<crate::diagnostics::Diagnostic>,
}

impl Visit for TaskSpawnScan<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        // Flag this node if it is `Task.spawn(...)` and we're NOT in an async
        // scope.
        if !self.in_async {
            if let ExprKind::MethodCall { object, method, .. } = &e.kind {
                if let ExprKind::Identifier(name) = &object.kind {
                    if name == "Task" && method == "spawn" {
                        self.diags
                            .push(crate::diagnostics::Diagnostic::error_with_code(
                                "`Task.spawn` can only be called inside an `async` function or closure — there is no executor to enqueue into in sync context",
                                e.span.clone(),
                                "E1116",
                            ));
                    }
                }
            }
        }

        match &e.kind {
            ExprKind::Closure(c) => {
                let prev = self.in_async;
                self.in_async = c.is_async;
                match &c.body {
                    ClosureBody::Expr(ex) => self.visit_expr(ex),
                    ClosureBody::Block(b) => self.visit_block(b),
                }
                self.in_async = prev;
            }
            _ => walk_expr(self, e),
        }
    }
}

/// Walk `program` and collect a diagnostic for every `Task.spawn(...)` call
/// found OUTSIDE an async function or async closure.
pub fn collect_task_spawn_outside_async_diagnostics(
    program: &Program,
) -> Vec<crate::diagnostics::Diagnostic> {
    let mut diags = Vec::new();
    for item in &program.items {
        collect_e1116_in_item(item, /*in_async=*/ false, &mut diags);
    }
    diags
}

fn collect_e1116_in_item(
    item: &TopLevelItem,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    match item {
        TopLevelItem::Function(func) => {
            let scope_async = in_async || func.is_async;
            scan_block_e1116(&func.body, scope_async, diags);
        }
        TopLevelItem::Class(class) => {
            for m in class.methods.iter() {
                let scope_async = in_async || m.is_async;
                scan_block_e1116(&m.body, scope_async, diags);
            }
            for inner_impl in class.inner_impls.iter() {
                for inner in inner_impl.items.iter() {
                    if let ImplItem::Method(m) = inner {
                        let scope_async = in_async || m.is_async;
                        scan_block_e1116(&m.body, scope_async, diags);
                    }
                }
            }
        }
        TopLevelItem::Impl(impl_block) => {
            for inner in impl_block.items.iter() {
                if let ImplItem::Method(m) = inner {
                    let scope_async = in_async || m.is_async;
                    scan_block_e1116(&m.body, scope_async, diags);
                }
            }
        }
        TopLevelItem::Module(module) => {
            for nested in module.items.iter() {
                collect_e1116_in_item(nested, in_async, diags);
            }
        }
        _ => {}
    }
}

fn scan_block_e1116(body: &Block, in_async: bool, diags: &mut Vec<crate::diagnostics::Diagnostic>) {
    let mut scan = TaskSpawnScan { in_async, diags };
    scan.visit_block(body);
}
