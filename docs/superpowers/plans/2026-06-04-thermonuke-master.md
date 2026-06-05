# Thermo-Nuclear Structural Refactor — Master Plan

> **For agentic workers:** This is the MASTER plan. Each phase below has (or will have) its own
> bite-sized TDD sub-plan in this directory. Use `superpowers:subagent-driven-development` or
> `superpowers:executing-plans` to implement each phase's sub-plan task-by-task. Phases are
> committed on top of the existing branch (single PR handled by the maintainer).

**Goal:** Remove ~6,500–7,000 lines (~5%) of structural duplication from the `ruxen` compiler and fix
3 latent correctness bugs, by introducing the two missing abstractions the codebase keeps
hand-rolling: a shared AST/`Ty` traversal, and data-driven runtime/method registries.

**Architecture:** Six independent, individually-shippable phases. Phase 1 is the keystone (a shared
traversal primitive); the 3 correctness bugs are fixed *by construction* as their hand-rolled walkers
migrate onto it. Phases 2 & 5 replace ordered match-ladders with lookup tables. Phase 3 collapses five
async-lowering paths into one CFG state machine. Phase 4 deletes a forked Cranelift backend. Phase 6
sweeps the remaining duplication + cheap wins.

**Tech Stack:** Rust 1.91, workspace crates (`ruxen_core` is the compiler), `cranelift_module`,
`cargo test -p ruxen_core`. No new dependencies are introduced by any phase.

---

## The two root causes (why this plan exists)

**Root Cause A — no shared traversal.** `grep -rnE 'trait (Visit|Walk|Fold)' compiler/ruxen_core/src`
returns nothing. Every "walk the tree looking for X" need re-hand-rolls the recursion. Counted: 5
walkers in `async_lowering`, 8 in `repl/eval.rs`, 1 in `formatter/comments.rs`, 6 `Ty`-walkers in
`typeck`, 5 `walk_tys_in_*` in `mir/lower/monomorphize.rs`. Each copy has its own `_ => false`/
`_ => {}` catch-all, and **they have already drifted into 3 live bugs** (see below).

**Root Cause B — behaviour encoded as match-arm order.** `typeck/method_resolvers/mod.rs` is a
~1,090-arm `match (ty, method)`; `mir/lower/drops.rs` encodes the runtime ownership ABI as 6
overlapping (and self-contradicting) string-literal sets. Every fix is surgery in the middle of a
ladder; the current branch diff is literally *the second patch* around the method-resolver ladder.

## The 3 live bugs fixed by Phase 1 (by construction)

1. **Await invisible inside several expression forms.** `async_lowering/mod.rs:4351`
   `expr_contains_await` has `_ => false`, so `.await` inside `EnumVariant` (`Some(g().await)`),
   `UnsafeBlock` (`unsafe { g().await }`), `IfLet`, `SafeNav`/`SafeNavCall`, `Range`, `MapLiteral`,
   `ArrayFill`, `MacroCall`, `Yield` is **not detected** → the fn routes to the no-await path →
   downstream the user gets a misleading E1110 instead of correct lowering/E1115.
2. **`block_on` not rewritten inside `while let`.** `async_lowering/mod.rs:5317`
   `rewrite_block_on_in_expr` lacks a `WhileLet` arm (and others) → `block_on(...)` inside a
   `while let` body is silently left un-rewritten.
3. **`Ty` substitution gaps.** `typeck/infer/collect.rs:798` `subst_ty` recurses through `Ref`/
   `RefMut` but **not** `RefLifetime`/`RefMutLifetime`, and covers `Class`/`Struct`/`Enum`/`Result`/
   `Tuple`/`Option`/`Array` but **not** `Map`/`Set`/`FixedArray`/`Newtype`/`Alias`/`Fn`/`RawPtr*`.
   A method returning `&'a MutexGuard[T]` or `Map[K,V]` leaves type params unsubstituted →
   `?T<n>_method` mangling → **link failure** — the exact failure mode the current diff patches around.

---

## Phase dependency graph

```
Phase 1 (AST Visit + Ty::map_inner + 3 bug fixes)   ← KEYSTONE
   ├──> Phase 3 (async_lowering CFG unification)     uses Visit for e1112/e1115/e1116 collectors
   └──> Phase 6 (derive/expr/eval/comments cleanup)  uses Visit/Ty::map_inner for remaining walkers

Phase 2 (RuntimeAbi table: drops + method_call)      independent
Phase 5 (method-resolver pipeline: typeck)           independent (Root Cause B, sibling of Phase 2)
Phase 4 (Cranelift share: TranslationEnv<M: Module>) independent
```

**Recommended execution order:** 1 → 2 → 5 → 3 → 4 → 6.
Rationale: 1 first (keystone + bugs). Then 2 & 5 (table conversions — high correctness leverage, low
risk, independent). Then 3 (biggest single-file reduction, needs Phase 1). Then 4 (highest risk —
needs a de-risking spike). Then 6 (cleanup sweep, needs Phase 1). Each phase compiles and passes the
full suite on its own before the next begins.

---

## Cross-phase conventions

- **TDD, refactor-first.** Per maintainer decision: bugs are fixed *as a side effect* of building the
  exhaustive primitive, not as separate prior patches. Each migration task writes a characterization
  test (pins current correct behaviour) **and** a bug-exposing test (red → green) before refactoring.
- **Exhaustive matches, no catch-alls.** Every `walk_*` / `map_inner` matches every variant explicitly.
  `_ => ...` is banned in the new traversal code — that ban is the whole point (adding an AST/`Ty`
  variant must become a compile error, not a silent drift). Enforced by `#![deny(...)]`? No — by code
  review + the fact that the match has no wildcard arm.
- **Test commands (real):** unit/inline `cargo test -p ruxen_core <module>::tests`; integration
  `cargo test -p ruxen_core --test <name>` (e.g. `async_lowering`, `async_negative`, `regex_typeck`).
- **Test caching (global rule 41/42):** each task caches its narrow run to
  `tmp/test-cache/<phase>-<tag>.log`. The **full** `cargo test -p ruxen_core` runs **once** at each
  phase's final integration task — never per intermediate task (additive/refactor changes run only
  their own narrow tests). Re-running answers a question you already have; grep the cached log instead.
- **Commits:** one commit per completed task (test+impl together), conventional-commit messages,
  on the existing branch. End each commit message with the Co-Authored-By trailer.
- **No behaviour change except the 3 named bug fixes.** Every other migration is behaviour-preserving;
  the characterization tests are the proof.
- **`/thermonuke` direction-check after EVERY task (maintainer-mandated).** The final step of every
  task — across all phases — invokes the `thermonuke` skill scoped to *that task's* incremental diff
  (`git diff <task-base>..HEAD`), to confirm the change is *reducing* structural complexity and not
  drifting. A per-task check is a single focused review of a small diff (not the multi-agent
  whole-project sweep). It must confirm: (a) net lines moved in the intended direction, (b) **no new
  `_ =>` catch-all** in any traversal/table, (c) no new god-function or special-case leaking into a
  shared path, (d) the task's structural goal was actually met (e.g. "a hand-rolled walker is gone,
  not added"). If the check flags drift, STOP and surface it before the next task. The **full
  multi-agent `/thermonuke` sweep** runs once at each phase's final-integration task (and is the
  authoritative phase gate shown to the maintainer).

---

## Risk register

| Risk | Phase | Likelihood | Mitigation |
|---|---|---|---|
| Visitor can't express "override + don't recurse" (closure-opacity in await-scan) | 1 | Med | Standard rustc pattern: `visit_expr` decides whether to call `walk_expr`. Closure arm simply doesn't recurse. Pinned by a test asserting `Some(async {...}.await-free closure)` stays opaque. |
| Migrated walker changes behaviour for an untested edge | 1,3,6 | Med | Characterization test FIRST on every migration; the full integration suite at phase end is the backstop. |
| `RuntimeAbi` table miscategorises a symbol → double-free/UAF | 2 | High-impact | Port each of the 6 existing lists into the table 1:1, then assert (in a test) the table reproduces every old predicate's answer for every symbol it mentioned. No symbol changes category without a test diff. |
| `TranslationEnv<M: Module>` borrow-split doesn't genericize cleanly | 4 | Med | **Mandatory de-risking spike task** before committing to the full share; if it fights the borrow checker, fall back to a macro-generated shared core. Pin test: identical CLIF (or identical execution) from both backends on a fixed MIR fixture. |
| Method-resolver pipeline reorders precedence vs the 1,090-arm match | 5 | High-impact | Build the table so existing arm-order is preserved exactly; a golden test runs a corpus of `(receiver_ty, method)` pairs through old and new and asserts identical results. |
| `ruxen_noop_passthrough` (global-rule-banned) is load-bearing | 4/6 | Low | Investigate first: confirm it's a real MIR sentinel and document why the ban doesn't apply, or delete it. Decision recorded in Phase 6 plan. |

---

## Verification bar per phase

A phase is "done" only when: (a) every task's narrow test is green and cached; (b) the full
`cargo test -p ruxen_core` is green (run once, cached to `tmp/test-cache/<phase>-final.log`); (c) the
net line delta is reported; (d) for behaviour-preserving phases, a one-line statement that no
non-bug-fix behaviour changed, backed by the characterization tests. Results shown to maintainer.

---

## Phase summaries

Detailed sub-plans (all written, full bite-sized TDD detail, each task ends with a `/thermonuke`
direction-check):
- **Phase 1** → `2026-06-04-phase1-ast-visit-and-ty-fold.md`
- **Phase 2** → `2026-06-04-phase2-runtime-abi-table.md`
- **Phase 3** → `2026-06-04-phase3-async-cfg-unification.md`
- **Phase 4** → `2026-06-04-phase4-cranelift-share.md`
- **Phase 5** → `2026-06-04-phase5-method-resolver-pipeline.md`
- **Phase 6** → `2026-06-04-phase6-cleanup-sweep.md`

Where a phase calls an API created in an earlier phase (3 & 6 use Phase 1's `Visit`/`Ty::map_inner`),
the sub-plan references that now-specified API directly. Independent phases (2, 4, 5) are fully
concrete against current `main`. Any task whose exact code can only be finalized against not-yet-built
code is marked with an explicit implementer sub-step (not a silent placeholder). Each summary below is
the approved design; the sub-plan file holds the tasks.

### Phase 1 — Foundation: AST `Visit`/`VisitMut` + `Ty::map_inner` (+ 3 bug fixes)
**Files:** create `compiler/ruxen_core/src/parser/visit.rs`; modify `parser/mod.rs` (mod decl),
`hir/types.rs` (add `Ty::map_inner`/`peel_refs`), `typeck/infer/collect.rs` (subst_ty→map_inner),
`async_lowering/mod.rs` (await-scan + block_on-rewrite → Visit). **Net:** small + (the 3 bugs closed).
See the full sub-plan. **This unblocks 3 and 6.**

### Phase 2 — `RuntimeAbi` table (`mir/lower`)
**Move:** replace the 6 overlapping string-sets in `drops.rs` (`FRESH_ALLOC_CALLEES` ~110 entries,
`is_runtime_consume_helper`, `is_runtime_borrow_helper` rebound 3×, `is_move_by_ffi_callee`,
`is_pointer_store_helper`, `borrows_first_arg`) and the static-ctor knowledge duplicated in
`method_call.rs`/`util.rs` with one `fn callee_ownership(&str) -> CalleeOwnership { result:
Fresh|Borrowed|None, arg_transfer: ArgMask }` sourced from a single declarative table.
**Files:** create `mir/lower/runtime_abi.rs`; modify `drops.rs`, `expr/method_call.rs`, `util.rs`.
**Tasks (outline):** (1) characterization test capturing every current predicate's answer for every
named symbol; (2) define `CalleeOwnership` + the table; (3) port `drops.rs` `Call` arm to one lookup,
assert test parity; (4) port `method_call.rs` static-ctor list to the table; (5) delete
`util.rs:is_builtin_static_method` dup; (6) phase-final full suite. **Est. −400 (drops) −350 (method_call).**
**Pin tests:** `ffi_alias_single_entry.rs`, `task_spawn_ownership.rs`, `drop_fixtures.rs`.

### Phase 5 — Data-driven method resolution (`typeck/method_resolvers`)
**Move:** convert the ~1,090-arm `match (ty, method)` into an ordered resolver pipeline
(`declared-method → named-stdlib-resolver → structural-builtin`) as the file header itself specs.
Precedence becomes one decision instead of arm position; fixes latent bug A2 (user class named like a
stdlib type whose declared `new -> Result` is shadowed). **Files:** `method_resolvers/mod.rs` + new
`method_resolvers/{collections,strings,concurrency,io,...}.rs`. **Tasks (outline):** (1) golden test:
a corpus of `(receiver_ty, method)` → expected return, captured from current behaviour; (2) introduce
`struct MethodResolver { matches, ret }` + dispatcher walking an ordered `Vec`; (3) migrate arms
namespace-by-namespace, asserting golden parity after each; (4) collapse the `builtin_method_type`
thin wrapper (`collect.rs:329`) into the dispatcher seam; (5) phase-final suite. **Est. ~1,090 arms →
~150-line dispatcher + small tables; ~900 net.** **Pin test:** `regex_typeck.rs`.

### Phase 3 — async_lowering CFG unification (`async_lowering`)
**Move:** replace the 5 hand-specialized paths (no-await / linear-N / while-1 / while-N / multi-phase)
and their `recognize_*Shape` syntactic allowlist with one segment/CFG state-machine lowering
(await-delimited basic blocks + edge list; loops = back-edges). Migrate the e1112/e1115/e1116
diagnostic collectors onto Phase 1's `Visit`. **Files:** split the 5,812-line `async_lowering/mod.rs`
into `async_lowering/{mod,cfg,lower,diagnostics}.rs`. **Tasks (outline):** (1) migrate the 3 diagnostic
collectors to `Visit` (depends on Phase 1), characterization tests; (2) introduce `Segment`+edge model
+ a `segment_cfg(body)` that subsumes the 3 `recognize_*`; (3) one `build_poll_body(segments, edges)`;
(4) route all shapes through it, deleting the 4 specialized builders behind golden async-output tests;
(5) phase-final suite. **Est. ~1,750 net (~30% of the file).** **Pin tests:** `async_lowering.rs`,
`async_negative.rs`, `async_executor.rs`, `async_io.rs`, `async_surface.rs`.
**Scope honesty:** this phase delivers the LOC reduction + a single lowering code path, but is
*behaviour-preserving on the accepted set* — `segment_cfg` initially accepts the **exact union** of
what the 3 `recognize_*` accepted and rejects the rest with the same E1115 (fix-forward). Broadening
async acceptance (loops with N awaits, `for`, etc.) is deliberately **deferred** to a later, separate
change — Phase 3 unifies the *implementation*, it does not yet widen the *allowlist*.

### Phase 4 — Share the Cranelift core (`codegen/cranelift` + `repl/jit.rs`)
**Move:** genericize `TranslationEnv` over `cranelift_module::Module`, promote the 12 byte-identical
emit/translate helpers from `pub(super)` to `pub`, reduce `repl/jit.rs` from 1,973 lines to a ~400-line
JIT adapter (JITModule construction + the capture-shim `register_runtime_symbols` + finalize). **Files:**
`codegen/cranelift/{mod,emit,helpers}.rs`, `repl/jit.rs`. **Tasks (outline):** (0) **MANDATORY SPIKE**:
prototype `TranslationEnv<'_, M: Module>` borrow-split on one helper; if it fights, switch to the
macro-shared-core fallback and re-plan; (1) genericize `TranslationEnv`; (2) promote+share helpers one
at a time; (3) rewrite `jit.rs` as the adapter; (4) **both-backends-identical** pin test on a fixed
MIR fixture so it can't silently re-fork; (5) resolve the `ruxen_noop_passthrough` ban question;
(6) phase-final suite. **Est. ~1,500 net.** Highest risk — spike-gated.

### Phase 6 — Cleanup sweep
**Move:** (a) `derive.rs`: make struct-debug path call the existing-but-bypassed `format_field_for_debug`,
introduce `fold_struct_fields` driver (−250); (b) `typeck/infer/expr.rs`: split the ~300-line
`MethodCall` god-arm into `infer_constructor_call`/`infer_selected_method`/`infer_combinator_block`
(−270); (c) migrate `repl/eval.rs`'s 8 walkers + `formatter/comments.rs` span walk +
`monomorphize.rs` `walk_tys_in_*` onto Phase 1's primitives (−500+, closes remaining drift);
(d) cheap wins: dedupe `COLLECTION_BUILTINS` const (ffi_registration ↔ resolve/types), rename
`library/std/foobar` → `_pin_zero_rust_stdlib`, finalize the `ruxen_noop_passthrough` decision.
**Files:** as listed. **Tasks:** one per item, each with characterization test. **Est. ~1,000+ net.**

---

## Aggregate target

| Phase | Net deletion (est.) | Bugs fixed | Risk |
|---|---|---|---|
| 1 | small + foundation | **3** | Low |
| 2 | ~750 | (UAF class auditable) | Low |
| 5 | ~900 | latent A2 | Low-Med |
| 3 | ~1,750 | (5 paths→1; allowlist preserved, not widened) | Med |
| 4 | ~1,500 | — | Med (spike-gated) |
| 6 | ~1,000+ | remaining drift | Low |
| **Total** | **~6,500–7,000** | **3 + 2 classes** | |
