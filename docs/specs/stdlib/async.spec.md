# Spec — `std.future` (Future / Poll / Waker / Context)

**Source docs:**
[docs/requirements/tier1_03_async.md](../../requirements/tier1_03_async.md),
[docs/prompts/v1/15_phase4_async.md](../../prompts/v1/15_phase4_async.md).

**Status:** **parser + typeck surface only.**  No runtime executor;
no `async` block evaluation; running an async program panics at
runtime.  Phase 4 lands the executor.

The async surface is staged ahead of the runtime so user-facing API
shape can stabilise.  Once the executor lands, this spec gains B-rows
for the runtime behaviours; today's pins prove the contract holds at
the type level only.

---

## B1 — `Future` mixin is registered

`Future` is a mixin with required method
`def var poll(ctx: &var Context) -> Poll[T]`.  Resolvable
via `use std.future.Future`.

## B2 — `Poll[T]` enum has `Ready(T)` and `Pending` variants

```riven
match poll_result
  Ready(v) -> v
  Pending  -> # wait
end
```

`Poll[T]` is registered as a tagged enum like `Option` / `Result`.

## B3 — `Waker` and `Context` classes exist

`Waker` carries a wake callback; `Context` wraps a `&Waker`.  Both
are class types — opaque to user code in v1.

## B4 — `async def` parses

```riven
async def fetch(url: &str) -> Result[String, IoError]
  ...
end
```

The parser accepts `async` as a function modifier.  Today the body
type-checks but does not lower to a state-machine MIR — running it
panics.

## B5 — `async { }` block parses

```riven
let fut = async { 42 }
```

Same caveat as B4: parses + typechecks; runtime panics.

## B6 — `.await` parses

```riven
async def main
  let x = some_future.await
end
```

`.await` is a postfix operator the parser recognises only inside
`async` contexts.

---

## Pin tests

| Behaviour | Test fn                                       | File                          |
|-----------|-----------------------------------------------|-------------------------------|
| B1-B3     | (typeck reachability via builtin registration) | `crates/riven-core/src/resolve/mod.rs` self-consistency (no dedicated integration pin yet) |
| B4-B6     | parser unit tests in `crates/riven-core/src/parser/tests.rs` (search for `async`) | |

---

## Gaps (large; this is a runtime-not-shipped feature)

- No integration test under `crates/riven-core/tests/stdlib_async.rs`.
  When Phase 4 lands an executor, add:
  - `async_block_runs_and_returns_value` — `async { 42 }.poll()` returns `Ready(42)`.
  - `await_chains_two_futures` — sequential `.await` of two
    independent futures.
  - `select_between_two_futures` — a `select!` macro pin.
- Phase 4 prompt at `docs/prompts/v1/15_phase4_async.md` enumerates
  the runtime behaviours; each becomes a B-row here when shipped.

## Out of scope (v2)

- `async def` in mixins.
- Stream / AsyncIterator surface.
- Pin / Unpin.
- Cancellation tokens / `JoinSet`.
- `tokio`-style multi-threaded executor — v1 will ship single-threaded.
