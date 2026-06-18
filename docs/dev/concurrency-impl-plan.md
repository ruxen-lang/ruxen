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
