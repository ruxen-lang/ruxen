# Spec — `async def` + `.await` lowering (async sub-phase 2)

**Source docs:**
[docs/requirements/tier1_03_async.md](../../requirements/tier1_03_async.md) §4.4,
[docs/prompts/v1/15_phase4_async.md](../../prompts/v1/15_phase4_async.md),
[docs/specs/stdlib/async.spec.md](../stdlib/async.spec.md) (sub-phase 1 — surface).

**Status:** sub-phase 2 of the async round. Sub-phase 1 (commit
`b513e90`) shipped the stdlib surface — `Future` mixin, `Poll[T]`
enum, `Context`/`Waker` shells. This sub-phase wires the **lowering**
so `async def foo` and `expr.await` produce real state machines that
implement Future. No executor yet (sub-phase 3) — but the lowered
code can be poll-driven by hand-written test harnesses to validate
shape.

Two staged milestones inside this sub-phase:
- **Milestone 2A** (B1–B6): `async def` with NO `.await` lowers to a
  trivial single-state machine that goes `Ready(value)` on first
  poll. Smallest possible state machine — derisks the layout work.
- **Milestone 2B** (B7–B14): `.await` introduces suspension points.
  Each `.await` becomes a state. Locals live across suspends are
  captured as state-machine fields.

Land 2A first; commit; then 2B.

---

## Milestone 2A — `async def` with no `.await`

### B1 — `async def foo() -> T` typechecks as returning `some Future[Output = T]`

```rvn
async def make_int() -> Int
  42
end
```

**Given** the source above
**Then** typeck reports the function's signature as
`def make_int() -> some Future[Output = Int]`. The `async` modifier
on the parser-level `FuncDef.is_async` flag is read by typeck during
signature inference; the declared return type `Int` is wrapped into
the existential return `some Future[Output = Int]`.

Calling `make_int()` returns a Future-implementing value. The
non-async caller can poll it manually:

```rvn
def main
  let mut fut = make_int()
  # Hand-written Context for testing — see B6
  let ctx = Context.test_dummy
  match (&var fut).poll(&var ctx)
    Poll.Ready(v) -> puts "#{v}"
    Poll.Pending  -> puts "pending"
  end
end
```

For Milestone 2A this should print `42`.

### B2 — Each `async def` generates an anonymous state-machine class

The lowering pass synthesizes one class per `async def`:

```rvn
# Pseudocode for what the compiler generates for `async def make_int() -> Int  42 end`:
class __MakeIntFuture
  __state: Int            # 0 = start, 1 = completed
  __result: Int           # the captured return value once Ready

  include Future
  type Output = Int

  def var poll(cx: &var Context) -> Poll[Int]
    match self.__state
      0 ->
        self.__state = 1
        Poll.Ready(42)
      _ ->
        # poll-after-complete: behaviour TBD by spec B5
        Poll.Pending
    end
  end
end
```

The class name is compiler-mangled (`__<FnName>Future` or similar —
not user-addressable). It includes `Future` and binds the associated
`Output = T`. The `__state` field is an i64 tag.

### B3 — Caller invocation: `async def foo()` is rewritten as `__FooFuture.new()`

When the user writes `make_int()` after the lowering, MIR emits a
constructor call to the synthesized state-machine class with the
initial state field set to 0 and arg fields populated from the
call site.

Function arguments (if any) become fields on the state-machine
struct:

```rvn
async def add(a: Int, b: Int) -> Int
  a + b
end
# →
class __AddFuture
  __state: Int
  a: Int        # captured at construction
  b: Int        # captured at construction
  ...
end
```

### B4 — Local variables that don't live across an await stay as MIR locals

In Milestone 2A there ARE no awaits, so EVERY local could be either
inline or a struct field — pick the cheaper option (inline). The
spec deliberately doesn't pin where 2A locals live; the constraint
is only that the observable behaviour matches a regular synchronous
function's local lifetime.

### B5 — Poll-after-Ready behaviour: panics with E1114 or returns Pending forever

After the future returns `Ready(v)` once, subsequent polls have
undefined-but-bounded behaviour. The spec REQUIRES one of:
1. **Panic** with diagnostic E1114 ("future polled after completion").
2. **Return `Pending` forever** — non-progressing but safe.

Pick (2) for v1 to keep the lowering simple. Document the choice
in the milestone 2A commit. (Rust's std picks "panic" but with
`std::future::Fuse` for safe wrapping; we defer that ergonomics
to v2.)

### B6 — Test harness: `Context.test_dummy` (#[cfg(test)] only)

Add a `Context.test_dummy` static method (in `library/std/future/src/lib.rvn`)
returning a `Context` whose `Waker` is a no-op stub. Lives behind a
`# Test-only — see context.spec.md` comment. The hand-driven poll
loop in B1 uses it; no executor needed.

Implementation: a static method that returns a heap-allocated
`RivenContext` whose waker is a function pointer to a `riven_waker_noop`
that does nothing. Wired in `library/std/future/runtime/executor.c`
(stubbed in sub-phase 1; this milestone replaces those stubs for
`test_dummy` only — real executor still pending sub-phase 3).

---

## Milestone 2B — `.await` suspension points

### B7 — `.await` desugars to a poll loop with a `return Pending` on Pending

```rvn
async def f() -> Int
  let x = g().await
  x + 1
end
```

The compiler rewrites the body to a state machine with TWO states:
state 0 = "before awaiting g", state 1 = "after .await, computing x + 1".

Pseudo-MIR for the generated poll:
```
match self.__state
  0 ->
    # Construct g()'s future, store in self.__sub
    self.__sub = g()
    self.__state = 1
    # Fall through into state 1 (poll immediately — allows
    # eager completion to skip a redundant park/wake roundtrip)
  1 ->
    # Continue: poll the sub-future
  _ -> unreachable
end
match (&var self.__sub).poll(cx)
  Poll.Pending  -> return Poll.Pending
  Poll.Ready(v) ->
    # v is `x`; lower the rest of the body
    Poll.Ready(v + 1)
end
```

The sub-future field (`__sub`) is typed at the static return type
of the awaited expression. A function with N `.await` calls
generates `__sub_0` … `__sub_{N-1}` fields (one per call site, not
one global — each await may produce a different Future type).

### B8 — Locals live across `.await` are captured as state-machine fields

```rvn
async def add_async() -> Int
  let a = compute().await
  let b = compute_other().await
  a + b
end
```

`a` is needed in state 2 (after the second .await) but defined in
state 1 (after the first). The lowering pass identifies `a` as
"live across the suspend at line 3" and promotes it to a field
`a: Int` on the state-machine class.

Locals NOT crossing a suspend stay as MIR locals.

### B9 — State transitions are explicit branch arms in `poll`

Each suspend point produces a `state` increment. The `poll` method
is a single big match-on-state with one arm per state. Branches
within a state stay as block expressions; suspends are state
transitions.

### B10 — Multiple `.await` chain typechecks and lowers

The B7 example with two `.await` calls. State machine has 3 states
(0 = start, 1 = mid, 2 = done — though state 2 is folded into
"return Ready" rather than stored). Verify by inspecting the
generated MIR.

### B11 — `.await` inside `if` / `match` / `while` works (DEFERRED in v1)

> **Deferred to follow-up.** Milestone 2B v1 supports only straight-
> line `.await` (each `.await` is the RHS of a top-level
> `let x = call().await` statement in an async fn body). Each
> per-branch continuation needing its own state-id allocation, and
> the live-set analysis for locals introduced inside a branch but
> used after the merge, is its own slice. The test
> `await_in_if_match_branches_lower` in `tests/async_lowering.rs`
> is `#[ignore]`d with a reason pointer.



```rvn
async def conditional() -> Int
  if check_condition()
    g().await
  else
    h().await
  end
end
```

Both branches of `if` may suspend. The lowering creates separate
states for each branch's continuation. Same shape for `match`.
`while` is more complex (state per loop iteration — defer to v2).

For Milestone 2B, support `if` and `match` only. `while`/`for` with
`.await` in the body returns E1115 ("await in loops not yet
supported") with a TODO referencing the v2 prompt. Document this
limit clearly.

### B12 — `.await` outside async context rejected (E1110)

```rvn
def main
  some_future.await    # E1110
end
```

**Then** the resolver (or typeck — wherever's natural) emits:
```
[E1110] error: `.await` is only valid inside `async def` or `async { }`
   |   some_future.await
   |               ^^^^^
note: wrap the call in `async { ... }` or add `async` to the enclosing function
```

The check fires at every `.await` site whose enclosing scope is not
marked `is_async`. Closures are checked individually — an `async { }`
closure provides an async context for its body.

### B13 — Borrow check across suspend points (DEFERRED in v1)

> **Deferred to follow-up.** The existing borrow checker runs
> post-lowering on HIR; teaching it to treat suspend points as
> borrow-invalidating boundaries is its own slice — the obvious
> wiring (mark every `.await` site as a borrow flush) interacts
> with the multi-arm `match` shape the lowering emits and needs a
> bespoke pin-test surface. The test
> `borrow_across_suspend_rejected_e1010` in `tests/async_lowering.rs`
> is `#[ignore]`d with a reason pointer.



A `&` or `&var` borrow that crosses a `.await` suspend point must
be flagged. The borrowed value sits on the caller's stack but the
suspended future may be polled later from a different stack frame,
so the borrow could outlive its referent.

**Given:**
```rvn
async def bad() -> Int
  let v = compute_vec()
  let r = &v[0]                # borrow
  some_other_future().await    # suspend — &v could dangle
  *r                           # use after suspend
end
```
**Then** E1010 (existing borrow-across-suspend) fires at the borrow
site with a note pointing at the suspend.

Captures by move that cross suspends are fine (they become fields).

### B14 — Drop of state-machine fields on drop / cancellation (DEFERRED in v1)

> **Deferred to follow-up.** v1's lowering eagerly constructs every
> `__sub_N` in `init` and placeholder-initialises every hoisted
> local with a primitive-typed default (Int/Bool/Float). With no
> per-state ownership transitions in the generated code, a "drop
> all fields unconditionally" lowering can't double-drop — nothing
> has been *consumed* at any state boundary. The smart-drop
> optimisation lands when sub-future construction moves from `init`
> to per-state (which is also a prerequisite for lazy-arg sub-
> futures, and for hoisting non-primitive locals across suspends).
> The test `state_machine_drop_only_active_fields` in
> `tests/async_lowering.rs` is `#[ignore]`d with a reason pointer.



When a state machine is dropped before completion (e.g. caller
gives up on the future), its in-flight `__sub_*` field's Drop runs,
plus drops for any active captured locals. The lowering inserts
drop calls in the synthesized class's `__drop` per the state — only
fields live in the current state get dropped (not all fields
unconditionally, which would double-drop locals that were already
consumed).

This is one of the more subtle parts of state-machine lowering.
Pin test: a state machine in state 1 holds `__sub_0` Ready and
`__sub_1` Pending; dropping the parent must drop `__sub_1` only.

---

## Pin tests

| Behaviour | Test fn                                                | File                          |
|-----------|--------------------------------------------------------|-------------------------------|
| B1 (2A)   | `async_def_signature_lifts_to_some_future`             | `tests/async_lowering.rs`     |
| B2 (2A)   | `async_def_generates_state_machine_class`              | `tests/async_lowering.rs`     |
| B3 (2A)   | `async_def_args_become_state_machine_fields`           | `tests/async_lowering.rs`     |
| B5 (2A)   | `poll_after_ready_returns_pending`                     | `tests/async_lowering.rs`     |
| B6 (2A)   | `context_test_dummy_constructs`                        | `tests/async_lowering.rs`     |
| B1-B6 e2e | `cases/721_async_def_no_await_handpoll.rvn`            | release-e2e                   |
| B7 (2B)   | `await_desugars_to_poll_match_pending_return`          | `tests/async_lowering.rs`     |
| B8 (2B)   | `local_live_across_await_promoted_to_field`            | `tests/async_lowering.rs`     |
| B10 (2B)  | `chained_awaits_generate_n_plus_1_states`              | `tests/async_lowering.rs`     |
| B11 (2B)  | `await_in_if_match_branches_lower`                     | `tests/async_lowering.rs`     |
| B12 (2B)  | `await_outside_async_context_rejected_e1110`           | `tests/async_negative.rs`     |
| B13 (2B)  | `borrow_across_suspend_rejected_e1010`                 | `tests/async_negative.rs`     |
| B14 (2B)  | `state_machine_drop_only_active_fields`                | `tests/async_lowering.rs`     |
| B7-B14 e2e| `cases/722_async_def_chained_await_handpoll.rvn`       | release-e2e                   |

---

## Milestone 2B v1 scope reductions (shipped 2026-05-20)

The v1 cut of 2B supports a canonical straight-line `.await` shape:

```rvn
async def f(args...) -> R
  let x_1 = g_1(<args from outer / consts>).await
  let x_2 = g_2(<args from outer / consts>).await
  ...
  <straight-line tail>
end
```

with these constraints:

* Each `.await` must be the outermost suffix of a `let x = … .await`
  statement (no `.await` deeper in a complex expression, no bare
  `expr.await` as a statement).
* The awaitee must be a direct `name(args)` call to another top-
  level `async def` whose return type the pass already knows.
* The awaitee's arguments must be expressible from outer-fn args
  and constants only — they cannot depend on prior await results
  (sub-futures are eagerly constructed in `init`).
* Hoisted-local field types must be Int/Bool/Float (so the
  placeholder default in `init` is safe). Other types require the
  `Option[T]`-wrapping lift, deferred.
* `.await` inside `if`/`match`/`while`/`for` is NOT supported.
  Loop-body `.await` gets E1115; arm-body `.await` is rejected by
  the lowering falling back to "leave the fn alone" and the
  resolver/typeck producing whatever error first surfaces.

B11, B13, B14 are explicitly DEFERRED (each section above carries
its own deferral block). Their pin tests in
`compiler/riven_core/tests/async_lowering.rs` are `#[ignore]`d with
reason strings pointing back here.

## Out of scope (sub-phase 3+)

- **`block_on` executor** (sub-phase 3). This sub-phase tests via
  hand-written poll loops + the `Context.test_dummy` harness.
- **Async I/O** (sub-phase 4).
- **`task.spawn`** (sub-phase 5).
- **`while`/`for` with `.await` in body** — E1115 (await in loops
  not yet supported). v2.
- **`?` operator in async functions** — should still work
  unchanged, since `?` is lowered before async-state-machine
  lowering. Verify with a quick smoke test, but not a separate
  behaviour.
- **`async ||` closure lowering** — same state-machine machinery
  applies but the lowering is more involved (closure captures +
  state-machine fields interact). Defer to a follow-up unless it
  falls out for free.
- **Self-referential state machines** (Pin replacement) — `!Move`
  semantics per the prompt's design decision; for v1 the lowering
  emits a `class __XxxFuture: include !Move` marker but otherwise
  doesn't enforce. Real `!Move` enforcement is its own piece.

## Reserved error codes (new)

- E1110 — `.await` outside async context
- E1114 — future polled after Ready (NOT used in v1 — we pick "return Pending forever" per B5)
- E1115 — await in loops (`while`/`for` body) not yet supported

E1010 (borrow across suspend) reused from the existing borrow-check
error code space — same diagnostic, suspend points are just another
borrow-invalidating boundary.
