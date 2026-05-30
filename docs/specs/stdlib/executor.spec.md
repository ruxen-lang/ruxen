# Spec — `std.executor.block_on` (async sub-phase 3)

**Source docs:**
[docs/requirements/tier1_03_async.md](../../requirements/tier1_03_async.md) §6,
[docs/prompts/v1/15_phase4_async.md](../../prompts/v1/15_phase4_async.md),
[docs/specs/stdlib/async.spec.md](async.spec.md) (sub-phase 1 surface),
[docs/specs/syntax/async_lowering.spec.md](../syntax/async_lowering.spec.md) (sub-phase 2 lowering).

**Status:** sub-phase 3 of the async round. Sub-phase 1 + 2 (commits
`b513e90`, `74c846d`, `92a5035`) shipped the surface + state-machine
lowering. This sub-phase wires the single-threaded **block_on
executor** so `block_on(future)` runs a future to completion.

V1 scope is intentionally minimal — no real wake-druxen scheduling
yet. The wake mechanism only matters once there are concurrent
tasks (sub-phase 5) or I/O events (sub-phase 4). With just `block_on`
+ user-written CPU-bound futures, a poll loop suffices.

---

## B1 — `std.executor.block_on(future)` runs a future to completion

```rx
use std.executor.block_on

def main
  let result = block_on(make_int())
  puts "#{result}"
end
```

**Given** an `async def make_int() -> Int  42 end` (lowered to a
`__MakeIntFuture` state machine in sub-phase 2A)
**When** the caller invokes `block_on(make_int())`
**Then** `block_on` returns `42`.

Surface: `block_on` is a free function in `std.executor` taking any
type that includes `Future` and returning the future's `Output`
type.

Lib decl (in `library/std/future/src/lib.rx` or a new
`library/std/executor/src/lib.rx` package):
```rx
def block_on as "ruxen_executor_block_on"(future: Int) -> Int
```

(Generic-over-Output return type follows the typed-FFI-returns
pattern from commit `0f357d5` — the typeck-level lift can be added
once the executor proves the runtime path; v1 ships with `-> Int` and
the user destructures at the call site or uses a Ruxen-level wrapper.)

## B2 — Poll loop implementation

```c
// library/std/executor/runtime/executor.c
int64_t ruxen_executor_block_on(int64_t future_ptr) {
    // Construct a real Context with a working Waker.
    RuxenContext ctx = ruxen_executor_make_context();
    for (;;) {
        // Call the future's poll(&var ctx) method via its mixin
        // dispatch. The lowered state machine class implements
        // Future, so this resolves through the Future-mixin vtable.
        int64_t poll_result = ruxen_future_poll(future_ptr, &ctx);
        if (ruxen_poll_is_ready(poll_result)) {
            int64_t value = ruxen_poll_unwrap_ready(poll_result);
            ruxen_executor_free_context(&ctx);
            return value;
        }
        // Pending — wait until a wake or the next iteration.
        ruxen_executor_park(&ctx);
    }
}
```

`ruxen_executor_park` is the wait point. In v1's simplest form it
spins (yield-loop with `sched_yield()`) since nothing in the
no-I/O / no-spawn scope actually parks. Sub-phase 4 replaces it
with `pthread_cond_wait` on the executor's wake condvar.

## B3 — `Waker.wake` becomes a real wake signal (single-threaded, no-op for v1 sub-phase 3)

For v1 sub-phase 3, `Waker.wake` and `Waker.wake_by_ref` are no-ops:
the poll loop spins continuously, so any "wake" is implicit. The
real signaling lands with sub-phase 4 when async I/O introduces
genuine park points (where the executor blocks on epoll/kqueue and
needs an explicit wake).

The lib decls in `library/std/future/src/lib.rx` are updated so
`ruxen_waker_wake` / `ruxen_waker_wake_by_ref` are NO-OPS (they no
longer `ruxen_panic`). The "wake" channel is conceptually wired but
mechanically inert until sub-phase 4.

## B4 — `Context.waker(&self) -> &Waker` returns the executor's waker

After this sub-phase, `Context.waker` returns a real waker (not the
panic-stub from sub-phase 1). The waker is the executor's "wake-me"
handle; in v1 sub-phase 3 it's a no-op singleton (calling wake on
it does nothing because the loop is already spinning).

## B5 — `Context.test_dummy` continues to work

The test_dummy Context from sub-phase 2A's `ruxen_context_test_dummy`
keeps working unchanged. It's still the way unit tests construct a
Context without invoking the full executor — useful for testing
hand-written state machines in isolation.

## B6 — `block_on` rejects nested invocation (E1112)

**Given** an async context (e.g. inside `async def main` or
`async { ... }`) that calls `block_on(other_future)`
**Then** typeck (or borrow check) rejects with E1112:
```
[E1112] error: cannot call `block_on` inside an async context — this would deadlock
   |  block_on(other_future)
   |  ^^^^^^^^
note: use `.await` instead to await a future inside an async context
```

The check fires at every `block_on` call site whose enclosing scope
is async (`is_async` true on the enclosing fn/closure). Same shape as
E1110 (`.await` outside async) but inverted.

## B7 — block_on round-trip for sub-phase-2A futures

```rx
use std.executor.block_on

async def make_int() -> Int
  42
end

def main
  puts "#{block_on(make_int())}"
end
```

Prints `42`. The single-state future returns Ready on the first
poll, so block_on returns immediately. E2E fixture.

## B8 — block_on round-trip for sub-phase-2B chained-await futures

```rx
use std.executor.block_on

async def inner_a() -> Int
  10
end

async def inner_b() -> Int
  20
end

async def outer() -> Int
  let a = inner_a().await
  let b = inner_b().await
  a + b
end

def main
  puts "#{block_on(outer())}"
end
```

Prints `30`. The outer future suspends across two `.await`s; the
poll loop polls multiple times before reaching Ready. E2E fixture.

## B9 — Drop on completion

After `block_on` returns the future's output, the future itself is
dropped (its `__drop` runs, which clears `__sub_*` fields and any
hoisted locals). The poll loop holds the future by-value; returning
the output and falling out of the function drops it cleanly.

Pin test: a state machine with a Drop-tracking sub-future asserts
that exactly one drop fires after `block_on` returns.

## B10 — No leak under repeated `block_on`

```rx
def main
  let mut i = 0
  while i < 1000
    let _ = block_on(make_int())
    i = i + 1
  end
end
```

(Where `_` discards.) Memory-bench-style pin test: this loop's RSS
should be flat (within tolerance). Sub-phase 3's executor must not
leak per-iteration heap allocations beyond what the future itself
holds.

---

## Pin tests

| Behaviour | Test fn                                              | File                          |
|-----------|------------------------------------------------------|-------------------------------|
| B1        | `block_on_runs_subphase2a_future_to_ready`           | `tests/async_executor.rs`     |
| B2        | (covered by B1, B7, B8 end-to-end runs)              | —                             |
| B3        | `waker_wake_is_noop_in_subphase3`                    | `tests/async_executor.rs`     |
| B4        | `context_waker_returns_real_waker_after_subphase3`   | `tests/async_executor.rs`     |
| B5        | `context_test_dummy_still_works`                     | `tests/async_executor.rs`     |
| B6        | `block_on_inside_async_rejected_e1112`               | `tests/async_negative.rs`     |
| B7        | e2e `cases/723_block_on_subphase2a_future.rx`       | release-e2e                   |
| B8        | e2e `cases/724_block_on_subphase2b_chained.rx`      | release-e2e                   |
| B9        | `block_on_drops_future_after_return`                 | `tests/async_executor.rs`     |
| B10       | `block_on_loop_does_not_leak`                        | `tests/async_executor.rs`     |

---

## Out of scope (sub-phase 4+)

- **Real wake mechanism** — sub-phase 4 wires `pthread_cond_wait` /
  epoll / kqueue, so park/wake become genuine blocking with
  signaling.
- **Async I/O** (sub-phase 4) — AsyncTcpStream, AsyncFile, time.sleep.
- **`task.spawn` + `task.yield_now`** (sub-phase 5) — multi-task within
  the single-threaded executor.
- **Multi-threaded work-stealing executor** — v2.

## Reserved error codes (new for sub-phase 3)

- E1112 — `block_on` called inside async context (would deadlock)
