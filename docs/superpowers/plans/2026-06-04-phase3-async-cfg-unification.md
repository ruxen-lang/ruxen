# Phase 3 — async_lowering CFG unification + `Visit`-based diagnostic collectors

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`)
> syntax for tracking. This is Phase 3 of `2026-06-04-thermonuke-master.md`. **It depends on Phase 1**
> (`parser::visit::{Visit, VisitMut, walk_expr, walk_block, walk_stmt}` and `Ty::map_inner`); those APIs
> are referenced as already-existing here, never redefined.

**Goal:** Collapse `compiler/ruxen_core/src/async_lowering/mod.rs` (5,812 lines) along two axes.
(a) Migrate the three diagnostic collector triplets (`collect_e1112/e1115/e1116_in_{item,block,expr}`)
onto Phase 1's `Visit` trait, deleting ~700 lines of near-identical hand-rolled `*_in_block`/`*_in_expr`
recursion that each carry their own `_ => {}` catch-all. (b) Replace the FIVE hand-specialized lowering
paths (`no-await` / `linear-N-await` / `while-single` / `while-multi` / `multi-phase-loop`) and their
fragile `recognize_*Shape` syntactic allowlist with ONE await-delimited segment/CFG state-machine
lowering (basic blocks split at `.await`; an edge list; loops = back-edges). Then split the monster file
into `async_lowering/{mod,cfg,lower,diagnostics}.rs`.

**Architecture.** The five lowering paths are already ONE algorithm wearing five costumes: every one of
`build_multi_state_poll_body` (2585), `build_loop_state_machine_poll_body` (2887), and
`build_multi_phase_loop_poll_body` (3397) emits the *same* `self.__state`-indexed `if/elsif/else` poll
skeleton — match a sub-future's `poll(cx)`, on `Ready(v)` store a field + bump `__state`/`__phase`, on
`Pending` return `Poll.Pending`; the loop variants merely add a back-edge that resets the discriminant
to the loop head instead of advancing to a terminal `Poll.Ready(tail)`. We make that shared structure
explicit: an async fn body becomes a `Cfg` of await-delimited `Segment`s connected by `Edge`s; loops
are back-edges to a segment that re-runs the loop condition. One `build_poll_body(&Cfg)` emits the
`__state`-indexed skeleton for *any* edge topology, subsuming all four builders and all three
`recognize_*`.

**This phase CHANGES GENERATED CODE for async fns** (the poll body for a given fn may differ
structurally from the old per-shape output). It is therefore **behaviour-preserving-with-golden-tests**,
not refactor-only: the contract is *runtime-equivalent poll behaviour*, proven by the async integration
suite (which compiles-and-runs) plus per-shape golden-equivalence tests. The new CFG lowering is built
**behind a feature gate / parallel path** and proven equivalent on every shape BEFORE the old paths are
deleted (Task 5). That gating is the single most important risk control in this plan.

**Tech Stack:** Rust 1.91, `cargo test -p ruxen_core`. No new dependencies.

> **Per-task direction-check (maintainer-mandated):** after the commit step of EVERY task below, run the
> `thermonuke` skill scoped to that task's diff (invoke with arg `git diff HEAD~1..HEAD`) and confirm
> (a) net lines moved in the intended direction, (b) **no new `_ =>` catch-all** in any traversal/lowering
> match, (c) no new god-function/special-case in a shared path, (d) the task's structural goal was met —
> for this phase specifically, that **a specialized lowering path / recognizer / hand-rolled walker was
> DELETED, not merely added alongside** (Tasks 1, 5, 6) or that the new shared path is gated and pinned
> (Tasks 2–4). If it flags drift, STOP and surface it. The full multi-agent sweep runs in Task 7. Each
> task's checkbox list ends with a `- [ ] Direction-check (/thermonuke on this task's diff)` step.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `compiler/ruxen_core/src/async_lowering/mod.rs` | pass entry (`lower_async_defs[_with_bootstrap]`), the await-table collection helpers, `mod` decls re-exporting the public API | Modify (shrink from 5,812 to the orchestrator + table-collection only) |
| `compiler/ruxen_core/src/async_lowering/diagnostics.rs` | the three public collectors (`collect_await_in_loop_diagnostics`, `collect_block_on_in_async_diagnostics`, `collect_task_spawn_outside_async_diagnostics`) as thin `Visit` impls | **Create** (Task 1 moves the e1112/e1115/e1116 logic here) |
| `compiler/ruxen_core/src/async_lowering/cfg.rs` | `Segment`, `Edge`, `Cfg`, and `segment_cfg(&Block) -> Option<Cfg>` (subsumes the 3 `recognize_*` + `segment_body`) | **Create** (Tasks 2–3) |
| `compiler/ruxen_core/src/async_lowering/lower.rs` | `build_poll_body(&Cfg, ...) -> Block` (subsumes the 4 builders) + `lower_async_fn(func) -> Option<(FuncDef, ClassDef)>` (the single replacement entry) + the shared AST-construction helpers (`self_field`, `rewrite_arg_refs_in_*`, `mangle_future_class_name`, the `Poll.Ready`/`Poll.Pending` constructors) | **Create** (Tasks 4–6) |

**Module placement rationale:** `diagnostics.rs` has no dependency on lowering and is independently
testable — it moves first (Task 1) so the rest of the split lands on a smaller file. `cfg.rs` is pure
AST→data-structure analysis (no codegen, no `Span`-heavy builders) so it can be unit-tested in
isolation. `lower.rs` consumes `cfg::Cfg` and owns every `Expr`/`Block`-constructing helper. `mod.rs`
keeps the public surface (`pub use diagnostics::*; pub use lower::lower_async_defs*;`) so external
callers — `typeck/mod.rs:66,73,80,165,168,170` and `resolve/exprs.rs:434` — compile unchanged.

---

## Task 1: Migrate the three diagnostic collectors onto `Visit`

**Why first:** lowest risk, independently testable, deletes the most obviously-duplicated code (three
`*_in_item`/`*_in_block`/`*_in_expr` triplets, ~700 lines, each with its own `_ => {}`). It also closes
the same variant-drift bug class Phase 1 closed for the await-scan: the hand-rolled `collect_e1115_in_expr`
(4540) matches `BinaryOp`/`UnaryOp`/`Borrow`/`Try`/`FieldAccess`/`MethodCall`/`Call`/`ClosureCall`/
`Assign`/`If`/… with a trailing catch-all, so an `.await` inside e.g. `EnumVariant`/`SafeNav`/`MapLiteral`
in a loop is silently un-diagnosed — exactly the forms Phase 1's `walk_expr` covers.

**Files:**
- Create: `compiler/ruxen_core/src/async_lowering/diagnostics.rs`
- Modify: `compiler/ruxen_core/src/async_lowering/mod.rs` (delete lines 4455–5242 — the three
  `pub fn collect_*_diagnostics` entries at 4455/4735/4979 and their `_in_item/_in_block/_in_expr`
  helpers — and add `mod diagnostics; pub use diagnostics::*;`)
- Test: `compiler/ruxen_core/tests/async_negative.rs` (the existing E1112/E1115/E1116 pin) + an inline
  characterization test asserting parity on a broad expression corpus.

The three collectors carry **context state** the bare `Visit` trait doesn't: E1115 needs
`in_async` + `in_loop`; E1112/E1116 need `in_async`. That state lives as fields on the visitor struct,
flipped on the way down. The `in_loop` flag is set by overriding `visit_expr` for the loop forms
(`While`/`WhileLet`/`For`/`Loop`) — set `in_loop = true`, recurse via `walk_expr`, restore. The
`in_async` flag is set in `visit_item`-equivalent code (the collectors walk `TopLevelItem`, which `Visit`
does not cover — keep the small top-level `for item in &program.items` match in each public fn, since
`Visit` is an *expr/stmt/block/pattern/type* traversal, and route into `visit_block` from there).

- [ ] **Step 1: Write the failing (bug-exposing) characterization test**

Add to `compiler/ruxen_core/tests/async_negative.rs` (mirror the existing `typeck_errors`/`rx` helpers
at lines 35/50):

```rust
#[test]
fn await_in_loop_inside_enum_variant_arg_is_diagnosed_e1115() {
    // `while cond do let _ = Some(g().await) end` — the await is nested in an
    // EnumVariant arg, a form the hand-rolled collect_e1115_in_expr never matched
    // (it had a trailing `_ => {}`). Must still fire E1115. Bug-by-construction.
    let src = r#"
        async def g() -> Int
            1
        end
        async def run() -> Unit
            let mut i = 0
            while i < 3
                let _x = Some(g().await)
                i = i + 1
            end
        end
    "#;
    let diags = typeck_errors(src);
    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E1115")),
        "await nested in EnumVariant arg inside a loop must raise E1115; got {:?}",
        diags.iter().map(|d| d.code.clone()).collect::<Vec<_>>()
    );
}
```

> Implementer: confirm the `Diagnostic` code accessor (`.code`) field name against
> `crate::diagnostics::Diagnostic` — `async_lowering/mod.rs:4548` builds these via
> `Diagnostic::error_with_code(msg, span, "E1115")`, so the code field exists; adapt the accessor to
> whatever `async_negative.rs`'s existing assertions use (read lines 66–203 for the established pattern).

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ruxen_core --test async_negative await_in_loop_inside_enum_variant 2>&1 | tee tmp/test-cache/phase3-task1-red.log`
Expected: FAIL — no E1115 emitted (the await is invisible to the old walker inside `EnumVariant`).

- [ ] **Step 3: Create `diagnostics.rs` with three `Visit` impls**

Create `compiler/ruxen_core/src/async_lowering/diagnostics.rs`. Each public fn keeps its exact current
name and signature (callers in `typeck/mod.rs` are unchanged) and its top-level-item walk; the
per-expr recursion is replaced by a `Visit` impl. Concrete shape for E1115 (the only stateful-on-loop
one); E1112/E1116 are the same pattern with only `in_async`:

```rust
//! Async surface diagnostics (E1112 block_on-in-async, E1115 await-in-loop,
//! E1116 task-spawn-outside-async), expressed as `parser::visit::Visit` impls.
//! The ONE exhaustive `walk_expr` (Phase 1) replaces three hand-rolled
//! `*_in_expr` recursions that each had a `_ => {}` catch-all and had drifted
//! (e.g. await inside EnumVariant/SafeNav/MapLiteral was invisible to E1115).

use crate::parser::ast::*;
use crate::parser::visit::{walk_expr, Visit};

/// E1115: `.await` inside a `loop`/`while`/`for` body that the CFG lowering
/// does not handle. `in_async`/`in_loop` are carried as visitor state; the loop
/// forms set `in_loop` for the duration of their body walk.
struct AwaitInLoopScan<'a> {
    in_async: bool,
    in_loop: bool,
    diags: &'a mut Vec<crate::diagnostics::Diagnostic>,
}

impl Visit for AwaitInLoopScan<'_> {
    fn visit_expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Await(_) if self.in_async && self.in_loop => {
                self.diags.push(crate::diagnostics::Diagnostic::error_with_code(
                    // PORT VERBATIM from async_lowering/mod.rs:4548-4556.
                    "`.await` inside a `loop` / `while` / `for` body is not yet \
                     supported; v1 lowering does not build per-iteration state \
                     machines. Hand-poll the future with `match fut.poll(&var cx)` \
                     inside the loop instead, or restructure to chained `.await`s \
                     outside the loop.",
                    e.span.clone(),
                    "E1115",
                ));
                walk_expr(self, e); // keep walking — multiple awaits each diagnose
            }
            ExprKind::While(_) | ExprKind::WhileLet(_) | ExprKind::For(_) | ExprKind::Loop(_) => {
                let prev = self.in_loop;
                self.in_loop = true;
                walk_expr(self, e);
                self.in_loop = prev;
            }
            // Closures are a separate async scope — do NOT inherit in_loop/in_async.
            // (Preserve whatever the old collector did at fn/closure boundaries;
            //  see mod.rs:4473 comment "Crossing a fn boundary resets the loop
            //  context". Async closures aren't lowered yet, so opacity matches.)
            ExprKind::Closure(_) => {}
            _ => walk_expr(self, e),
        }
    }
}

pub fn collect_await_in_loop_diagnostics(program: &Program) -> Vec<crate::diagnostics::Diagnostic> {
    let mut diags = Vec::new();
    for item in &program.items {
        collect_e1115_in_item(item, /*in_async=*/ false, &mut diags);
    }
    diags
}

// `Visit` covers expr/stmt/block/pattern/type but NOT TopLevelItem; keep the
// small top-level walk that flips `in_async` and skips the recognized loop
// shapes, then hand each body to the Visit impl.
fn collect_e1115_in_item(
    item: &TopLevelItem,
    in_async: bool,
    diags: &mut Vec<crate::diagnostics::Diagnostic>,
) {
    // PORT the item-level structure from mod.rs:4465-4520 EXACTLY, including:
    //  * Function: scope_async = in_async || func.is_async; THEN the
    //    `if scope_async && cfg-handled-shape { return; }` guard. In the OLD
    //    code this guard was `recognize_while_single_await(..).is_some() ||
    //    recognize_while_multi_await(..).is_some()` (mod.rs:4483-4488). In THIS
    //    task it must stay byte-for-byte equivalent, so keep calling the (not-
    //    yet-deleted) recognizers via their `crate::async_lowering::` path.
    //    Task 5 swaps this single call site to `segment_cfg(&func.body).is_some()`
    //    once the CFG path is the source of truth — NOT here.
    //  * Class/Impl/Module recursion exactly as mod.rs:4491-4517.
    let _ = (item, in_async, diags);
    todo!("port mod.rs:4465-4520 item walk; body recursion via AwaitInLoopScan");
}
```

Then the analogous `collect_block_on_in_async_diagnostics` (E1112, port from mod.rs:4735–4977 — only
`in_async` state, the interesting expr is a `Call`/`ClosureCall` to identifier `block_on`) and
`collect_task_spawn_outside_async_diagnostics` (E1116, port from mod.rs:4979–5242 — only `in_async`,
interesting expr is the `task.spawn`/`spawn` call form the old `collect_e1116_in_expr` matched). For
both, the `_in_block`/`_in_expr` recursion bodies are deleted and replaced by a `Visit` impl whose
`visit_expr` handles the one interesting node and delegates everything else to `walk_expr`.

> **Implementer obligation (explicit sub-step, not a placeholder):** before green, open mod.rs
> 4735–4977 (E1112) and 4979–5242 (E1116), read which `ExprKind` arm each treats as "the interesting
> call" and the exact diagnostic message/code/span each emits, and reproduce those *verbatim* in the
> two analogous `Visit` impls. The three `todo!`s exist only because the precise E1112/E1116 message
> strings and the `block_on`/`spawn` recognition predicates were not transcribed during planning;
> guessing them would be invented precision. Everything structural (the `Visit` shape, the in_async
> threading, the top-level item walk) is fully specified above.

- [ ] **Step 3b: Wire the module + delete the old collectors**

In `async_lowering/mod.rs`: add near the top `mod diagnostics;` and `pub use diagnostics::*;`, then
DELETE lines 4455–5242 (the three public fns + their nine `_in_*` helpers). The `pub use` keeps the
external call sites (`typeck/mod.rs`, `resolve/exprs.rs`) compiling without edits.

- [ ] **Step 4: Run tests to verify green**

Run: `cargo test -p ruxen_core --test async_negative 2>&1 | tee tmp/test-cache/phase3-task1-green.log`
Expected: PASS including the new EnumVariant test and every pre-existing E1112/E1115/E1116 case (66–203).
Then the surface pin: `cargo test -p ruxen_core --test async_surface 2>&1 | tee tmp/test-cache/phase3-task1-surface.log`
Expected: PASS (proves the public collector API is intact for downstream typeck).

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/async_lowering/mod.rs compiler/ruxen_core/src/async_lowering/diagnostics.rs compiler/ruxen_core/tests/async_negative.rs
git commit -m "refactor(async): collectors e1112/e1115/e1116 over Visit (-~700 LOC)

Replace three hand-rolled *_in_item/_in_block/_in_expr triplets (each with its
own _ => {} catch-all) with thin Visit impls over Phase 1's exhaustive
walk_expr. Closes a variant-drift gap: .await nested in EnumVariant/SafeNav/
MapLiteral inside a loop is now diagnosed (E1115). Public collector signatures
unchanged; moved to async_lowering/diagnostics.rs.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — confirm 3 collector triplets DELETED, no
  new `_ =>` in `diagnostics.rs`, net negative LOC, public API preserved via `pub use`.

---

## Task 2: Introduce the `Cfg` data model (`Segment` + `Edge`), no lowering yet

**Why now:** define the target data structure and prove it can *represent* every shape, before writing
the analysis (Task 3) or the codegen (Task 4). Pure data + a couple of constructors; no behaviour change.

**Files:**
- Create: `compiler/ruxen_core/src/async_lowering/cfg.rs`
- Modify: `async_lowering/mod.rs` (add `mod cfg;`)
- Test: inline `#[cfg(test)] mod tests` in `cfg.rs`

The model, derived directly from what the four builders already emit (the `__state`/`__phase`-indexed
poll skeleton):

```rust
//! Await-delimited control-flow graph for an async fn body. Each `Segment` is a
//! maximal run of straight-line statements ending at (at most) one `.await`
//! suspension point. `Edge`s connect segments; a loop is a back-edge. ONE
//! `build_poll_body` (lower.rs) emits the `self.__state`-indexed poll skeleton
//! for ANY topology, subsuming the five hand-specialized paths.

use crate::lexer::token::Span;
use crate::parser::ast::{Expr, LetBinding, Statement};

/// A basic block in the async CFG: straight-line work, then an optional suspend.
pub struct Segment {
    /// 0-based index == the `self.__state` discriminant value for this segment.
    pub id: usize,
    /// Statements that run on entry to this segment, BEFORE its suspend. None
    /// may contain a nested `.await` (those split into earlier segments).
    pub stmts: Vec<Statement>,
    /// The suspension that ends this segment, if any. `None` only for the
    /// terminal segment (whose `stmts` end in the tail value).
    pub suspend: Option<Suspend>,
}

/// A `let <binding> = <awaitee>.await` suspension point.
pub struct Suspend {
    /// Binding name — becomes a `self.<binding>` field (every await result
    /// survives the next poll, matching current 2B behaviour, mod.rs:1826).
    pub binding: String,
    /// The awaitee expression (`g(args)` / `Class.method(args)`), pre-rewrite.
    /// `lower.rs` builds the sub-future ctor + the `(&var self.__sub_i).poll(cx)`
    /// match from this, exactly as describe_await/AwaitSub does today (mod.rs:1783).
    pub awaitee: Expr,
}

/// Where control goes after a segment's suspend resolves `Ready`.
pub enum Edge {
    /// Advance to segment `to` (the straight-line / linear-N case).
    Next { from: usize, to: usize },
    /// Conditional back-edge: re-evaluate `cond`; if true go to `to` (loop head),
    /// else fall through to `else_to`. Models `while cond` loops as a back-edge,
    /// replacing the recognize_while_single/multi + multi-phase-loop builders.
    Loop { from: usize, cond: Expr, to: usize, else_to: usize },
}

pub struct Cfg {
    pub segments: Vec<Segment>,
    pub edges: Vec<Edge>,
    /// Statements after the loop / after the last suspend that produce the fn's
    /// return value (the terminal `Poll.Ready(<tail>)`).
    pub tail: Vec<Statement>,
    pub span: Span,
}
```

- [ ] **Step 1: Write the failing test (construct + assert shape for each of the 5 cases)**

End `cfg.rs` with a test that hand-builds a `Cfg` for the linear-2-await shape and one for the
while-single shape and asserts the invariants the lowering will rely on (segment ids are dense 0..N,
exactly one terminal segment with `suspend: None`, every `Edge::Loop.to` points at a real segment id):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfg_invariants_hold_for_handbuilt_linear_two_await() {
        // Hand-build the shape `let a = f().await; let b = g().await; a + b`.
        // (Constructors only — no analysis yet; that's Task 3.)
        let cfg = /* build a 3-segment, 2-edge Cfg literal */;
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.segments.last().unwrap().suspend.is_none(), true);
        assert!(cfg.segments.iter().enumerate().all(|(i, s)| s.id == i));
    }

    #[test]
    fn cfg_validate_rejects_dangling_edge() {
        let mut cfg = /* the same Cfg */;
        cfg.edges.push(Edge::Next { from: 0, to: 99 });
        assert!(cfg.validate().is_err());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ruxen_core --lib async_lowering::cfg 2>&1 | tee tmp/test-cache/phase3-task2-red.log`
Expected: FAIL to compile — `Cfg`/`Segment`/`Edge`/`validate` not found.

- [ ] **Step 3: Write the data structures + `Cfg::validate`**

Add the structs above plus a `validate` that checks: dense segment ids, exactly one `suspend: None`
terminal, every edge endpoint is an existing segment id, no `Next` *out of* the terminal. `validate` is
the cheap invariant the lowering depends on (it lets `build_poll_body` use `unreachable!` on
ill-formed graphs with a load-bearing comment instead of a silent `_ =>`).

- [ ] **Step 4: Run to verify green**

Run: `cargo test -p ruxen_core --lib async_lowering::cfg 2>&1 | tee tmp/test-cache/phase3-task2-green.log`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/async_lowering/cfg.rs compiler/ruxen_core/src/async_lowering/mod.rs
git commit -m "feat(async): add await-delimited Cfg/Segment/Edge model

Pure data model + validate() for the unified async lowering. A segment is a
straight-line run ending at one .await; edges connect them; loops are
back-edges. No analysis or codegen yet — Tasks 3-4 build segment_cfg() and
build_poll_body() against this.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — additive new file; confirm no `_ =>` in
  `validate`, and that this is genuinely the substrate for deletions in Tasks 3–6 (not a parallel 6th path).

---

## Task 3: `segment_cfg(&Block) -> Option<Cfg>` — one analysis subsuming the 3 `recognize_*`

**Files:**
- Modify: `compiler/ruxen_core/src/async_lowering/cfg.rs` (add `segment_cfg`)
- Test: inline `#[cfg(test)] mod tests` in `cfg.rs` (extend Task 2's module)

`segment_cfg` is the union of `segment_body` (2149), `recognize_while_single_await` (2023), and
`recognize_while_multi_await` (1882). It walks `body.statements` once and produces a `Cfg`:

- Straight-line `let x = e.await` / non-await stmts → split into `Segment`s at each await
  (port the phase-tracking state machine from `segment_body`, mod.rs:2160–2202: `seen_first_await`,
  `in_tail`; the same "await-let pattern must be a plain `Identifier`" guard at 2189).
- A single top-level `Statement::Expression(While)` whose body contains `.await` → its body is segmented
  the same way, and a `Edge::Loop { cond, to: <loop-head segment>, else_to: <post-loop segment> }` is
  added (port the loop-detection + `expr_contains_await(&loop_cond)` rejection from
  `recognize_while_single_await` 2023–2067 and the N-await phase splitting from
  `recognize_while_multi_await` 1926–1969 — these collapse into "segment the loop body, wire a
  back-edge"). The single-vs-multi distinction (which forced two recognizers + two builders) **disappears**:
  a loop body with 1 await is just a loop body with 1 segment-suspend; N awaits is N. No N==1/N>=2 split.
- Anything outside the supported subset (await in loop condition, await nested in a non-let expr,
  non-identifier await-let pattern, more than one top-level awaiting loop) → `None`, preserving the exact
  rejection set the three recognizers had (so E1115 still fires for unsupported shapes, via Task 1's
  collector, which Task 5 re-points at `segment_cfg`).

- [ ] **Step 1: Write failing tests — one per old shape, asserting `segment_cfg` reproduces it**

Add to `cfg.rs` tests. Parse a snippet into a `Block` (use the crate's `Lexer`+`Parser`; mirror
`async_lowering/mod.rs`'s own inline `mod tests` parse helper at ~5803), then assert the `Cfg` shape:

```rust
#[test]
fn segment_cfg_no_await_is_single_terminal_segment() {
    let body = parse_fn_body("def f\n  1 + 2\nend");
    let cfg = segment_cfg(&body).expect("no-await body is a valid (degenerate) Cfg");
    assert_eq!(cfg.segments.len(), 1);
    assert!(cfg.segments[0].suspend.is_none());
    assert!(cfg.edges.is_empty());
}

#[test]
fn segment_cfg_linear_two_await_has_three_segments_two_next_edges() {
    let body = parse_fn_body("async def f\n  let a = g().await\n  let b = h().await\n  a + b\nend");
    let cfg = segment_cfg(&body).unwrap();
    assert_eq!(cfg.segments.len(), 3);            // seg0 .await g, seg1 .await h, seg2 tail
    assert!(matches!(cfg.edges.as_slice(), [Edge::Next{..}, Edge::Next{..}]));
}

#[test]
fn segment_cfg_while_single_await_has_back_edge() {
    let body = parse_fn_body(
        "async def f\n  while keep()\n    let x = step().await\n  end\n  0\nend");
    let cfg = segment_cfg(&body).unwrap();
    assert!(cfg.edges.iter().any(|e| matches!(e, Edge::Loop { .. })));
}

#[test]
fn segment_cfg_while_multi_await_has_n_segments_one_back_edge() {
    let body = parse_fn_body(
        "async def f\n  while keep()\n    let a = r().await\n    let b = w().await\n  end\n  0\nend");
    let cfg = segment_cfg(&body).unwrap();
    assert_eq!(cfg.edges.iter().filter(|e| matches!(e, Edge::Loop{..})).count(), 1);
    // two suspends inside the loop body
    assert_eq!(cfg.segments.iter().filter(|s| s.suspend.is_some()).count(), 2);
}

#[test]
fn segment_cfg_rejects_await_in_loop_condition() {
    let body = parse_fn_body("async def f\n  while c().await\n    let x = s().await\n  end\n  0\nend");
    assert!(segment_cfg(&body).is_none());   // same rejection as recognize_while_single_await:2065
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ruxen_core --lib async_lowering::cfg::tests::segment_cfg 2>&1 | tee tmp/test-cache/phase3-task3-red.log`
Expected: FAIL — `segment_cfg` not found.

- [ ] **Step 3: Implement `segment_cfg`**

Port the three recognizers into one walk. Reuse Phase 1's await-scan (`block_contains_await`/
`expr_contains_await`, themselves Visit-based after Phase 1 Task 4) for the rejection guards. Concrete
sub-steps with the exact source to fold in:
  1. Top-level loop detection + single-await-loop rejection set ← `recognize_while_single_await`
     mod.rs:2024–2067 (`while_idx`, the "any other stmt must not contain await" guard, the
     `expr_contains_await(&loop_cond)` reject).
  2. Loop-body segmentation into N suspends ← `recognize_while_multi_await` mod.rs:1926–1969
     (the `current_pre`/`phases`/`post_last_await_stmts` accumulation), generalized to N≥1 (drop the
     `phases.len() < 2` reject at 1967).
  3. Straight-line (no-loop) segmentation ← `segment_body` mod.rs:2160–2202 (the
     `seen_first_await`/`in_tail` phase machine and the identifier-pattern guard at 2189).
  4. Assemble `segments` (dense ids), `edges` (`Next` for the straight chain; one `Loop` back-edge for
     the loop), `tail`, and call `Cfg::validate()` before returning `Some`.

> Implementer: this is the largest analysis step but every line has a named source. Do NOT invent
> rejection rules — the supported subset must be EXACTLY the union of what the three old recognizers
> accepted, so that Task 5's golden-equivalence and the E1115 negative tests stay green.

- [ ] **Step 4: Run to verify green**

Run: `cargo test -p ruxen_core --lib async_lowering::cfg 2>&1 | tee tmp/test-cache/phase3-task3-green.log`
Expected: PASS (Task 2 + the 5 new shape tests).

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/async_lowering/cfg.rs
git commit -m "feat(async): segment_cfg subsumes the 3 recognize_*Shape walkers

One body walk produces a Cfg for no-await / linear-N / while-single /
while-multi shapes. The single-vs-multi-await distinction vanishes (N suspends
in a loop body, one back-edge). Rejection set is the exact union of the three
old recognizers so E1115 negatives are unchanged. Recognizers not yet deleted
(Task 5 deletes them once build_poll_body is proven equivalent).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — confirm `segment_cfg` is one function with
  no `_ =>` rejection fallthrough; note that the 3 recognizers are still present (deletion deferred to
  Task 5 by design — flag if thermonuke reads this as drift, it is intentional gating).

---

## Task 4: `build_poll_body(&Cfg, ...)` — one builder, behind a parallel path

**Files:**
- Create: `compiler/ruxen_core/src/async_lowering/lower.rs`
- Modify: `async_lowering/mod.rs` (add `mod lower;`)
- Test: inline tests in `lower.rs` + a golden-equivalence test (see Step 1).

`build_poll_body` emits the `self.__state`-indexed `if/elsif/else` poll skeleton for an arbitrary `Cfg`.
This is the *generalization* of the shared skeleton all four builders already emit; port the per-arm
construction verbatim:
  - Per segment with a `Suspend`: the `match (&var self.__sub_i).poll(cx) { Ready(v) -> self.<binding>
    = v; <edge-action>; Pending -> Poll.Pending }` arm ← `build_multi_state_poll_body` mod.rs:2635–2693
    (the `assign_local` / `bump_state` / `terminal_ready` / `ready_block` construction).
  - The `<edge-action>` is computed FROM THE EDGE, not hardcoded: `Edge::Next` → `self.__state = to`,
    `Poll.Pending` (re-poll) ← mod.rs:2669–2691; `Edge::Loop` → emit the cond re-check and set
    `self.__state` to `to` (loop head) or `else_to`, ← the back-edge logic in
    `build_loop_state_machine_poll_body` (2887+) and the `__phase` reset in
    `build_multi_phase_loop_poll_body` (3397+). The `__phase` field merges into `__state` (a loop is
    just an edge that can target an earlier segment id — no separate discriminant needed).
  - Terminal segment (`suspend: None`): `Poll.Ready(<tail block>)` ← mod.rs:2621–2633 (`terminal_ready`)
    + the tail-rewrite via `rewrite_arg_refs_in_block` (2612).
  - Field set: `__state: Int`, one `__sub_i` per suspend, one `self.<binding>` per suspend, plus outer
    args — exactly the union the four builders compute today. (No `__phase`.)

`build_poll_body` lives in `lower.rs` alongside the shared AST helpers (`self_field`,
`rewrite_arg_refs_in_{expr,block}`, `mangle_future_class_name`, the `Poll.Ready`/`Poll.Pending`
constructors) — MOVE those from `mod.rs` rather than copying (grep their current defs and relocate).

**Gating:** add an internal `fn lower_async_fn_via_cfg(func) -> Option<(FuncDef, ClassDef)>` that does
`segment_cfg(&func.body).map(build_poll_body...)`. The pass entry still calls the OLD five-path ladder
(mod.rs:131–197). The new path is exercised ONLY by the golden-equivalence test below until Task 5
flips the switch.

- [ ] **Step 1: Write the failing golden-equivalence test (the load-bearing risk control)**

Add to `lower.rs` an inline test that, for each of the five shapes, lowers via BOTH the old path and the
new CFG path and asserts the synthesised poll bodies are **runtime-equivalent**. Structural AST equality
is too strict (the skeletons differ by construction); assert equivalence the way the integration suite
does — compile-and-run both and compare output. Pin it as a unit test that builds the program twice:

```rust
#[test]
fn cfg_lowering_matches_old_path_on_all_five_shapes() {
    for src in FIVE_SHAPE_FIXTURES {            // one snippet per shape (see Task 3 fixtures)
        let old = lower_with_old_ladder(parse(src));   // existing dispatch
        let new = lower_with_cfg(parse(src));           // segment_cfg + build_poll_body
        // Both must produce a __<Fn>Future class with a `poll` method and the
        // same field set (modulo __phase, which the CFG path doesn't emit).
        assert_eq!(field_names_sorted(&new), field_names_sorted(&old).without("__phase"));
        // And — the real contract — both compile+run to identical stdout on the
        // async_io harness fixture for this shape.
    }
}
```

> Implementer: if compiling-and-running inside a unit test is too heavyweight, instead add ONE fixture
> per shape under `tests/fixtures/ruxen/` and a `tests/async_lowering.rs` integration test that runs
> each through the CFG path (feature-gated) and asserts identical stdout to the old path. The
> compile-and-run helper already exists at `async_io.rs:72` (`compile_and_run`) — reuse its shape.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ruxen_core --lib async_lowering::lower 2>&1 | tee tmp/test-cache/phase3-task4-red.log`
Expected: FAIL — `build_poll_body`/`lower_async_fn_via_cfg` not found.

- [ ] **Step 3: Implement `build_poll_body` + `lower_async_fn_via_cfg` (port from the 4 builders)**

Follow the per-arm porting map above. Keep the match over `Edge` exhaustive (no `_ =>`). Move the shared
helpers from `mod.rs` into `lower.rs`. Do NOT delete the old builders yet.

- [ ] **Step 4: Run to verify green**

Run: `cargo test -p ruxen_core --lib async_lowering::lower 2>&1 | tee tmp/test-cache/phase3-task4-green.log`
Expected: PASS — the CFG path produces equivalent poll behaviour on all five shapes.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/async_lowering/lower.rs compiler/ruxen_core/src/async_lowering/mod.rs
git commit -m "feat(async): build_poll_body — one CFG-driven poll skeleton (gated)

Single builder emitting the __state-indexed poll if/chain for any Cfg edge
topology, generalizing the shared skeleton the 4 hand-builders already emit.
__phase folds into __state (a loop is a back-edge). Gated behind
lower_async_fn_via_cfg; old five-path ladder still drives the pass. A
golden-equivalence test proves runtime parity on all five shapes before the
old paths are deleted in Task 5.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — confirm the `Edge` match is exhaustive,
  the golden test exists, and shared helpers were MOVED (not duplicated). Net LOC may rise this task
  (parallel path) — that is expected and reversed in Task 5; note it explicitly.

---

## Task 5: Route all five shapes through the CFG path; DELETE the old paths

**Files:**
- Modify: `compiler/ruxen_core/src/async_lowering/mod.rs` (replace the dispatch ladder 131–197 with one
  `lower_async_fn_via_cfg` call; delete the recognizers 1882/2023/2149 and the builders 2585/2887/3397
  and the now-dead `lower_async_fn_while_*`/`lower_one_async_fn[_with_await]` 390/672/1097/1434 and the
  structs `AwaitSub`/`Segments`/`LoopPhase`/`WhileMultiAwaitShape`/`WhileSingleAwaitShape` 1783–2005)
- Modify: `async_lowering/diagnostics.rs` (re-point the E1115 item guard from `recognize_while_*` to
  `segment_cfg(&func.body).is_some()` — the single call site noted in Task 1 Step 3)
- Test: the full async integration suite (this is the behaviour-preserving gate).

This is the deletion that realizes the ~1,750 net reduction. The pass entry shrinks to:

```rust
for item in program.items.iter_mut() {
    if let TopLevelItem::Function(func) = item {
        if !func.is_async || func.is_class_method { continue; }
        if let Some((rewritten, sm_class)) = lower::lower_async_fn_via_cfg(func) {
            *func = rewritten;
            new_classes.push(TopLevelItem::Class(sm_class));
        }
        // else: unsupported shape — left for the E1115 collector to diagnose,
        // exactly as the old ladder's final fall-through did.
    }
}
```

- [ ] **Step 1: Re-confirm the golden-equivalence test from Task 4 is the red/green gate**

No new test code; Task 4's `cfg_lowering_matches_old_path_on_all_five_shapes` is the pin. Before
deleting, run it once more to confirm green against current HEAD:
`cargo test -p ruxen_core --lib async_lowering::lower 2>&1 | tee tmp/test-cache/phase3-task5-pre.log`

- [ ] **Step 2: Delete the old paths + flip the dispatch**

Make the edits above. After deletion, the old golden-equivalence test that compared *two* paths can no
longer compile (the old path is gone) — convert it to assert the CFG path's output against the
golden fixtures directly (drop the `lower_with_old_ladder` half), OR delete it and rely on the
integration suite. Prefer converting: it keeps a fast per-shape unit pin.

- [ ] **Step 3: Run the async integration suite (the behaviour-preserving gate)**

```bash
cargo test -p ruxen_core --test async_lowering 2>&1 | tee tmp/test-cache/phase3-task5-lowering.log
cargo test -p ruxen_core --test async_negative 2>&1 | tee tmp/test-cache/phase3-task5-negative.log
cargo test -p ruxen_core --test async_executor 2>&1 | tee tmp/test-cache/phase3-task5-executor.log
cargo test -p ruxen_core --test async_io 2>&1 | tee tmp/test-cache/phase3-task5-io.log
cargo test -p ruxen_core --test async_surface 2>&1 | tee tmp/test-cache/phase3-task5-surface.log
```
Expected: ALL green. `async_io` (compile-and-run) and `async_executor` are the real behaviour proof —
the generated poll code changed, but observable async execution must be identical. If any shape
regresses, FIX FORWARD in `cfg.rs`/`lower.rs` (do not `git reset --hard`); the failing fixture names the
shape whose segmentation or edge-action diverged from the old builder.

- [ ] **Step 4: Confirm net reduction**

Run: `git diff --stat HEAD~1..HEAD` — expect a large negative delta from `mod.rs` (the 4 builders + 3
recognizers + 5 lowering entries + 5 structs removed). Cumulative phase target ~1,750 net.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/async_lowering/
git commit -m "refactor(async): one CFG lowering replaces 5 paths + recognizers

Delete recognize_while_single/multi_await, segment_body, the 4 specialized
poll builders, lower_async_fn_while_*/lower_one_async_fn[_with_await], and the
AwaitSub/Segments/LoopPhase/While{Single,Multi}AwaitShape structs. The pass now
runs every async fn through segment_cfg + build_poll_body. E1115 guard re-points
to segment_cfg. Behaviour pinned by the async integration suite (compile+run).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — confirm the 4 builders + 3 recognizers +
  5 structs are GONE, large net-negative LOC, no new `_ =>` in the surviving dispatch, no
  `#[ignore]`/passthrough sentinel introduced.

---

## Task 6: Finalize the file split (`mod`/`cfg`/`lower`/`diagnostics`)

**Files:**
- Modify: `compiler/ruxen_core/src/async_lowering/{mod,cfg,lower,diagnostics}.rs`
- Test: build-only + the async suite (already green from Task 5).

After Tasks 1/4/5, most code already lives in the right module. This task moves the residual
table-collection helpers and the `lower_async_defs[_with_bootstrap]` orchestrator into their final homes
and makes `mod.rs` the thin public surface.

- [ ] **Step 1: Move + re-export**

- `describe_await` and the `collect_class_*_returns_into` / `collect_future_outputs_into` helpers
  (mod.rs:245+) → `lower.rs` (they feed `Suspend.awaitee` construction). Keep
  `lower_async_defs`/`lower_async_defs_with_bootstrap` in `mod.rs` (the public entry) OR move to
  `lower.rs` and `pub use lower::{lower_async_defs, lower_async_defs_with_bootstrap};` — pick one and
  ensure `mod.rs` re-exports the full prior public surface (`lower_async_defs`,
  `lower_async_defs_with_bootstrap`, the three `collect_*_diagnostics`).
- `mod.rs` ends as: `mod cfg; mod lower; mod diagnostics; pub use diagnostics::*; pub use lower::*;`
  (plus whatever else was `pub`).

- [ ] **Step 2: Verify the public API is unchanged for external callers**

Run: `cargo build -p ruxen_core 2>&1 | tee tmp/test-cache/phase3-task6-build.log`
Expected: clean — `typeck/mod.rs:66,73,80,165,168,170` and `resolve/exprs.rs:434` resolve
`crate::async_lowering::*` unchanged. A build error here means a re-export is missing.

- [ ] **Step 3: Run the async suite (no behaviour change — pure move)**

Run: `cargo test -p ruxen_core --test async_lowering --test async_negative 2>&1 | tee tmp/test-cache/phase3-task6-async.log`
Expected: green (this task is a code move; the narrow async pins suffice — the full suite is Task 7).

- [ ] **Step 4: Confirm the four-file shape**

Run: `wc -l compiler/ruxen_core/src/async_lowering/*.rs` — each file is materially smaller than the
original 5,812-line monolith; no single file dominates.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/async_lowering/
git commit -m "refactor(async): split async_lowering into mod/cfg/lower/diagnostics

Pure code move: cfg.rs (Segment/Edge/segment_cfg), lower.rs (build_poll_body +
orchestrator + AST helpers + await-table collection), diagnostics.rs (the three
Visit-based collectors). mod.rs is the thin public surface; external callers
unchanged via pub use.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

- [ ] **Direction-check (/thermonuke on this task's diff)** — confirm no logic changed (move only),
  the public re-exports cover every prior `pub` symbol, no `_ =>` introduced.

---

## Task 7: Phase-3 final integration (phase gate)

**Files:** none (verification only).

- [ ] **Step 1: Run the full compiler-crate suite once**

Run: `cargo test -p ruxen_core 2>&1 | tee tmp/test-cache/phase3-final.log`
Expected: all green. Per global rules 41/42 this is the ONLY full-suite run in Phase 3; intermediate
tasks ran only their narrow async tests.

- [ ] **Step 2: Run the five async integration pins explicitly and cite them**

```bash
for t in async_lowering async_negative async_executor async_io async_surface; do
  cargo test -p ruxen_core --test $t 2>&1 | tee tmp/test-cache/phase3-final-$t.log
done
grep -E "test result" tmp/test-cache/phase3-final-async_*.log
```
Expected: all green. `async_io`/`async_executor` (compile-and-run) are the authoritative proof that the
changed generated poll code is runtime-equivalent.

- [ ] **Step 3: Confirm no catch-all sneaked into the new code**

Run: `grep -nE '_ =>' compiler/ruxen_core/src/async_lowering/{cfg,lower,diagnostics}.rs | grep -v 'mod tests'`
Expected: NO matches inside `segment_cfg`/`build_poll_body`/`validate`/the `Visit` impls. (A deliberate
`other => walk_expr(self, e)` default in a `Visit::visit_expr` is the recurse-everything-else arm, not a
drift hazard — distinguish it from a `_ => {}` that silently drops a variant.)

- [ ] **Step 4: Full multi-agent `/thermonuke` sweep (authoritative phase gate)**

Invoke the `thermonuke` skill on the whole Phase 3 diff (`git diff <phase3-base>..HEAD`). It must
confirm: the 5 hand-specialized lowering paths + 3 recognizers + 3 diagnostic-collector triplets are
*reduced to* one CFG lowering + one `segment_cfg` + three `Visit` collectors (not added alongside); net
reduction near the ~1,750 target; no new `_ =>`/god-function/`#[ignore]`/passthrough. Surface its report.

- [ ] **Step 5: Report**

Report to maintainer: net line delta (`git diff --stat <phase3-base>..HEAD`), the five async pins green
(cite the `phase3-final-*.log` files), full suite green (cite `phase3-final.log`), and the statement:
"Generated async poll code changed structurally (5 paths → 1 CFG), but observable async execution is
identical — proven by the compile-and-run async_io/async_executor pins and the per-shape golden-
equivalence test." Await go-ahead for Phase 4.

---

## Self-Review (run before handing off)

**Spec coverage:** master Phase 3 row — (a) e1112/e1115/e1116 → `Visit` (✓ Task 1, deletes ~700 LOC),
(b) 5 lowering paths + `recognize_*` allowlist → one CFG state machine (✓ Tasks 2–5), (c) file split
into `{mod,cfg,lower,diagnostics}` (✓ Task 6). Depends on Phase 1's `Visit`/`walk_expr` — referenced,
not redefined.

**Task ordering vs the mandated sequence:** (1) diagnostics→Visit first ✓; (2) `Segment`+edge model +
`segment_cfg` introduced alongside the old paths, gated ✓ (Tasks 2–3); (3) one `build_poll_body` ✓
(Task 4); (4) route all five through it + delete the 4 builders/recognizers behind a green integration
suite ✓ (Task 5); (5) split the file ✓ (Task 6). The general CFG lowering — the hardest part — is built
behind a parallel `lower_async_fn_via_cfg` path with a golden-equivalence test (Task 4) BEFORE the old
paths are deleted (Task 5), per the explicit risk guidance.

**Risk honesty:** the single general CFG lowering is the hardest and highest-risk step. Mitigations:
(a) the data model (Task 2) and analysis (Task 3) land before any codegen; (b) `build_poll_body` is
gated and never drives the pass until proven (Task 4); (c) a per-shape golden-equivalence test +
the compile-and-run `async_io`/`async_executor` pins are the deletion gate (Task 5); (d) fix-forward
only, no `git reset --hard`. The residual risk is an *unsupported* shape that the old ladder happened to
accept but `segment_cfg` rejects — controlled by porting the rejection set as the EXACT union of the
three recognizers (Task 3 Step 3) and by the E1115 negatives staying green.

**Placeholder scan:** three `todo!`-style sub-steps (E1112/E1116 message-string porting in Task 1; the
`Cfg` literal construction in Task 2's test) are flagged as explicit implementer obligations with exact
source line ranges to transcribe from (mod.rs:4735–4977, 4979–5242, 2585–3397+), because those literal
strings / hand-built fixtures weren't transcribed during planning and guessing them is invented
precision. Every structural decision (the `Cfg`/`Segment`/`Edge`/`Suspend` types, the `Visit` impl
shapes, the per-arm porting map, the gating strategy) is fully specified.

**Type/name consistency:** `segment_cfg(&Block) -> Option<Cfg>`, `build_poll_body(&Cfg, ...) -> Block`,
`lower_async_fn_via_cfg(func) -> Option<(FuncDef, ClassDef)>` (mirrors the deleted
`lower_one_async_fn`'s signature, mod.rs:390). Public collector names
(`collect_await_in_loop_diagnostics`/`collect_block_on_in_async_diagnostics`/
`collect_task_spawn_outside_async_diagnostics`) and `lower_async_defs[_with_bootstrap]` are PRESERVED and
re-exported, so `typeck/mod.rs` and `resolve/exprs.rs` need no edits.
