# Async Preview (typeck only)

> **Status:** **parser + typeck surface only.**  No runtime executor.
> Programs that use `async` / `.await` parse and typecheck, but the
> binary panics at runtime.  Full async runtime lands in Phase 4
> (prompt 15).
>
> **See also:** [Spec — std.future](../specs/stdlib/async.spec.md)
> for the full contract + pin tests.

Async support in Ruxen follows Rust's design: a `Future` mixin with
a `poll` method, a `Poll[T]` enum with `Ready(T)` / `Pending`
variants, and `async def` / `async { }` / `.await` syntax that lowers
to a state-machine.

In v1 only the syntax + types ship.  The runtime executor that
actually drives futures is Phase 4 work.

This chapter shows you what compiles today so you can write async
APIs ahead of the runtime.

---

## 1. The types that exist

```ruxen
use std.future.{Future, Poll, Waker, Context}
```

| Type            | Role                                              | Status            |
|-----------------|---------------------------------------------------|-------------------|
| `Future[T]`     | Mixin with required `poll(&var, &var Context) -> Poll[T]` | types ✓ runtime ✗ |
| `Poll[T]`       | Enum: `Ready(T)` or `Pending`                     | types ✓ runtime ✗ |
| `Waker`         | Class — wraps a wake callback                     | types ✓ runtime ✗ |
| `Context`       | Class — wraps a `&Waker` for the poll call        | types ✓ runtime ✗ |

You can name these types, pattern-match on `Poll[T]` variants, and
write functions that take `&var Context` as a parameter.

---

## 2. `async def`

```ruxen
async def fetch(url: &str) -> Result[String, IoError]
  Ok(String.from("placeholder"))
end
```

The compiler accepts `async` as a function modifier.  The return
type as written is `Result[String, IoError]`, but the actual return
type of an `async def` is `some Future[Output = Result[String, IoError]]`
(matches what Rust would express).

Calling `fetch(...)` today produces a typed value but doesn't run
the body — you need the executor.

---

## 3. `async { ... }` block

```ruxen
let fut = async { 42 }
```

`fut` has type `some Future[Output = Int]`.

---

## 4. `.await`

```ruxen
async def main
  let bytes = fetch(&"https://example.com").await?
  puts "got #{bytes.len} bytes"
end
```

`.await` is a postfix operator legal only inside `async` contexts
(`async def` or `async { ... }` block).  The parser rejects it elsewhere.

When the runtime ships, `.await` will desugar to a `loop { match
poll(...) { Ready(v) => break v, Pending => yield } }`.

---

## 5. What you can do today

- **Design async APIs.**  Method signatures, mixin definitions, and
  return types all typecheck.  You can iterate on shape before the
  runtime lands.
- **Document expected behaviour in specs.**  When the runtime lands,
  the spec already says what each method should do.
- **Write parser tests for `async` syntax in user code.**  They'll
  catch syntax regressions before runtime work begins.

What you **can't** do:

- Run any `async def` body.
- Schedule futures concurrently.
- Test `.await` against real I/O.
- Benchmark async code.

---

## 6. When Phase 4 lands

The phase-4 prompt at
[`docs/prompts/v1/15_phase4_async.md`](../prompts/v1/15_phase4_async.md)
describes the executor design.  Rough sequence:

1. **MIR lowering** for `async def` produces a state-machine struct
   with one variant per `.await` point.
2. **`Executor.block_on(future)`** synchronously polls a future to
   completion.  Single-threaded for v1.
3. **`Waker` is wired to a parker** so `Pending` returns from `poll`
   actually wait until something rings the waker.

Each piece earns a spec section in
[`async.spec.md`](../specs/stdlib/async.spec.md) when it ships.
Today's specs and pin tests guarantee the *type contract* doesn't
drift while the runtime is built up underneath.

---

## 7. Concurrency vs async — different surfaces

Don't confuse `std.future` (this chapter) with `std.sync`
(chapter 22 — concurrency primitives).  `std.sync` is OS threads
+ mutexes + atomics; `std.future` is cooperative single-threaded
async/await.  They compose — a future can spawn a thread and a
thread can run an executor — but they're independent v1 features.

Both have typeck contracts shipping today and runtime work in
Phase 4.

---

**Next:** [Chapter 25 — Implementing Iterator for Your Own Type](25-implementing-iterator.md)
to see how a custom data type can opt into the iterator pipeline,
or browse the [spec index](../specs/README.md).
