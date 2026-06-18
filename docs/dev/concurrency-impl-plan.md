# Concurrency adoption — implementation plan (§4.1 foundation first)

Authoritative spec: `docs/decisions/zio-comparison-concurrency-adoption.md`.
This plan decomposes **§4.1 (first-class cancellation + leak-free `def drop`
across `.await`)** — the foundation everything else (§4.2–§4.7) sequences on,
per §6 — into small, pin-tested TDD increments, after resolving the one real
architectural fork the spec flags.

---

## Step 1 — KEY DESIGN DECISION: route cancel/drain drop through the EXISTING future class_info (NARROW), not global `mixin Drop dispatch runtime` (BROAD)

**Decision: NARROW.** Add a second vtable slot — a `Drop` vtable — *inside the
class_info struct that future classes already carry*, and synthesize a
`Drop_dynamic_drop(ptr)` helper that the C scheduler/drain calls. Do **not**
flip `mixin Drop` to `dispatch runtime` (which would add an 8-byte header to
**every** droppable class — `Mutex`, `File`, `SharedSync`, every user class with
`def drop`). The narrow path touches the layout of **future classes only** —
classes that already pay the class_info cost via `include Future`.

### Why the narrow path is sound and minimal-blast-radius (evidence)

1. **Every async future already carries a class_info header.** `mixin Future
   dispatch runtime` (`library/std/future/src/lib.rx:52`) gives every implementor
   an 8-byte `class_info_ptr` at heap offset 0
   (`docs/specs/types/mixin_vtables.spec.md` §B2; populated by `emit_class_info_init`,
   `compiler/ruxen_core/src/mir/lower/emit.rs:405`). The synthesized `async def`
   state machine `include Future` (segments `["Future"]`,
   `compiler/ruxen_core/src/async_lowering/lower.rs:630` and `:1135`), so it is
   already a runtime-dispatch class. Adding a Drop slot costs it **zero** extra
   header bytes.

2. **The class_info struct already widens for >1 mixin — internally, not in the
   instance header.** `class_field_index_shift`
   (`compiler/ruxen_core/src/mir/lower/type_helpers.rs:366-382`) returns **1** for
   any class with non-empty `runtime_dispatch_includes` — exactly one header slot,
   *regardless of how many vtables the class_info points at*. The class_info data
   section itself is emitted generically from `MirClassInfo.vtable_symbols`
   (`compiler/ruxen_core/src/codegen/cranelift/mod.rs:367-394`): N vtable symbols →
   an N-pointer `__const` struct, all relocations. So a future class_info holding
   `[future_vtable, drop_vtable]` is a 16-byte const struct pointed at by the same
   single offset-0 header pointer. **The instance layout of a future does not
   change; the const class_info struct grows by 8 bytes.** Non-future classes are
   untouched (their `runtime_dispatch_includes` stays empty → shift 0 → no header).

3. **The vtable + dynamic-dispatch-helper machinery is already generic over any
   runtime-dispatch mixin.** `collect_mixin_vtables`
   (`compiler/ruxen_core/src/mir/lower/mod.rs:908-950`) walks each class's
   `runtime_dispatch_includes` and emits one `MirVtable` per mixin from
   `mixin_info.required_methods`; `synthesize_dynamic_dispatch_helpers`
   (`mod.rs:969-1050`) emits `<Mixin>_dynamic_<method>` for every runtime-dispatch
   mixin that has an implementor, doing `class_info[mixin_slot] → vtable[method_idx]
   → indirect call` (`build_dynamic_dispatch_helper`, `mod.rs:1053-1136`). Drop is
   a 1-method mixin (`mixin Drop` / `def drop`,
   `library/std/core/src/lib.rx:232-234`), so this synthesizes `Drop_dynamic_drop`
   verbatim — *if* future classes carry Drop in `runtime_dispatch_includes` at
   slot 1. **No new synthesis code is needed for the helper; only the include
   wiring.**

4. **The drain hook is waiting for exactly this symbol.**
   `library/std/future/runtime/scheduler.c:347-384` —
   `ruxen_executor_drain_remaining` explicitly does NOT call user `def drop` and
   its comment says "swap in the call to `Drop_dynamic_drop(future_ptr)` here"
   once Drop is runtime-dispatched. `block_on`-exit drain is currently un-plumbed
   (no `drain_remaining` call exists in the block_on rewriter,
   `compiler/ruxen_core/src/async_lowering/lower.rs:2731+`; the lib decl exists at
   `library/std/future/src/task_handle.rx:38`).

5. **Spec §B11 already decided the broad path is out of scope for v1.**
   `mixin_vtables.spec.md` §B11: "for v1, only `Future` opts into `dispatch
   runtime`… The migration to `Drop dispatch runtime` etc. is a separate
   follow-up — each mixin's migration is its own commit + memory-layout review."
   The spec §4.1 risk (4) names the broad-Drop ABI change as "needs a
   layout-stability pin." The narrow path **sidesteps that risk entirely**: no
   non-future class layout moves.

### What NARROW costs vs BROAD

| | NARROW (future-vtable-routed) | BROAD (`mixin Drop dispatch runtime`) |
|---|---|---|
| Layout of `Mutex`/`File`/`SharedSync`/user classes | unchanged | +8 bytes header each |
| Layout-stability blast radius | future classes only | every droppable class (the §4.1 risk-4 ABI change) |
| New synthesis code | wire Drop into future `runtime_dispatch_includes` (slot 1) + emit Drop vtable when a future has `def drop` | flip `mixin Drop` modifier + audit every C `sizeof`/offset (spec §B11) |
| Reuses existing `*_dynamic_*` machinery | yes, verbatim | yes |
| Future generality (Drop on arbitrary `dyn` collections) | deferred (v2, spec out-of-scope) | enabled now (but unneeded for §4.1) |

NARROW delivers the §4.1 guarantee — `def drop` fires on drain/cancel for **any
future** — with the smallest possible layout change. BROAD's only extra
capability (polymorphic Drop over non-future `dyn` collections) is explicitly v2
(spec §B9/§B11 "next round" / "out of scope"). Per hard-rule 4 (no abstractions
for hypothetical futures): build the narrow thing §4.1 needs.

### Narrow-path mechanism (how a Drop slot lands in the future class_info)

The synthesized state machine, and every hand-written future with a `def drop`
(`TimeSleepFuture` `:51`, `TaskJoinFuture`, `AsyncReadWithTimeoutFuture`), must
have **Drop added to its `runtime_dispatch_includes` at slot 1** so:
- `collect_mixin_vtables` emits `__rx_vtable_Drop_for_<Future>` (pointing at
  `<Future>_drop`) and lists it as the **second** entry in
  `__rx_classinfo_<Future>` (after the Future vtable);
- `synthesize_dynamic_dispatch_helpers` emits `Drop_dynamic_drop(self)` reading
  `class_info[1] → drop_vtable[0] → <Future>_drop(self)`.

The wiring options (decided in Increment 1):
- (A) In `async_lowering/lower.rs`, when synthesizing the state machine, emit
  `include Drop` alongside `include Future` (so resolve adds Drop to
  `runtime_dispatch_includes` *iff Drop is a runtime-dispatch mixin*). This
  requires `mixin Drop` to BE `dispatch runtime` — which is the BROAD path. **Rejected.**
- (B) **Chosen:** keep `mixin Drop` static, and add a *targeted* MIR-lowering step
  that, for **runtime-dispatch (future) classes that have a `def drop`**,
  synthesizes a `Drop` `MirVtable` + appends its symbol to that class's
  `MirClassInfo.vtable_symbols`. This is a compiler-internal augmentation of the
  vtable tables for future classes only; it does not change `mixin Drop`'s
  dispatch mode and does not touch any non-future class. The `Drop_dynamic_drop`
  helper is synthesized the same generic way once a `Drop` vtable exists.

Mixin-slot indexing: the dynamic-dispatch helper hardcodes `mixin_slot = 0` today
(`mod.rs:1095`, v1 single-mixin). The Drop helper must read **slot 1**. Increment
1 generalizes `build_dynamic_dispatch_helper` to take the mixin's slot index from
the class_info ordering (Future=0, Drop=1) rather than the literal `0`. This is
the one place the "v1 single-mixin" assumption is widened — and it is the change
spec §B8 anticipated ("the offset for each mixin's slot… computed at codegen
time"). Because `Drop_dynamic_drop` must work across *all* future classes, the
helper reads the slot from a per-class offset (all futures put Drop at slot 1, so
v1 can use a fixed `1` for Drop and `0` for Future; a per-class offset table is
the clean generalization but a fixed `1` is correct while Future is always slot 0).

---

## §4.1 decomposed into TDD increments

Each increment: failing pin first → minimal change → narrow tests + relevant
stability pin only (rules 41/42) → CHANGELOG + module CLAUDE.md + TASKS.md.

### Increment 1 — a drained/cancelled future runs its `def drop` (THIS TASK)
- **Pin:** `tests/release-e2e/cases/NNN_async_drop_on_drain.rx` — a top-level
  `async def` owns a value whose `def drop` has an observable effect; the program
  drops/drains the future before completion; expected stdout proves the drop ran.
- **Change:**
  1. Generalize `build_dynamic_dispatch_helper` to dispatch a mixin at a given
     class_info slot (Future=0, Drop=1).
  2. Synthesize a `Drop` `MirVtable` + class_info slot for every runtime-dispatch
     (future) class that has a `def drop` (`user_drop_classes`), emitting
     `Drop_dynamic_drop`.
  3. Wire `ruxen_executor_drain_remaining` (scheduler.c) to call
     `Drop_dynamic_drop(future_ptr)` on every still-queued/parked future before
     freeing, and plumb a `drain_remaining` call into the `block_on`-exit path so
     the root future's still-pending sub-futures drain.
- **Layout pin:** extend `tests/mixin_vtables.rs` (or a new
  `async_drop_layout_stability` pin) asserting (a) a non-future droppable class's
  field shift stays 0 (no header), (b) a future class_info now has 2 vtable slots
  with Future at 0, Drop at 1, and the future instance header stays a single
  offset-0 pointer.

### Increment 2 — state-indexed drop glue on the synthesized state machine

**Ground-truth re-derivation (verified this increment — supersedes earlier guesses):**

1. **Ruxen has NO automatic recursive per-field drop.** When a class has no
   `def drop`, its fields are NOT dropped (they leak) — `drops.rs:404-466` emits
   only the user `{Class}_drop(self)` (if any) + `ruxen_dealloc(self)` for the
   whole block; there is no field walk. When a class HAS `def drop`, the user body
   FULLY owns field cleanup; the compiler does NOT append field-drops. (Confirmed
   by Explore over `drops.rs`, `drop-elaboration.md`.) **Consequence:** the
   synthesized state machine's `def drop` must EXPLICITLY drop each live field — no
   machinery does it for us.

2. **The state machine `init` is EAGER** (`lower.rs:529-579`): pre-first-await
   stmts run, crossing locals are copied to `self.<name>`, ALL `__sub_i` are
   constructed, and binding fields get a synthesizable default (Int/Bool/Float/
   Unit/Option.None/Result — all Copy/trivial, `default_value_for_type`
   `lower.rs:2234`). So at `__state == 0` every `__sub_i` and every pre-await
   crossing local is ALREADY live; only a binding field WITHOUT a default holds
   garbage until its suspend resolves (partial-init case).

3. **`<Future>_drop` is called AT MOST ONCE per instance** — there is no
   Ready-path field-drop to race with. A spawned future is freed by the scheduler
   (drain calls `Drop_dynamic_drop` once; the pump-completion path
   `scheduler.c:338` does NOT call drop on completed futures); a block_on root
   future is scope-exit-dropped once by `__block_on_fut`'s owner. `Task.spawn_raw`
   taints the spawned local out of `alloc_rooted` (fixture `task_spawn_move_by_ffi`),
   so a spawned future is never ALSO scope-exit-dropped. **The double-drop
   discriminant is therefore `__state` alone** — within one `<Future>_drop` call we
   drop each live field exactly once; nothing outside this method drops these
   fields. (This is simpler than spec §4.1 risk-1 feared, BECAUSE Ruxen has no
   auto field-drop — the risk-1 "Ready path already drops fields" premise does not
   hold for this state machine.)

**Design (chosen layer + mechanism):** synthesize the state machine's
`def var drop` as AST in `async_lowering` (same layer that already emits `init`/
`poll`), switching on `self.__state`. For each field LIVE at a given state, emit a
move into a fresh local — `let __drop_<field> = self.<field>` — so the EXISTING MIR
drop-elaboration fires the correct `<Type>_drop` + `heap_free_callee` at that
local's scope exit. This reuses all proven drop machinery (ffi-alias resolution,
element-aware free helpers) and keeps `async_lowering` type-agnostic; we do not
reinvent field-type-directed drops. The presence of `def drop` makes the state
machine a user-drop class → Increment 1 gives it the concrete `<Future>_drop` in
its Drop vtable slot (replacing the `_drop_noop`), so drain/cancel reaches it.

**AS-BUILT scope (narrowed during impl + two Codex passes — supersedes the
"drop everything live" sketch above):** drop ONLY the `__sub_i` sub-future fields.
They are the resources that actually leak on drain/cancel (a parked sub-future
owns the reactor timer / fd / read-state) and they are the only fields whose
liveness we can prove WITHOUT per-state metadata, via a runtime NULL guard:

- **`__sub_i` sub-futures — dropped, each NULL-GUARDED (`if self.__sub_i != 0`).**
  Linear machines build every `__sub_i` eagerly in `init`; LOOP machines build
  them LAZILY in `poll` (init only zeroes `__sub_ready_i`), so a loop future
  drained before its first pump has a ZERO `__sub_i`. The guard drops only
  constructed sub-futures — `Drop_dynamic_drop(self.__sub_i)` + `ruxen_dealloc`,
  reverse decl order. Without the guard, `Drop_dynamic_drop(0)` null-derefs the
  class_info load (the blocker Codex's 2nd pass caught).
- **Bare-named await-binding fields — NOT dropped.** The `r` in
  `let r = make().await` is a non-`__` field with no synthesisable default → holds
  UNINITIALISED memory until its await resolves → dropping it while parked at an
  earlier state is UB (partial-init / risk 2). Pin 394 guards this.
- **Cross-suspend USER-DROP class locals — deferred (follow-up).** A pre-await
  crossing local IS eagerly inited and could be dropped, but at MIR it is
  name-indistinguishable from a binding-result field, so this needs the eager-vs-
  late metadata threaded from async-lowering.

**Double-drop discriminant (risk 1) — `<Future>_drop` is the SOLE dropper of
`__sub_i`,** so `__state` is not even needed as a discriminant: the poll body only
BORROWS `self.__sub_i` (`(&var self.__sub_i).poll(cx)`); the pump never drops a
completed future (`scheduler.c:336`); a spawned state machine is tainted out of
scope-exit drop (`MOVE_BY_FFI`); a block_on root is scope-dropped once. Each
sub-future therefore drops exactly once. (Simpler than the spec's risk-1 fear,
because Ruxen has no auto field-drop → there is no Ready-path field-drop to race.)

- **Pins (all GREEN):**
  - `392_async_subfuture_drop_on_drain.rx` — linear `async def` awaits a
    Pending-forever sub-future with an observable `def drop`; drained while parked
    at suspend #0 → sub_0's `def drop` fires EXACTLY ONCE.
  - `393_async_drop_count_exactly_once.rx` — double-drop guard: drain fires the
    drop once; a SECOND drain (empty queue) is a no-op.
  - `394_async_binding_not_dropped_uninit.rx` — droppable-`Output` binding NOT
    dropped while uninitialised (no UB), sub-future drop still fires.
  - `395_async_loop_drain_before_pump.rx` — LOOP future drained before first pump;
    zero `__sub_0` skipped by the NULL guard, no crash, no spurious drop.
  - Re-ran `mixin_vtables.rs` layout pins (the state machine now has a real
    `<Future>_drop`, so its Drop slot points at the concrete drop, not the no-op)
    and `task_spawn_ownership` / the async e2e sweep.
- **Follow-up leak (Codex LOW, parked for user):** loop re-init overwrites a
  completed prior iteration's `__sub_i` with a new pointer without dropping the
  old one (`async_lowering/lower.rs` loop builder) — a pre-existing intra-loop
  LEAK (not a UAF, the old pointer is lost not freed). Increment 2's drain glue
  improves the FINAL iteration's cleanup; the intra-loop overwrite is a distinct
  loop-builder fix.

### Increment 3 — cooperative `cx.cancelled?` bit through `Context` (DONE)

**Surface (chosen, honest):** a `_Thread_local int8_t` cancel flag in `executor.c`
— NOT per-`Context` — because the user async-fn body never sees `cx` (only the
synthesised `poll(cx)` does), exactly like the thread-local reactor handle
`ruxen_reactor_current_handle()`. Read via `cx.cancelled?` (a `Context` method,
for poll bodies that hold `cx`) AND the bare free fn `ruxen_cancel_requested()`
(for any body that does not); set via `ruxen_request_cancel()`, reset via
`ruxen_clear_cancel()`. Explicit-check surface (a Bool the user/leaf-future
checks), NOT an auto-inject — the current lowering doesn't thread `cx` into user
code and async-methods-on-classes are unlowered, so explicit-check is what's
supported. COOPERATIVE: only acts where a future CHECKS the bit; does NOT preempt
an await-free compute loop (spec §4.1 risk 5). The flag is thread-scoped and
PERSISTS across `block_on` calls until `ruxen_clear_cancel()` (so a cancel
requested before `block_on` is observed) — documented, pinned by `398`.

**Cancel → early-return → drop-glue path:** a leaf future's `poll` checks
`cx.cancelled?` and returns `Poll.Ready(<sentinel>)` instead of `Pending`; the
awaiting state machine completes; **the block_on root future is then dropped at
scope exit**, firing the Increment-2 `<Future>_drop` glue → its live `__sub_i`
sub-futures are cleaned up.

**Pre-existing leak this surfaced + fixed:** the block_on root future
(`__block_on_fut_N = worker()`) was NEVER scope-exit-dropped — its local is
assigned from the wrapper-call result, which the conservative dealloc analysis
taints. So `block_on(async_fn())` leaked the root future AND its sub-futures on
EVERY call, and a cancelled future's resources were never cleaned. Fix
(`drops.rs::compute_dealloc_safe_locals`): force-root a `__block_on_fut_*`
Future-typed local so `insert_drops` emits its `<Class>_drop` + `ruxen_dealloc`
(the spec §4.1 "block_on-exit path calls Drop_dynamic_drop"). SOUND by a
STRUCTURAL consume-check (not the name): force-root only when the local is never
passed by-value to a call, stored (`SetField`), moved/copied/assigned-into, or
returned — a genuine block_on root is only BORROWED by `poll`. A same-named user
local that IS consumed stays tainted → no double-free (pinned by `397`, after a
Codex high finding).

- **Pins (all GREEN):**
  - `396_async_cooperative_cancel.rx` — leaf observes `cx.cancelled?`,
    early-returns, block_on-exit drop fires the sub-future's `def drop`.
  - `397_block_on_fut_name_collision_safe.rx` — a consumed user local sharing the
    `__block_on_fut_` name is NOT force-rooted (no double-free / clean exit).
  - `398_cancel_flag_persists_until_clear.rx` — the cancel flag persists across
    block_on calls until `ruxen_clear_cancel()`.

**Feature #1 (§4.1) is COMPLETE** across Increments 1–3.

---

## Feature #2 — Structured task `Scope` (spec §4.2)

The keystone: a `Scope` value that OWNS its spawned children and whose `def drop`
joins-then-cancels every still-pending child before control leaves the block, so
a detached/leaked async task is unrepresentable. Built as a `.rx` class over the
thread-local scheduler (`ruxen_executor_spawn`), the `Future` mixin, and the §4.1
drop machinery — plus ONE small additive scheduler primitive (per-handle cancel).

### Surface decision (honest — what the current lowering supports)

The spec sketch is `Scope.open do |s| ... s.spawn { || fetch(u).await } ... s.collect.await ... end`.
Two pieces of that sketch are NOT supported by the current lowering, verified
against source this session:
- **async closures** — `s.spawn { || fetch(u).await }` needs a closure body
  containing `.await` lowered into a spawnable future. `async_lowering/lower.rs:2486`
  treats closure bodies as "a separate async scope — opaque to this scan": async
  closures are NOT lowered into state machines. So **`s.spawn` takes an already-
  constructed FUTURE VALUE** — `s.spawn(fetch(u))`, where `fetch` is a top-level
  `async def` (which IS lowered to a wrapper returning its `__*Future`). This is
  the `Task.spawn_raw(future_ptr)` precedent.
- **`s.collect.await` as an async method** — async methods on classes are
  unlowered (only top-level free fns). So **`collect` is a SYNCHRONOUS method that
  pumps the thread-local scheduler** (`ruxen_executor_pump_tasks()` in a loop)
  until every child handle reports `TaskHandle.is_done` — the same hand-pump the
  v1 728 spawn-join surface uses, no `.await` needed. `Scope.open` is therefore
  callable from SYNC code (it drives async internally, like `block_on`).

So the v1 surface is:
```ruxen
Scope.open do |s|        # s: &var Scope, yielded by Scope.open (immediately-invoked do…end)
  s.spawn(work_a())      # work_a / work_b are top-level `async def`s → future values
  s.spawn(work_b())
  s.collect              # SYNC pump: drive all children to completion (fail-fast)
end                      # <- Scope#drop: cancel any still-pending child (fires its def drop)
```

### Per-scope cancel — the one new runtime primitive

`drain_remaining` (§4.1) drains the ENTIRE thread-local queue — wrong for a Scope
that must cancel only ITS OWN children (nested scopes / mixed block_on roots must
be untouched). The scheduler has per-handle `is_done`/`result`/`drop_handle`
(refcount) but NO per-handle cancel. Add ONE additive C primitive:
`ruxen_executor_cancel_task(handle)` — find the queue entry whose `handle ==`
the arg, fire `Drop_dynamic_drop(future_ptr)` on it (the §4.1 path), unlink + free
the entry, and mark the handle `done` so a dangling join resolves. `Scope#drop`
calls it per still-pending child. This is the only runtime addition; everything
else is `.rx`.

### Raw-`Int` handle wall-off (spec §4.2 risk 4)

`s.spawn` records the child's raw `Int` handle in the Scope's private
`Array[Int]` field and returns `nil` (NOT the handle) — so a child handle cannot
escape the scope to be copied out / joined out-of-band, which would break the
ownership/borrow guarantee. The scope is the sole holder of its children's
handles; `collect`/`drop` are the only ways to observe/cancel them.

### Increments (TDD)

> **Pin-numbering scheme.** Feature #1 occupies `391`–`398` (committed). Cases
> `399`–`411` are contended (`401`–`411` are pre-existing `*_array_*` fixtures),
> so all Feature #2+ concurrency pins live in the free **`412`–`499`** block.
> Feature #2 (Scope) = `412`–`416`; later features continue from `417`.

- **Inc 1 — spawn + collect happy path.** `Scope.open do |s| s.spawn(a()); s.spawn(b()); s.collect end`
  drives both children to completion; observable side effects (each child prints)
  prove both ran. Pin `412`. (Establishes the class, the `Array[Int]` child list,
  the sync pump-collect, the raw-Int wall-off.)
- **Inc 2 — Scope#drop cancels a still-pending child.** A child that never
  completes (Pending forever) is cancelled by `Scope#drop` when the block exits
  WITHOUT collecting it (or collect stops early) → its `def drop` fires via
  `ruxen_executor_cancel_task` → `Drop_dynamic_drop`. Pin `413` (observable child
  drop). Adds the scheduler `cancel_task` primitive.
- **Inc 3 — fail-fast vs supervised.** `Scope.open` (fail-fast): a child error
  cancels siblings + propagates; `Scope.supervised`: children fail independently,
  errors aggregated. v1 error channel: children return an `Int` status (the raw
  scheduler result is `Int`); fail-fast stops the collect loop + cancels remaining
  on the first error sentinel. Pin `414`/`415`. (If the error-encoding proves to
  need typed results — `TaskJoinFuture.Output` is pinned to `Int` — scope v1 uses
  the Int-sentinel convention and documents it.)

Each increment: failing pin first → minimal `.rx` (+ the one C primitive in Inc 2)
→ narrow tests + re-confirm 391–398 → Codex review (park LOW).

### AS-BUILT (all increments GREEN, pins `412`–`416`)

- **`Scope.open` uses an EXPLICIT `cancel_remaining` join-fence after the block,
  NOT `s`'s scope-exit `def drop`.** The `yield(s)` passes `s` to the block by
  value at the closure-call ABI, which the drop analysis treats as a consume, so
  `s` is not auto-drop-elaborated in `open` (same class as the Increment-3
  block_on-fut issue, but here resolved by an explicit cleanup call rather than a
  force-root). `def drop` remains as a safety net.
- **`collect` PARKS on the reactor** (`Scope.pump_or_park` mirrors block_on's
  pending arm: pump; if `pumped==0 && !ready`, `Thread.yield_now`) — without this
  a timer/fd-backed child busy-spins. Pin `416` (Codex high finding).
- **`ruxen_executor_cancel_task` is reentrancy-safe**: unlink the entry + mark the
  handle done BEFORE running the child's `def drop`, so a `def drop` that re-enters
  the scheduler can't double-free the entry (Codex blocker finding).
- **Fail-fast/supervised are two collect METHODS** (`collect` / `collect_supervised`)
  the user chooses, not two `Scope.open` entry variants — the entry-level
  `Scope.supervised` sugar is deferred (would need a mode flag); the method choice
  is the v1 surface.
- **FOLLOW-UPS:** (a) `s.spawn(async_fn())` where the arg is an async-def WRAPPER
  call fails to LINK (`_<fn>` undefined symbol) — the async-fn wrapper isn't
  emitted/linked for the spawn-arg path; v1 spawns a direct stdlib future or a
  hand-written `include Future` value. (b) typed `Result[T,E]` child channel awaits
  typed `TaskJoinFuture.Output` (pinned to `Int` today). (c) entry-level
  `Scope.supervised` sugar. (d) the stale-`libruxenrt.a` linkage note: adding a new
  C runtime symbol referenced by a bootstrap stdlib class (emitted into every
  program) requires a clean rebuild of all `target/*/libruxenrt.a` archives.

**Feature #2 (§4.2) is COMPLETE** across Increments 1–3 (+ the two Codex fixes).

---

## Feature #3 — General `Race` (spec §4.3)

First future to complete wins; **all losers are deterministically cancelled (their
`def drop` runs) before the winner's value is returned**, reusing the §4.1
`Drop_dynamic_drop` path. No new reactor primitive — the reactor already
multiplexes multiple registrations on one `park_current`, so two arms both
registered (e.g. a read fd + a timer) both wake the same poll.

### Ground truth (verified — Explore over scheduler.c + the precedent)
- `Future_dynamic_poll(self, cx)`, `Drop_dynamic_drop(self)`, `ruxen_poll_is_ready`
  are **C-only** symbols (not `.rx`-callable). `cx` is the raw `RuxenContext*` at
  the i64 ABI — pass straight into `Future_dynamic_poll(child, cx)`. Poll heap
  layout: tag@0 (Ready=0, Pending=1), payload@8.
- A future VALUE flows into an `Int` param as its heap pointer (the
  `Task.spawn_raw(fut: Int)` / `Scope.spawn` precedent).
- `AsyncReadWithTimeoutFuture` is the proven 2-way race: a hand-written `.rx`
  future driving a C step machine, first-Ready wins, loser resource torn down on
  the win path + in `drop`.

### Design (C-helper-backed `RaceFuture`, mirroring AsyncReadWithTimeoutFuture)
- `Race.of(a, b)` takes two future VALUES as `Int` (heap pointers) and returns a
  `RaceFuture` (hand-written, `include Future`). The poll logic — polling each arm
  via `Future_dynamic_poll`, inspecting the `Poll`, firing the loser's
  `Drop_dynamic_drop`, freeing the loser's heap block — lives in a C step machine
  (`ruxen_race_*`, like the read-timeout state). The `.rx` `poll` calls the step,
  and on a win builds the `Poll.Ready(Race2.<arm>(payload))` enum (enum
  construction stays in `.rx` so the compiler owns the layout).
- **Result encoding (chosen): `enum Race2 First(Int) Second(Int)`** — the winner's
  i64 payload, tagged by which arm won. v1 Int channel (same limit as
  `TaskJoinFuture.Output = Int`; typed-generic-Output deferred). The user
  `match`es `First`/`Second` and casts the Int to the arm's type. Mirrors the
  spec sketch's `Race.First(b)`/`Race.Second(_)`, Int-typed. Documented.
- **Loser-cancel fires `def drop` via the §4.1 path:** the C step, on first-Ready,
  calls `Drop_dynamic_drop(loser_ptr)` (its concrete `def drop` — reactor
  deregister / fd close) + frees the loser block, BEFORE returning the winner.
  `RaceFuture#drop` (dropped un-resolved, e.g. cancelled by an outer Scope) drops
  BOTH still-live arms. Reentrancy-safe by the same discipline as
  `cancel_task` (don't hold a reachable reference across the user drop).

### Increments (TDD, pins 417+)
- **Inc 1 — `Race.of(a, b)` 2-arm, winner returned.** Two arms; one resolves first
  (Ready), the other is Pending; `Race.of(a,b).await` (or hand-poll) returns
  `Race2.First`/`Second` with the winner's payload. Pin `417`. Establishes the
  `RaceFuture` class, the C step, the `Race2` enum, the Int-pointer ABI.
- **Inc 2 — loser-cancel runs the loser's `def drop`.** The losing arm has an
  OBSERVABLE `def drop`; when the winner resolves, the loser's drop fires EXACTLY
  ONCE (before the winner's value is observed). Pin `418`. Plus a `RaceFuture#drop`
  pin (`419`): a Race dropped before either arm resolves drops BOTH arms.
- **Inc 3 — `Race.any(arr)` N-way.** Poll a runtime-length `Array[Int]` of arms;
  first Ready wins, all other arms dropped. Result is `Won(idx, payload)` or the
  `Race2`-style tag generalized. Pin `420`. (Spec: ship `Race.of` first, `Race.any`
  second — the N-way array-of-futures storage + per-element dispatch.)

Each increment: failing pin first (417+) → minimal `.rx` + C step → narrow tests
+ re-confirm 391–398 & 412–416 → Codex review (park LOW).

### AS-BUILT (all increments GREEN, pins 417–422)
- **`Race.any` → `Race.any_of`** — `any` is a reserved keyword (the `any Mixin`
  type position), so the N-way constructor is `any_of`.
- **Winner block is LEAKED, not dropped** (Codex blocker fix): the winner
  returned Ready, MOVING its Output out (the i64 payload may be a heap String/
  class pointer the caller now owns); running the winner's `def drop` would free
  that moved-out result → UAF. So only LOSERS are dropped+freed; the winner block
  leaks, matching the v1 scheduler discipline (completed future blocks leak; only
  the payload is consumed). Tradeoff: a reactor-backed winner's own fd/timer
  isn't torn down by Race — but a winner that returned Ready should have released
  its own resources on its Ready path. Pin `422` (winner NOT dropped, only loser).
- **Reentrancy-safe** (Codex high fix): the N-way step/free snapshot all arm
  pointers + zero every slot BEFORE any user `def drop`, and the any-state carries
  a `busy`/`free_pending` guard so a re-entrant `state_free` during a loser's drop
  defers rather than freeing the state mid-step. Snapshot mallocs null-checked.
- **Empty `Race.any_of` panics** (Codex medium fix) — a no-arm race never resolves.
- **Move-by-value arm contract** (Codex blocker, documentation-only — same
  non-enforceable raw-`Int` gap as `Task.spawn_raw`/`Scope.spawn`): Race owns +
  frees its arms; don't reuse/retain an arm value, nor pass one already spawned.
  Needs typed move-only future params (a shared v1 lowering gap).
- **Pins:** `417` first-arm win, `418` second-arm win, `419` loser-cancel drop,
  `420` RaceFuture#drop both arms (via a `Scope` cancel — composes §4.2+§4.3),
  `421` N-way `any_of`, `422` winner-not-dropped.
- **Known v1 limitation:** the result payload is an `Int`; a heap Output (String/
  class) survives (winner leaked) but `p as String` prints the raw pointer — no
  typed re-hydration (the Int-channel limit). Typed-generic Output is deferred.

**Feature #3 (§4.3) is COMPLETE** across Increments 1–3 (+ the Codex fixes).

---

## Feature #4 — Async backpressured `Queue` (spec §4.4)

A bounded, reactor-aware queue split into `AsyncSender`/`AsyncReceiver` whose
`send`/`recv` PARK THE TASK via the reactor (never block the OS thread — the
fatal flaw of the sync MPSC `Receiver.recv`). Built as `library/std/future/src/
queue.rx` (hand-written `SendFuture`/`RecvFuture`, the `AsyncReadWithTimeoutFuture`
shape) over a new C control block + step machines in `runtime/queue.c`. Reuses
the `Future` mixin, the reactor wake path (`ruxen_waker_wake` via the parked op's
`cx` waker), and the `SharedSync`-style refcount discipline. NO new reactor
primitive.

### Decisions (v1 defaults, from the brief — taken, not re-litigated)
- `Queue.bounded(capacity, overflow)` → split ends; capacity MANDATORY, NO
  unbounded constructor (Go). `enum Overflow { Suspend; DropOldest; DropNewest }`
  (ZIO taxonomy). Int element/error channel (`Result[Int, QueueError]`), the
  established Task/Race/Scope convention. SINGLE-REACTOR v1 (cross-core wake is
  §4.4 risk 3 / §6, the largest unknown — deferred to v2). Leak-free close by
  ownership (last-sender-drop → recv `Err(QueueClosed)`; receiver-drop → parked
  Suspend-send `Err`).

### Surface deviations forced by the current lowering (verified this session)
- **`let (tx, rx) = Queue.bounded(...)` (spec sketch) does NOT work** — class-typed
  tuple destructuring leaves the bindings unresolved `Infer` (`?T_send`); `(Int,
  Int)` tuples destructure fine, class-typed elements don't. So `bounded` returns
  a **`QueueEnds` handle**; pull `sender`/`receiver` off it (single-class returns
  resolve). Typeck class-tuple gap = a v1 follow-up.
- **`.await` on the no-arg `recv`** must be spelled `recv()` (empty-paren →
  `MethodCall` awaitee Shape-3; a paren-less `rx.recv` parses as `FieldAccess`,
  which `describe_awaitee` doesn't desugar).
- **Await segments must be simple `let`s** (the accepted linear-N CFG shape) — no
  `match`-with-mutation between awaits. The consumer binds each recv then does the
  matching/summing in the terminal segment.
- **SPAWNED multi-await futures don't self-reschedule:** the linear-N state
  machine returns `Poll.Pending` on intermediate-state advance WITHOUT re-waking
  its own task, and the pump gates on the task's ready bit → a spawned multi-await
  producer strands after its first await. A `block_on` ROOT is re-polled every
  loop iteration regardless, so a multi-await ROOT works. Pins therefore use
  single-send producer TASKS + a multi-recv consumer ROOT. (Pre-existing
  async-lowering limit; self-wake-on-advance is the systemic fix, deferred.)
- **A stdlib-class END local from a method/constructor result is tainted out of
  drop-elaboration** (`alloc_rooted`), so its `def drop` never fires at a
  mid-program point — the SAME conservative-dealloc limit `Scope` worked around
  with `cancel_remaining`. So the close pins drive the close CODEPATH directly
  (`ruxen_queue_{sender,receiver}_drop`, the exact C logic `def drop` runs); the
  automatic last-`def drop`-closes-it trigger is a drop-rooting follow-up.

### ABI notes
- The `Overflow` no-payload enum crosses to C as a HEAP POINTER whose int32 tag
  sits at offset 0 (the `Shutdown`/`ruxen_tcp_stream_shutdown` precedent — NOT a
  bare i64). `ruxen_queue_new` reads it; unknown/null tag panics. Tag order pinned
  to `RUXEN_OVERFLOW_*` (Suspend=0/DropOldest=1/DropNewest=2). *(Was a latent bug
  — Inc1 stored `(int)overflow` directly; only surfaced in Inc2 when the overflow
  branch was first reached. Fixed.)*
- The END `def drop`s are FFI-ALIAS (`ruxen_async_{sender,receiver}_end_drop`),
  not `.rx` bodies — a bootstrap-class `.rx`-body drop isn't elaborated for a
  local; the FFI-alias form is (the `Sender`/`File` precedent). The wrapper reads
  the control-block ptr from the END heap block (`{ int64 queue_ptr }` offset 0;
  the ends don't `include Future`, so no class_info header).

### Increments (TDD, pins 423+ — the free concurrency block)
- **Inc 1 — bounded enqueue/dequeue + `Suspend` backpressure.** Pin `423`
  (3 single-send producer tasks via `share` + parked multi-recv consumer root,
  sum=60); pin `424` (cap-1 full-queue send parks the producer, a dequeue wakes
  it, the 2nd send resolves Ok). Establishes the control block, ring buffer, both
  futures, the `Result[Int, QueueError]` channel, the park/wake wiring.
- **Inc 2 — DropOldest / DropNewest.** Pin `425` (sliding: cap-2, send 1..4 →
  recv 3,4 = 7); pin `426` (dropping: send 1..4 → recv 1,2 = 3). Drove out the
  overflow-tag-from-pointer ABI bug.
- **Inc 3 — close→Err.** Pin `427` (send-closed → recv `Err(QueueClosed)`);
  pin `428` (recv-closed → send `Err(QueueClosed)`). Scoped to the close→Err
  CODEPATH (not full leak-free cleanup — the surviving end's ref isn't released,
  the field-drop follow-up).

### AS-BUILT (all increments GREEN, pins `423`–`428`)
- **Wake routing:** a parked op records `ruxen_context_waker(cx)` (the `cx->waker`
  pointer, offset 0 of `RuxenContext`) in the queue's single recv/send wake slot;
  the peer's enqueue/dequeue/close calls `ruxen_waker_wake(slot)` (marks the task
  ready + pokes the reactor wake fd — the exact path spawned-task wakeups use). A
  `block_on` ROOT's waker has `task == NULL`, so it relies on the loop's
  unconditional re-poll rather than `wake_task`; a spawned task gets the precise
  `wake_task`.
- **Codex (rule 44) fixes applied:** Inc1 — (blocker) cancel-while-parked waker
  UAF: the op future captures its registered waker (`ruxen_queue_{recv,send}_waker`)
  and clears the slot in `def drop` via `ruxen_queue_cancel_{recv,send}` (`==`
  match-guarded); (high) `QueueEnds` duplicate-end UAF: `sender`/`receiver` are
  consuming (`def var`, zero the founding ptr, panic on re-take) + `QueueEnds.drop`
  releases untaken refs; (medium) capacity overflow guard. Inc2 — (medium) unknown/
  null `Overflow` tag panics rather than silently defaulting to Suspend (matching
  the `Shutdown` precedent) + stale "bare i64 tag" ABI comments corrected. Inc3 —
  (medium, verified) the close pins prove the close→Err codepath but NOT full
  leak-free ownership (the surviving end's ref isn't released by the synthesized
  future drop, which drops only `__sub_i` sub-futures) — claims tightened in the
  pin headers + this plan, not overstated.
- **Parked LOW for the orchestrator:** add an `Overflow` tag-stability pin test
  (mirroring `compiler/ruxen_core/tests/shutdown_tag_stability.rs`) — `Overflow`
  has no equivalent source-order pin; the e2e pins exercise each tag end-to-end
  but a dedicated pin would catch ABI drift directly. (Codex Inc2 low.)
- **FOLLOW-UPS (v2 / cross-cutting, not Feature #4 bugs):** (a) cross-core wake
  routing (the §4.4 risk-3 unknown) + atomic refcounts. (b) a waiter LIST for
  multi-producer backpressure (single `send_waker` strands a 2nd blocking sender)
  and multi-consumer recv. (c) spawned-multi-await self-reschedule (self-wake on
  state advance) — would let a spawned producer do >1 await. (d) drop-rooting for
  stdlib-class method-result locals — would make the automatic ownership-driven
  close fire (and `QueueEnds`/end locals free their refs). (e) class-typed tuple
  destructuring → restore the `let (tx, rx) = Queue.bounded(...)` sugar. (f) typed
  `Queue[T]` element channel (currently Int, like `TaskJoinFuture.Output`).

**Feature #4 (§4.4) is COMPLETE** across Increments 1–3 (+ the Codex fixes),
scoped to the single-reactor v1 the brief specified.

---

## Feature #5 — Async `Stream` (spec §4.5)

A pull-based, backpressured async stream — the async analogue of `Enumerable[T]`.
`library/std/future/src/stream.rx` (`mixin Stream` + 3 adapters) + a tiny
`runtime/stream.c` (resource helper for the drop-on-cancel pin). Reuses the
`Future`/`Enumerable` patterns directly — this feature was smooth because the
two existing patterns translated 1:1.

### Decisions (v1 defaults, from the brief — taken, not re-litigated)
- `mixin Stream dispatch runtime` over ONE required `def var poll_next(cx) ->
  Poll[Option[Int]]`, with `for_each`/`count`/`sum`/`map`/`filter`/`take` as
  default bodies / adapter classes built on `poll_next` (the `Enumerable`-over-
  `each` reuse). Int channel (v1). Single-consumer pull; leak-free cancel via
  `def drop`. NOT in scope: concurrent stages (`mapPar`/`merge`), reactive-
  streams interop (v2).

### SPIKE result (make-or-break, confirmed FIRST)
A user-defined `mixin Stream dispatch runtime` with a required `poll_next` + a
default-body `for_each` (sync pump) LOWERS and RUNS, with `poll_next`
runtime-dispatched. This is the FIRST `dispatch runtime` mixin that also carries
default-body methods — `Future` has only the required method, `Enumerable` has
defaults but isn't `dispatch runtime`. The combination works.

### Surface as built (what the current lowering supports)
- **`for_each` is a SYNC pump driver** (builds a `Context`, pumps `poll_next`,
  parks on `Pending` via `Scope.pump_or_park`, stops on `Ready(nil)`) — NOT an
  `async def`, NOT `.await`-in-loop (both gated). The v1 consumer driver is
  `stream.for_each do |x| … end` from SYNC code, NOT the spec's `for x in
  stream` (method-level async lowering — v2).
- **Adapters constructed DIRECTLY** (`MapStream.new(src, f)`), not fluent
  `src.map(f)` — moving `self` out of a `&var self` method body isn't supported
  (the explicit `(self)` consume is `lib`-block-only). The v1 surface takes the
  inner stream as a constructor arg by value (the `Race`/`Queue` value-construct
  shape); fluent `.map`/`.filter`/`.take` is a v2 by-move-self follow-up.
- **Adapters hold the inner as `any Stream`** (dyn-mixin) and poll it via the
  runtime vtable (`(&var self.inner).poll_next(cx)`) — verified via a spike that
  the dyn field + dyn dispatch lowers.
- **`.await` of a no-arg method needs explicit parens** carried over from F4
  (not relevant to Stream's sync driver, but the same lowering floor).

### Increments (TDD, pins 429+)
- **Inc 1 — `mixin Stream` + source `poll_next` + drain drivers.** Pin `429`
  (Counter source, `for_each`/`count`/`sum`); pin `430` (task-backed source over
  a Queue + spawned producer — proves `for_each` PUMPS on Pending, would spin
  without it).
- **Inc 2 — `map`/`filter`/`take` adapters.** Pins `431` (map), `432` (filter),
  `433` (take + composed `map→filter→take` chain, take stops the chain early),
  `435` (adapter FUSE — re-polling a finished adapter stays `nil`).
- **Inc 3 — leak-free drop-on-cancel mid-stream.** Pin `434` (a resource-owning
  source partially consumed via a borrow-driver, then scope-exit drop fires the
  source `def drop` exactly once).
- **Inc 4 — leak-free adapter CHAINS** (the spec's headline guarantee: "dropping
  the chain drops every stage"). Widen the §4.1 narrow Drop-vtable gate
  (`mir/lower/mod.rs`) from `Future`-only to `Future OR Stream` so a `Stream`
  class gets a Drop vtable slot at the uniform slot 1; each adapter's `def drop`
  runs its inner `any Stream`'s `def drop` via `ruxen_stream_drop_inner` →
  `Drop_dynamic_drop(inner)` + free. Pin `436` (a `TakeStream(MapStream(Resource
  Source))` chain partially consumed then dropped → source `def drop` fires
  EXACTLY once, a live-count guard reads 0 — no leak, no double-drop) + a layout
  pin (`stream_class_with_drop_gets_drop_vtable_slot`).

### AS-BUILT (all increments GREEN, pins `429`–`436`)
- **Codex (rule 44) fixes:** Inc1 — (high) `for_each` Pending arm now uses
  `Scope.pump_or_park` (was a bare `Thread.yield_now`), so a TASK-backed source
  makes progress instead of spinning (proven by pin 430); (high) the misleading
  explicit `cx.drop` was removed (it never fired — `cx.drop` didn't reach
  `ruxen_executor_context_drop` — and masked a bounded per-drive `Context` leak,
  now documented). Inc2 — (medium) adapters are now FUSED (a `done` latch: once
  the inner returns `nil`, later `poll_next`s return `nil` without re-polling, so
  a non-fused inner can't leak a value past end; proven by pin 435). Inc4 (the
  gate widening — an ABI/layout change) — Codex VALIDATED no double-drop
  (moved-in sources/adapters are tainted out of scope-exit drop), the
  `__weak Drop_dynamic_drop` guard, and no non-async layout regression; one
  verified low (a stale C comment) fixed.
- **Inc4 slot-uniformity — verified sound:** a Future class and a Stream class
  each have exactly ONE real runtime-dispatch mixin, so
  `drop_vtable_slot_for_class` (returns `runtime_dispatch_includes.len()`) yields
  1 for both → Drop at the uniform slot 1 → the singleton-slot assert holds → ONE
  `Drop_dynamic_drop` helper serves both (pin
  `stream_class_with_drop_gets_drop_vtable_slot`). The change is one site
  (`is_future` → `is_async_protocol`) + the C helper + adapter drops + the layout
  pin — no broad ABI work, no destabilization (33 e2e + 20 mixin_vtables pins
  green).
- **Inc4 SHARP EDGE (pre-existing, REPORTED to the maintainer, NOT fixed here):**
  a class that `include`s BOTH `Future` AND `Stream` (semantically incoherent — a
  value can't be both a one-shot future and a pull stream) has 2 runtime-dispatch
  mixins → Drop@2, diverging from the uniform slot 1 → the hard slot-uniformity
  `assert!` PANICS (a LOUD compiler crash, NOT a silent miscompile). This panic
  PRE-DATES Inc4 (the §4.1 assert already covered `Future + <2nd dispatch-runtime
  mixin>`; before Stream there was no 2nd stdlib `dispatch runtime` mixin to
  combine). Inc4 makes `Future + Stream` a newly-reachable instance. A proper
  diagnostic (reject the combination with an error code, or per-slot
  `Drop_dynamic_drop` helpers) is a separate maintainer call — out of Inc4's
  "leak-free chains" scope (a new `E####` + `docs/errors/`).
- **Pre-existing compiler limits hit + documented (NOT Stream bugs):**
  (a) a stdlib-class / source local consumed via a `&var self` mixin method
  (`src.for_each`) is tainted out of drop-elaboration → its scope-exit drop is
  suppressed; the borrow-driver path (`&var stream` to a free fn) drops correctly
  (pin 434). Same drop-rooting limit Scope/Queue hit. (b) A no-arg-`def init`
  source class `.new` (paren-less) CRASHES when wrapped in an adapter (a separate
  constructor-lowering quirk — worked around with an arg-init source; filed as
  **Q46** in `gui-stack-v1-issues.md`).
- **FOLLOW-UPS (v2):** fluent `.map`/`.filter`/`.take` (by-move-self); the
  ergonomic `for x in stream` driver (method-level async lowering); typed
  `Stream[T]` element channel (currently Int); concurrent stages
  (`mapPar`/`merge`); the for_each-then-drop drop-rooting fix; a proper diagnostic
  (or per-slot helpers) for the `Future + Stream` incoherent-include panic.

**Feature #5 (§4.5) is COMPLETE** across Increments 1–4 (+ the Codex fixes),
scoped to single-consumer pull streams with leak-free cancel AND leak-free
adapter chains.

---

## How §4.2–§4.7 sequence on top (per spec §6)

- **§4.2 Scope** (keystone): hard-depends on §4.1 (Scope#drop joins-then-cancels
  children by firing each child future's `def drop` via `Drop_dynamic_drop`). Also
  gated on the **async-methods-on-classes** open question (§6) —
  `s.collect.await` would be an async method; v1 ships a free-fn/hand-written-future
  `collect` factory (the `Async.sleep`/`Task.join` precedent). Built as `.rx` over
  the thread-local scheduler; no new runtime.
- **§4.3 Race.of** (2-arm first, then `Race.any`): cheap once §4.1 lands —
  loser-cancel runs each loser's `Drop_dynamic_drop`. Reuses `Future_dynamic_poll`
  + the raw-`Int` child-pointer ABI. Concrete-arm `.await` already resolves
  (`731_class_static_call_await` is a passing pin per spec §4.3 correction);
  generic-`Output` Race inherits `TaskJoinFuture`'s concrete-Output limit.
- **§4.4 Queue** + **§4.5 Stream** (parallelizable): Queue's multi-core cross-thread
  wake is the largest genuine unknown (spec §4.4 risk 3 / §6 open question — likely
  single-reactor v1). Stream's ergonomic `for x in stream` is gated on
  async-methods-on-classes. Both reuse the per-op future shape + reactor.
- **§4.7 TestClock**: independent of compiler work (C-runtime + one `.rx` class),
  can land early to make §4.6 testable; C scope is larger than "one function" —
  must also virtualize `ruxen_instant_now`/`elapsed` (spec §4.7 correction).
- **§4.6 Retry/Schedule** (last): pure `.rx` over closures/`Duration`/`Async.sleep`;
  best validated with §4.7 in place.

**Open questions a maintainer must still settle (spec §6):** async-methods-on-classes
(gates §4.2/§4.4/§4.5 ergonomics), `segment_cfg` widening for `.await`-in-loop
shapes, raw-`Int` handle ABI wall-off inside Scope/Race, cross-thread reactor wake,
Scope fail-fast-vs-supervised default, and whether state-indexed liveness lives in
async-lowering or extends drop-elaboration (`docs/decisions/drop-elaboration.md`).
