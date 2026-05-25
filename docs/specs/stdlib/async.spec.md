# Spec — `std.future` (Future / Poll / Waker / Context)

**Source docs:**
[docs/requirements/tier1_03_async.md](../../requirements/tier1_03_async.md),
[docs/prompts/v1/15_phase4_async.md](../../prompts/v1/15_phase4_async.md).

**Status:** **Sub-phase 1 of the async round — pure stdlib surface.**
The Future mixin, Poll enum, and Context/Waker shells type-check and
participate in trait resolution. No `async def` lowering yet (sub-phase
2), no executor (sub-phase 3), no async I/O (sub-phase 4). Running a
`block_on(future)` panics with "executor not implemented".

The surface is staged ahead of the runtime so user-facing API shape
can stabilise. Same expansion pattern as `sync.spec.md` followed
during the multithreading round.

---

## B1 — `mixin Future` with associated `type Output`

```rx
mixin Future
  type Output
  def var poll(cx: &var Context) -> Poll[Self.Output]
end
```

`Future` is registered as a built-in mixin under `std.core`
(matching the existing minimal `mixin Future; def poll; end` shell
that ships today). The expansion adds:

1. Associated type `Output` — every implementor declares its
   concrete output type. Typeck resolves `Self.Output` through the
   includer's `type Output = T` binding.
2. Method signature `def var poll(cx: &var Context) -> Poll[Self.Output]`
   — the required method's signature in the mixin body. Implementors
   may override but must satisfy the wire-shape (`&var Context` arg,
   `Poll[Self.Output]` return).

Resolvable via `use std.core.Future` (the new canonical path) or
`Future` directly (member of the global prelude).

## B2 — `enum Poll[T] { Ready(T), Pending }`

```rx
enum Poll[T]
  Ready(T)
  Pending
end
```

Registered as a built-in tagged enum (same shape as `Option[T]` and
`Result[T, E]`) via `register_builtins` in `resolve/mod.rs`. Tag
layout pinned: `Ready = 0, Pending = 1`. The runtime never inspects
these tags in sub-phase 1 (no executor), but the layout is fixed
for cross-build stability.

**Match exhaustiveness:** the typeck enforces all variants are
covered, same as Option/Result.

```rx
match poll_result
  Ready(v) -> v
  Pending  -> # waiting
end
```

## B3 — `class Context` + `class Waker` with the v1 API

```rx
class Context
  lib "runtime/executor.c"
    def waker as "ruxen_context_waker"(self) -> &Waker
  end
end

class Waker
  lib "runtime/executor.c"
    def wake as "ruxen_waker_wake"(self) -> ()
    def wake_by_ref as "ruxen_waker_wake_by_ref"(self) -> ()
  end
end
```

Both classes are opaque to user code in v1 — they hold an executor-
maintained pointer and expose only the wake API. Lib decls point at
the as-yet-unwritten `library/std/future/runtime/executor.c`
(creating the package as part of this sub-phase, with stub C
implementations that `ruxen_panic` for now — they become real in
sub-phase 3).

Context/Waker move from their current home in `library/std/sync/src/lib.rx`
to a new `library/std/future/src/lib.rx` package, with a deprecated
re-export from sync to avoid breaking any caller. (Probably none —
they're currently empty shells.)

## B4 — Hand-written Future round-trip (typeck-only)

```rx
class CountdownFuture
  remaining: Int
  include Future
  type Output = Int

  def init(n: Int)
    self.remaining = n
  end

  def var poll(cx: &var Context) -> Poll[Int]
    if self.remaining == 0
      Poll.Ready(0)
    else
      self.remaining = self.remaining - 1
      Poll.Pending
    end
  end
end
```

**Given** the class above
**Then** the program type-checks. The `include Future` resolves;
the `type Output = Int` binds the associated type; `Poll[Int]` is
recognised as the satisfying return.

Calling `f.poll(&var ctx)` at runtime panics until sub-phase 3 lands
`block_on` — but the call site type-checks cleanly.

## B5 — Negative: `include Future` without `type Output` rejected

**Given**
```rx
class BadFuture
  include Future
  def var poll(cx: &var Context) -> Poll[Int]
    Poll.Ready(0)
  end
end
```
**Then** typeck rejects: `[E0612] error: include of mixin 'Future'
requires associated type Output to be bound`. Diagnostic E0612 is
reused (the existing "missing required associated type" code).

## B6 — Negative: `poll` with wrong signature rejected

**Given** `def var poll(cx: &var Context) -> Int` (returning Int
instead of `Poll[Self.Output]`)
**Then** typeck rejects with the existing mixin-signature mismatch
diagnostic (E0613).

## B7 — `async def foo` parses (no lowering)

```rx
async def fetch(url: &str) -> Result[String, IoError]
  Result.Ok(url.to_string)
end
```

Parser accepts `async` as a function modifier. Today the body
type-checks AS A NON-ASYNC FUNCTION (sub-phase 1 doesn't lower) —
the `async` flag is recorded on `FuncDef` but doesn't change the
return type or insert a state machine. Running the program calls
the function as-is. Sub-phase 2 lifts the return to `some Future`
and lowers.

## B8 — `async { ... }` block parses (no lowering)

```rx
let fut = async { 42 }
```

Same caveat as B7. Today this is equivalent to `let fut = 42`.
Sub-phase 2 makes it return a `some Future`.

## B9 — `.await` parses (sub-phase 2 wires the lowering)

```rx
async def main
  let x = some_future.await
end
```

`.await` is a postfix operator the parser recognises. In sub-phase
1 it elides to a no-op (`x = some_future`); sub-phase 2 makes it a
real suspension point.

This means user code written against the async surface in
sub-phase 1 parses + type-checks, ships as a working synchronous
program, and gets async semantics for free once sub-phase 2 lands.
**Bridge mode.**

## B10 — `library/std/future/Ruxen.toml` package

A new package created during sub-phase 1:
- `library/std/future/Ruxen.toml` declaring `std-future` v0.1.0
  with deps on `std-core`.
- `library/std/future/src/lib.rx` carrying the class shells from B3.
- `library/std/future/runtime/executor.c` carrying stub
  implementations of `ruxen_context_waker`, `ruxen_waker_wake`,
  `ruxen_waker_wake_by_ref` — each `ruxen_panic("executor not
  implemented")` for now. Sub-phase 3 fills them in.

## B11 — Declare Poll as an .rx enum (not register_builtins)

`Poll[T]` lives in `library/std/future/src/lib.rx` as a plain
Ruxen enum:

```rx
enum Poll[T]
  Ready(T)
  Pending
end
```

This matches the existing pattern for `IoError` / `SeekFrom` /
`IoErrorKind` / `Shutdown` — every other recently-added stdlib enum
is in pure Ruxen, not in `register_builtins`. The bootstrap merge
picks it up. Tag indices follow declaration order (0 = Ready, 1 =
Pending). Pin test (`poll_tag_layout_stability`) reads the .rx
source and asserts the order — same shape as
`io_error_tag_stability` and `shutdown_tag_stability`.

This is consistent with the project directive: keep the surface in
Ruxen wherever feasible; Rust handles only what cannot be expressed
in Ruxen (lowering passes, codegen, FFI registration).

---

## Pin tests

| Behaviour | Test fn                                              | File                                |
|-----------|------------------------------------------------------|-------------------------------------|
| B1        | `future_mixin_has_associated_output_and_poll`        | `tests/async_surface.rs`            |
| B2        | `poll_enum_registered_with_ready_pending_variants`   | `tests/async_surface.rs`            |
| B2 (tag)  | `poll_tag_layout_stability`                          | `tests/async_surface.rs`            |
| B3        | `context_and_waker_classes_resolve`                  | `tests/async_surface.rs`            |
| B4        | e2e `tests/release-e2e/cases/720_handwritten_future_typechecks.rx` (no execution — just compile) | release-e2e |
| B5        | `include_future_without_output_rejected_e0612`       | `tests/async_negative.rs`           |
| B6        | `poll_signature_mismatch_rejected_e0613`             | `tests/async_negative.rs`           |
| B7        | `async_def_parses_subphase1_no_lowering`             | `parser/tests.rs`                   |
| B8        | `async_block_parses_subphase1_no_lowering`           | `parser/tests.rs`                   |
| B9        | `dot_await_parses_subphase1_elides_to_value`         | `parser/tests.rs`                   |
| B10       | (covered by build — package discovers and links)     | —                                   |
| B11       | covered by B2                                        | —                                   |

---

## Out of scope (sub-phase 2+)

- **`async def` lowering** (sub-phase 2) — state-machine struct generation, suspend-point partitioning, drop tracking across suspends.
- **`.await` desugaring** (sub-phase 2) — postfix operator becomes a `match self.poll(cx) { Ready(v) -> v, Pending -> return Pending }` loop.
- **`block_on` executor** (sub-phase 3) — single-threaded cooperative scheduler with wake queue.
- **Async I/O** (sub-phase 4) — `AsyncTcpStream` / `AsyncFile` / `time.sleep`.
- **`task.spawn` + `task.yield_now`** (sub-phase 5) — multi-task within the single-threaded executor.
- **`Send` bound on cross-thread futures** — v2 (paired with multi-threaded scheduler).
- **`select!` macro** — v2.
- **Stream / AsyncIterator** — v2.
- **Pin / Unpin** — replaced by `!Move` semantics per the decision-already-made list in the v1 prompt.

## Out of scope (v2 — separate roadmap)

- `async def` in mixins.
- Cancellation tokens / `JoinSet` (Rust's tokio-style task lifecycle).
- Multi-threaded work-stealing executor.
