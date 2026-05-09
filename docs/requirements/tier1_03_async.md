# Tier 1 — Feature 03: Async / Await

Status: Proposed (no code in-tree).
Target: Riven v0.3 — v0.5 (phased).
Owner: Compiler team.

---

## 1. Summary & Motivation

Riven is positioned as a systems-capable language for 2026-era network code
(HTTP services, database drivers, gateways, embedded agents). These workloads
are dominated by I/O-bound concurrency: thousands of connections waiting on
sockets and timers. A thread-per-connection model wastes memory and context
switches; green-thread runtimes (Go, Crystal) trade a simpler mental model for
an opaque, always-on scheduler that is awkward to embed in libraries.

Rust's stackless `async fn` + `Future` trait has emerged as the
ownership-friendly compromise: zero-cost suspension, no hidden heap
allocations per task, pluggable executors. Because Riven already commits to
Rust-style ownership, borrowing, move semantics, and AOT compilation (see
core principles P1-P5), stackless async is the only design that composes
cleanly with the rest of the language.

This document specifies how async/await integrates into Riven's six-phase
pipeline (lexer → parser → resolve → typeck → borrow_check → mir → codegen)
and defines a minimum viable executor so that a user can write:

```riven
async def fetch_user(id: UInt64) -> Result[User, HttpError]
  let response = await http.get("/users/#{id}")
  response.json[User]
end

def main
  runtime.block_on do
    let user = await fetch_user(42)
    puts(user.name)
  end
end
```

---

## 2. Current State

### 2.1 What exists

- **Lexer.** `async` and `await` are already reserved keywords:
  `crates/riven-core/src/lexer/token.rs:84-85` (`TokenKind::Async`,
  `TokenKind::Await`) with lookup at
  `crates/riven-core/src/lexer/token.rs:306-307`. `yield` is also reserved
  (`:83`, `:305`). Concurrency-adjacent keywords `actor`, `spawn`, `send`,
  `receive` are reserved but unused (`:127-130`, `:349-352`).

- **AST.** A `Yield` expression variant exists:
  `crates/riven-core/src/parser/ast.rs:324-325` (`ExprKind::Yield(Vec<Expr>)`)
  parsed at `crates/riven-core/src/parser/expr.rs:434-450`. **No** `async`
  marker on `FuncDef` (`ast.rs:544-556`), **no** `Await` expression,
  **no** `Future` type.

- **Resolver.** `Yield` is lowered to a plain unresolved function call named
  `"yield"`: `crates/riven-core/src/resolve/mod.rs:1971-1983`. It does not
  participate in control-flow tracking.

- **Runtime shim.** `yield` is currently mapped to a no-op passthrough in
  the codegen runtime table:
  `crates/riven-core/src/codegen/runtime.rs:69-70`. This is a placeholder
  — the current behaviour is effectively "call the block argument once".

- **Formatter.** `Yield` is pretty-printed:
  `crates/riven-core/src/formatter/format_expr.rs:243-251` and
  `comments.rs:842`.

- **Semantic tokens.** The LSP recognizes `async`/`await`/`yield` as
  keywords: `crates/riven-ide/src/semantic_tokens.rs:109-111`.

### 2.2 What is missing

| Layer        | Missing                                                                 |
|--------------|-------------------------------------------------------------------------|
| AST          | `is_async: bool` on `FuncDef`; `ExprKind::Await(Box<Expr>)`; `async do` block form |
| HIR          | `HirExprKind::Await`; function `is_async` flag; desugared-future type   |
| Ty           | `Ty::Future(Box<Ty>)` (or modelled via a built-in trait `Future`)       |
| Resolver     | Check `await` only inside `async` functions / closures; track an "async scope" bit |
| Typeck       | `T: Future<Output = U>` constraint; default `Output` associated type    |
| Borrow check | Detect borrows live across a suspend point; diagnose `!Send` futures when spawned |
| MIR          | Generator/state-machine lowering pass; `Suspend` terminator             |
| Codegen      | Self-referential state struct layout; `Pin` equivalent                  |
| Runtime (C)  | No event loop, no epoll/kqueue/IOCP wrapper, no task queue — `runtime/runtime.c` has zero async primitives (verified by grep: no `async`, `poll`, `epoll`, `executor`, `future`, `task`). |
| Stdlib       | No async I/O module. `puts`/`print` are blocking FFI wrappers.         |

### 2.3 Implication

Async is a **new** feature end-to-end. The existing `Yield` AST node is a
vestigial placeholder — it is not a generator and not a suspension point
today. Section 10 discusses whether to repurpose it or deprecate it.

---

## 3. Goals & Non-Goals

### Goals

- G1. `async def` functions compile to stackless state machines. No hidden
  allocation per suspension.
- G2. `await` suspends the calling async context until a `Future` resolves.
  Syntax is Riven-native, not a transplant.
- G3. The borrow checker rejects `&mut` (and `&`) borrows that cross a
  suspend point illegally — the guarantee is strictly stronger than "compiles
  in Rust".
- G4. A minimal built-in single-threaded executor ships with the compiler,
  callable as `runtime.block_on(async_expr)`.
- G5. The `Future` trait is part of `core`, user-implementable, and has a
  stable `Poll[T]` enum.
- G6. Async functions interoperate with non-async code: a non-async caller
  can await via `block_on`; an async function can call non-async code
  freely.
- G7. The design must not preclude a future work-stealing, multi-threaded
  executor (`runtime::spawn`, `Send`-bound tasks).
- G8. Diagnostics: "cannot `await` outside an `async` function" is a
  first-class error with a fix-it suggestion.

### Non-goals (for Tier 1)

- N1. `async fn` in traits with full object-safety (deferred; see §15).
- N2. `async Drop`.
- N3. Structured concurrency primitives (`TaskGroup`, nursery) — deferred
  to Tier 2 once the base works.
- N4. Cross-thread cancellation tokens. Phase 1 supports only abort-on-drop.
- N5. Async iterators / streams (`for await x in stream`). Deferred.
- N6. A pluggable reactor protocol across OSes beyond Linux epoll / macOS
  kqueue. Windows IOCP is a stub in Phase 3d.

---

## 4. Design Choice: Stackless vs Stackful

### Options considered

| Dimension            | Stackless (Rust)         | Stackful (Go/Crystal/Kotlin*)   |
|----------------------|--------------------------|----------------------------------|
| Per-task memory      | Size of state machine (tens of bytes) | Whole stack (KB-MB) |
| Suspension cost      | One compare-and-jump     | Stack switch + GC-safe point   |
| FFI across `await`   | Trivial — no stack       | Hard — FFI uses OS stack       |
| Ownership / borrow   | Transparent to borrow checker: suspend points are visible in MIR | Opaque: any call may suspend, violating "no borrows across suspend" is hard to express |
| User ergonomics      | Viral (`async` colour)  | Non-viral (looks sync)         |
| Debuggability        | Async backtraces are synthetic | Native stack traces work     |
| AOT binary size      | Small, one state enum per async fn | Large runtime, growable stacks |

*Kotlin coroutines are technically stackless via CPS, but the programmer
experience is stackful.

### Recommendation: Stackless.

The deciding factors are ownership and FFI. Riven's P1-P5 principles forbid
a hidden allocator, a growable stack managed by a runtime, or a GC-style
safepoint — all of which stackful coroutines effectively require. Stackless
futures also play cleanly with Riven's existing MIR CFG representation
(§7): a suspend point is just a new kind of terminator.

The "viral colour" objection is acknowledged. Mitigation: phase 3 ships
`runtime.block_on` so pure library users can call async APIs without
themselves being `async`. Documentation must lean hard on "async is a
compilation target, not a runtime".

---

## 5. Surface Syntax

### 5.1 Async functions

```riven
async def fetch(url: &str) -> Result[String, IoError]
  let conn = await net.connect(url)
  let bytes = await conn.read_all()
  Ok(String.from_utf8(bytes)?)
end
```

Grammar addition (parser/mod.rs at the item-level dispatcher around
`:513` and `:517`):

```
FuncDef   ::= Visibility? "async"? "def" ...
```

`async` is allowed before `def` but after visibility:
`pub async def ...`. It is also allowed on methods in `class`, `impl`, and
`trait` bodies. `async` before `init`/`def mut` is **rejected** (an async
constructor makes no semantic sense; an async mutating method is allowed).

`FuncDef` gains a field: `pub is_async: bool`. Printer and formatter must
preserve it.

### 5.2 Async closures

```riven
let f = async do |x| await process(x) end
```

`ClosureExpr` gains `pub is_async: bool` (alongside existing `is_move`).
Parser rule: `async` may appear before `move` or before a `do`/`{`
introducer.

### 5.3 Async block

```riven
let fut = async do
  let a = await op_a()
  let b = await op_b()
  a + b
end
```

An `async do ... end` with no parameters is an **async block expression**,
producing a `Future[T]` where `T` is the block's tail type. This is
needed for combinators and for the `spawn` primitive.

Parser distinguishes async block from async closure by whether the `do`
has `|params|`.

### 5.4 Await

Preferred form — postfix, Ruby/Rust-style method-chain feel:

```riven
let user = fetch_user(id).await
let bytes = conn.read_all().await?
```

Rationale: keeps `?` composable (`.await?`), reads left-to-right, and
avoids prefix-operator precedence hazards. Postfix `.await` is implemented
as a **keyword-method** token sequence: the parser, when it sees a `.`
followed by the `Await` token, produces an `ExprKind::Await(Box<Expr>)`
node. This special-case lives in the postfix-chain loop in
`parser/expr.rs` (alongside `MethodCall` and `FieldAccess`).

Alternative form — prefix `await expr`:

```riven
let user = await fetch_user(id)
```

**Decision**: support both. Prefix `await` is parsed in the unary/primary
dispatcher like `return`/`yield`. Precedence is unary-tight, binding
tighter than binary operators but looser than method calls. The formatter
normalizes to postfix `.await` (more Ruby-like chaining).

### 5.5 Grammar summary (new productions)

```
Item          ::= Vis? "async"? "def" ...
ClosureExpr   ::= "async"? "move"? ("do" ... "end" | "{" ... "}")
AsyncBlock    ::= "async" "do" Block "end"
PrimaryExpr   ::= ... | "await" Expr               # prefix form
PostfixOp     ::= ... | "." "await"                # postfix form
```

---

## 6. The `Future` Trait

### 6.1 Definition (in `core`)

```riven
pub enum Poll[T]
  Ready(T)
  Pending
end

pub trait Future
  type Output
  def poll(&mut self, ctx: &mut Context) -> Poll[Self.Output]
end

pub class Context
  # Opaque for user code; wraps a waker handle for the executor
end

pub class Waker
  def wake(consume self)
  def wake_by_ref(&self)
  def clone(&self) -> Waker
end
```

### 6.2 Where it lives

- Source: `crates/riven-core/src/corelib/future.rvn` (new directory).
- Parsed and resolved identically to other built-in types (`Vec`, `Option`).
- `Ty::Future(Box<Ty>)` is **not** a primitive; it is represented as
  `Ty::Class { name: "Future", generic_args: [T] }` with a registered
  associated type. This avoids growing the `Ty` enum in `hir/types.rs:39`.

### 6.3 Pin — or why Riven can skip it

Rust needs `Pin<&mut T>` because a self-referential generator, once moved,
invalidates internal pointers into itself. Riven has two options:

1. **Replicate Pin.** Add `core::pin::Pin[T]`, require `poll` to take
   `Pin<&mut Self>`, and introduce `Unpin` as an auto-trait.

2. **Make all generated futures address-stable by construction.** Pin's
   purpose is runtime enforcement of "don't move this value after
   polling". Because Riven generates state-machine structs itself, the
   compiler can:

   - Emit futures as `#[address_stable]` — a new internal attribute the
     borrow checker honours (same mechanism as `!Move` in some
     proposals for Rust).
   - Alternatively, force every future into a heap box at creation
     (`Box<dyn Future>`), which moots the problem at a per-task
     allocation cost.

**Recommendation for Phase 3b**: take option (2a). Generated futures are
marked `!Move` at the Ty level — the borrow checker rejects any attempt to
move a future after its first `.poll()`. This deletes an entire concept
(`Pin`, `Unpin`) from user-visible surface area and is consistent with
Riven's philosophy of compiler-enforced invariants over library types.

If the design proves too restrictive (e.g., users want to build
`Vec<Box<dyn Future>>`), fall back to option (1) and introduce `Pin`
explicitly in a Tier 2 revision.

### 6.4 Built-in impls required

| Type                | Source              | Output         |
|---------------------|---------------------|----------------|
| `Ready[T]`          | `async { T }`       | `T`            |
| `Join<A, B>`        | `future.join(a, b)` | `(A.Output, B.Output)` |
| `Race<A, B>`        | `future.race(a, b)` | `A.Output` or `B.Output` |
| `Pending[T]`        | debug helper        | never resolves |

---

## 7. State-Machine Lowering

This is the heart of the feature. The transform converts each async
function body into a state-machine struct plus a `Future` impl.

### 7.1 Where in the pipeline

**MIR lowering (`crates/riven-core/src/mir/lower.rs`) is the correct
place.** Rationale:

- The borrow checker runs on HIR (`borrow_check/mod.rs:30`) and must see
  `await` points to validate borrows across them. Therefore HIR must retain
  `HirExprKind::Await`.
- MIR is already a CFG with basic blocks and terminators
  (`mir/nodes.rs:319-342`). A suspend point is naturally expressed by
  splitting the block at the await and adding a new terminator.
- After MIR lowering, downstream (codegen) doesn't need to know the
  function was async — it sees an ordinary function whose body happens to
  be `poll` and whose surrounding struct is the state.

### 7.2 Transform outline (per async fn `foo -> T`)

1. **Collect live state**: every local variable, parameter, and temporary
   whose live range crosses any `await` point.
2. **Generate state enum**:
   ```
   enum Foo_State
     Init(...params...)
     Suspend0 { <live locals>, inner: F0 }
     Suspend1 { <live locals>, inner: F1 }
     ...
     Done
   end
   ```
   where `Fi` is the type of the inner future being awaited at point `i`.
3. **Generate struct**:
   ```
   struct FooFuture
     state: Foo_State
   end
   ```
   `FooFuture` is marked `!Move` (see §6.3).
4. **Generate `impl Future for FooFuture`**: the `poll` method is a large
   match on `self.state` that dispatches to the corresponding basic block.
5. **Rewrite call sites**: a call `foo(args)` in an async context becomes
   `FooFuture { state: Init(args) }`; it does not execute until polled.
6. **Rewrite `await` expressions** in the original body:
   ```riven
   let x = expr.await
   ```
   becomes, inside the MIR CFG:
   ```
   BB_pre_await:
     tmp_fut = <compute expr>
     state = Suspendi { ..., inner: tmp_fut }
     terminator = Suspend { resume: BB_resume }
   BB_resume:
     match state.inner.poll(ctx)
       Ready(x) → goto BB_after_await
       Pending  → return Pending
     end
   ```
7. **Drop handling**: if the future is dropped while in state
   `Suspend_i`, each live local must be dropped. Extend the existing
   drop-elaboration pass to synthesize a `Drop` impl for `FooFuture` that
   matches on state and drops in the right order.

### 7.3 New MIR nodes

Add to `mir/nodes.rs`:

```rust
pub enum Terminator {
    ...existing variants...,
    /// Yield Pending, resume at `resume` on next poll.
    Suspend {
        state_tag: u32,
        resume: BlockId,
    },
}
```

No new `MirInst` variants needed — state struct field reads/writes are
just `GetField`/`SetField`.

### 7.4 Borrow checker interaction (**critical**)

Run on HIR (`borrow_check::borrow_check`, `borrow_check/mod.rs:30`)
**before** MIR lowering. Add a pass in `borrow_check/mod.rs`:

- For each async function body, compute "live across await": the set of
  locals whose live range spans at least one `HirExprKind::Await`.
- If any member of that set is a `&mut T` borrow with conflicting
  accesses after the await resumes, emit `E_async_borrow_across_await`
  (new error code in `borrow_check/errors.rs`). The diagnostic must
  point at the borrow, the await, and the subsequent use.

Example rejected:

```riven
async def bad(v: &mut Vec[Int])
  let r = &mut v[0]        # &mut borrow
  await yield_now()        # suspension
  *r += 1                  # ERROR: &mut borrow held across await
end
```

Example accepted (borrow released before await):

```riven
async def good(v: &mut Vec[Int])
  v[0] += 1
  await yield_now()
  v[0] += 2                # fresh borrow after resume
end
```

The `Send` check (§9) is a separate pass that runs on the generated state
struct: if any field is `!Send`, the whole future is `!Send`.

---

## 8. Executor

### 8.1 Minimum viable — single-threaded reactor

Phase 3c ships **one** executor: a single-threaded event-loop reactor
patterned after smol / tokio's current-thread runtime.

Public surface (in `core::runtime`):

```riven
# Drive a future to completion on the calling thread.
# Blocks the thread until the future resolves.
def runtime.block_on[F: Future](fut: F) -> F.Output

# Inside an async context, yield to the reactor once.
async def runtime.yield_now() -> ()

# Spawn an additional task onto the current-thread executor.
# Requires F: Future + 'static (task outlives its creator).
def runtime.spawn[F: Future + 'static](fut: F) -> JoinHandle[F.Output]
```

### 8.2 Reactor abstraction

Implementation lives in `crates/riven-core/runtime/reactor.c` (new file,
linked into every Riven binary that uses async — guarded by
`--features runtime-async`):

| OS          | Backend        |
|-------------|----------------|
| Linux       | `epoll` + `eventfd` for wakeups |
| macOS / BSD | `kqueue`       |
| Windows     | Stub in Phase 3d (returns `IoError.Unsupported`) |

The reactor exposes a C ABI that the generated state machines call:

```c
void riven_async_register_read(int fd, RivenWaker *waker);
void riven_async_register_write(int fd, RivenWaker *waker);
void riven_async_register_timer(uint64_t ns, RivenWaker *waker);
void riven_async_unregister(int fd);
int  riven_async_poll_once(int64_t timeout_ns);   // returns # events
```

`RivenWaker` is an opaque `struct { void (*wake)(void*); void *task; }`
(one pointer plus one function pointer — ABI-stable).

### 8.3 Task queue

Minimal task queue: an intrusive doubly-linked list of
`struct RivenTask { /* state-machine struct */ next, prev; ... }`. Wake is
O(1) by push-to-ready. `block_on` drains the ready queue, then calls
`riven_async_poll_once` with timeout = next-timer.

No allocator per `.await` beyond the one boxed root future in
`block_on`.

### 8.4 Startup

**Explicit**, not implicit:

```riven
def main
  runtime.block_on do
    app.run()
  end
end
```

Rationale: matches Rust's explicit `#[tokio::main]`, matches Riven's
"no hidden runtime" principle, and allows the compiler to avoid linking
the reactor for programs that never use async (saves ~30 KB in the
statically-linked binary).

A convenience macro may later be provided:

```riven
async_main! app.run()
```

---

## 9. Integration with `Send` / `Sync`

### 9.1 Current state

Riven has **no `Send` or `Sync` marker traits today**. The reserved
keyword `Send` (lexer `:129`, `:351`) refers to message-send for the
unimplemented actor system, not the auto-trait. This document reserves
the trait names `core::marker::Send` and `core::marker::Sync` (different
path from the keyword) for Phase 3c onward.

### 9.2 For the single-threaded executor

`block_on` and current-thread `spawn` do not require `Send` — the future
is polled on the thread that created it. No new constraints.

### 9.3 For a future work-stealing executor

`runtime::spawn_mt[F: Future + Send + 'static]` requires `Send`. The
compiler-generated state struct is `Send` iff every field
(i.e., every "live across await" local) is `Send`. Because `Send` is an
auto-trait, this is derived mechanically.

### 9.4 Relationship to a `'static` bound

Async tasks outlive the stack frame that spawned them. `spawn` therefore
requires the task future to be `'static`. This forces captures to be owned
or `Arc`-equivalent. The `'static` concept already exists in
`hir/types.rs` as `Ty::RefLifetime("static", ...)`; we extend lifetime
bound checking in `borrow_check/lifetimes.rs` to enforce it on spawn
arguments.

---

## 10. Relationship to the Existing `Yield` AST

The `Yield` variant (`ast.rs:324`) was added for Ruby-style `yield` inside
blocks — i.e., a method yielding to its trailing-block argument. That is
not a generator, not a coroutine, and not a suspend point. It is a
block invocation.

**Decision**: leave `ExprKind::Yield` in place for its Ruby
semantics. It is orthogonal to async. The current runtime shim
(`codegen/runtime.rs:69-70`) remains.

**Generators** (`gen fn`, `yield` as suspend) are a separate Tier 2
feature. If implemented, they will reuse the state-machine lowering
infrastructure built for async, but will not collide with `ExprKind::Yield`
because generators require a **new** introducer (`gen def`, or similar).
The current `yield` keyword will likely need to be reused for generator
yield, which means the Ruby block-yield semantics must migrate to a
method call (`self.block.call(args)`) before generators land. This
migration is explicitly listed as a risk (§15).

**Summary**: async does **not** depend on generators. Ship async first.
Generators can fall out of the same MIR machinery later.

---

## 11. Cancellation & Timeouts

Phase 1 (3b-3c): **abort-on-drop only.** Dropping a future causes its
state machine to drop, which drops all live locals. There is no
`CancellationToken`, no cooperative cancellation signal, no `select!`.

Timeouts are built on drop:

```riven
let result = runtime.timeout(5.seconds, fetch_user(id)).await
```

`timeout` is a combinator future that wakes on a timer and drops the
inner future when it fires. Because drop runs the state machine's
synthesized `Drop` impl (§7.2 step 7), all outstanding resources (open
sockets, in-flight reads) are released.

Phase 2 (Tier 2): structured concurrency with `TaskGroup` and
`CancellationScope`. Out of scope here.

---

## 12. I/O Integration

### 12.1 Approach

Create a new stdlib module `core::async_io`. Do **not** re-export
blocking `fs`/`net` with an `async_` prefix — keep the namespaces
separated to avoid ambiguity about which API a user is using.

Minimum viable surface for Phase 3d:

```riven
# async_io::net
async def TcpStream.connect(addr: &str) -> Result[TcpStream, IoError]
async def TcpStream.read(&mut self, buf: &mut [UInt8]) -> Result[USize, IoError]
async def TcpStream.write(&mut self, buf: &[UInt8]) -> Result[USize, IoError]
async def TcpListener.bind(addr: &str) -> Result[TcpListener, IoError]
async def TcpListener.accept(&mut self) -> Result[(TcpStream, SocketAddr), IoError]

# async_io::time
async def sleep(duration: Duration) -> ()
def Instant.now() -> Instant

# async_io::fs (Phase 3d+1)
async def fs.read_to_string(path: &str) -> Result[String, IoError]
async def fs.write(path: &str, data: &[UInt8]) -> Result[(), IoError]
```

### 12.2 Under the hood

Each async I/O operation registers a `Waker` with the reactor on
`Pending`, then returns. When the kernel reports readiness, the reactor
wakes the task. On Linux this is level-triggered epoll; on macOS kqueue.

File I/O on Linux uses a thread-pool fallback (real async file I/O needs
`io_uring`, deferred to Tier 2) because epoll does not work for regular
files.

---

## 13. Implementation Plan (files to touch)

Lexer: **no changes** (tokens already present).

Parser:
- `crates/riven-core/src/parser/ast.rs` — add `is_async: bool` to
  `FuncDef` (`:544`), `MethodSig` (`:567`), `ClosureExpr` (`:480`);
  add `ExprKind::Await(Box<Expr>)` near `:320`; add
  `ExprKind::AsyncBlock(Block)` if desired for distinct formatting.
- `crates/riven-core/src/parser/mod.rs` — in the item-level
  dispatcher around `:513`/`:517`, accept optional `Async` token
  before `Def`; thread through to `parse_func_def` (`:1285`) — add
  `is_async` parameter. Same for trait / impl method parsing
  (`:1152`, `:1200`, `:1205`).
- `crates/riven-core/src/parser/expr.rs` — in postfix-chain loop,
  handle `Dot Await` → `ExprKind::Await`; in primary dispatcher,
  handle `TokenKind::Await` → prefix form; extend
  `is_expression_start` around `:1056` to include `Await`.
- `crates/riven-core/src/parser/printer.rs` — print the new nodes.
- `crates/riven-core/src/formatter/format_expr.rs` and
  `comments.rs` — format `.await` chains, `async def`, `async do`.

Resolver / HIR:
- `crates/riven-core/src/hir/nodes.rs` — add `HirExprKind::Await`
  (`:51`); add `is_async: bool` to `HirFuncDef` (`:378`) and closure
  (`:168`).
- `crates/riven-core/src/resolve/mod.rs` — lower `ExprKind::Await`
  into `HirExprKind::Await`; add `ScopeKind::AsyncFunction` /
  `ScopeKind::AsyncClosure` to `scope.rs:16` and check that
  `await` only appears inside one (new error: `E_await_outside_async`).

Typeck:
- `crates/riven-core/src/typeck/infer.rs` — typing rule for
  `Await`: `e: F` where `F: Future[Output = T]` yields expression
  of type `T`. Emit constraint via the trait solver. For an `async
  def -> T`, the function's exported type is
  `impl Future[Output = T]` (desugared at type-check or at MIR
  lowering).
- `crates/riven-core/src/typeck/traits.rs` — register `Future` as
  a known trait with one associated type `Output`.

Borrow check:
- `crates/riven-core/src/borrow_check/mod.rs` — new pass
  `check_borrows_across_await`. Reuse existing `BorrowSet` and
  region machinery.
- `crates/riven-core/src/borrow_check/errors.rs` — new `ErrorCode`
  entries: `E_await_outside_async`, `E_borrow_across_await`,
  `E_future_not_send`.

MIR:
- `crates/riven-core/src/mir/nodes.rs` — add `Terminator::Suspend`
  (`:323`).
- `crates/riven-core/src/mir/lower.rs` — new function
  `lower_async_function` near `:115`. Detect `is_async`, compute
  liveness across `HirExprKind::Await`, synthesize state enum +
  state struct + `poll` method, rewrite call sites. Factor out
  liveness analysis into `mir/liveness.rs` (new file).
- `crates/riven-core/src/mir/tests.rs` — new suite
  `async_lowering_tests` with fixture programs.

Codegen:
- `crates/riven-core/src/codegen/cranelift.rs` — handle
  `Terminator::Suspend` (emit a return-with-Pending path and a
  resume label).
- `crates/riven-core/src/codegen/llvm/mod.rs` — same, for LLVM
  backend.
- `crates/riven-core/src/codegen/runtime.rs` — map
  `core::runtime::block_on`, `core::runtime::yield_now`,
  `core::runtime::spawn` to C runtime entry points.

Runtime (C):
- `crates/riven-core/runtime/reactor.c` — new, epoll/kqueue loop.
- `crates/riven-core/runtime/waker.c` — new, waker primitives.
- `crates/riven-core/runtime/runtime.c` — link in reactor
  conditionally.

Corelib (Riven source):
- `crates/riven-core/src/corelib/future.rvn` — `Future`, `Poll`,
  `Context`, `Waker` definitions.
- `crates/riven-core/src/corelib/runtime.rvn` — `block_on`,
  `spawn`, `yield_now`.
- `crates/riven-core/src/corelib/async_io/...` — Phase 3d.

LSP / tooling:
- `crates/riven-ide/src/semantic_tokens.rs:109-111` — already
  flags `async`/`await` as keywords; add hover text.
- `crates/riven-cli/` — no changes.

Tests:
- New fixture files in `crates/riven-core/tests/fixtures/`:
  `async_basic.rvn`, `async_await_chain.rvn`, `async_borrow.rvn`
  (borrow-across-await failure case), `async_send.rvn` (Send
  failure case), `async_timer.rvn`, `async_tcp_echo.rvn`.

---

## 14. Phasing

### Phase 3a — `Future` trait, hand-written state machines (2 weeks)

- Ship `core::future::Future`, `Poll`, `Context`, `Waker` as Riven
  source.
- No `async`/`await` syntax. Users manually implement `Future` for
  their own types.
- No executor beyond a test-only `poll_once` helper.
- Unblocks: design validation of the trait shape, concrete test cases
  for the borrow checker's `!Move` rule.

### Phase 3b — `async def` + `.await` + state-machine lowering (4-6 weeks)

- Parser, HIR, typeck, borrow check changes (§13).
- MIR-level state machine synthesis.
- No executor yet; tests use a manual `run_to_completion` helper that
  polls in a loop until `Ready` or stalls (no real I/O).

### Phase 3c — Minimal single-threaded executor (2 weeks)

- `core::runtime::block_on`, `yield_now`, `spawn` (current-thread).
- `reactor.c` with epoll-only (Linux) + timer wheel.
- Add `async_io::time::sleep`.

### Phase 3d — Async TCP + kqueue port (2-3 weeks)

- `async_io::net::TcpStream`, `TcpListener`.
- macOS kqueue backend.
- Windows reactor returns `Unsupported`.

### Phase 3e (Tier 2) — work-stealing, structured concurrency, async iterators

Out of scope here.

---

## 15. Open Questions & Risks

### Risks

- **R1. `!Move` future strategy may paint us into a corner.** If users
  want to store futures in collections, we'll need `Pin` after all. Keep
  the door open by making `!Move` an internal compiler attribute rather
  than a user-facing marker trait, so we can retrofit `Pin` without a
  breaking change. *Severity: medium. Mitigation: phase 3a tests with
  hand-written impls will expose this early.*

- **R2. `yield` keyword collision.** Ruby-style block `yield` and
  generator `yield` want the same word. Migrating block-yield to
  `block.call(args)` is a source-breaking change for any existing Riven
  programs. *Severity: low today (Riven is pre-1.0). Mitigation:
  deprecate `ExprKind::Yield` in Phase 3b with a compiler warning;
  remove in 1.0.*

- **R3. Borrow-checker liveness across await is non-trivial.** Rust's
  NLL borrow checker took years to get right. Riven's existing checker
  is simpler and may need substantial extension. *Severity: high.
  Mitigation: be conservative — reject anything uncertain; document
  escape-hatch with `unsafe` blocks.*

- **R4. Windows support gap.** IOCP semantics differ enough from epoll
  that pretending to be cross-platform is misleading. *Severity: medium.
  Mitigation: document clearly; reject async I/O calls on Windows with a
  clear error until a real IOCP backend lands.*

- **R5. Cranelift-JIT REPL (Phase 12, see project memory)** needs to
  handle suspend terminators. JIT-compiled async code is harder than
  AOT. *Severity: low (REPL is optional tool). Mitigation: REPL can
  reject async code in a top-level expression, require `runtime.block_on`.*

### Open questions

- **Q1.** Should async functions be callable from non-async code directly
  (returning a future value)? Yes — the call simply constructs the state
  struct; only `.await` requires an async context.

- **Q2.** Is the postfix `.await` actually a method call on `Future`, or
  syntactic sugar? Proposal says sugar (new `ExprKind::Await`). Method
  interpretation would require `Future` to have a built-in `.await`
  method that the type checker synthesizes — clever but obscures
  diagnostics.

- **Q3.** `async fn` in traits: do we punt to Tier 2 or ship with
  dynamic dispatch only (via `dyn Future`)? **Recommendation**: punt to
  Tier 2; ship static-only in Phase 3b.

- **Q4.** Do we need `Unpin` if `!Move` futures can never be moved?
  Probably not — but keep the trait name reserved in case we need it.

- **Q5.** Formatter: one-line `expr.await` vs `expr.await` on a new
  line for long chains. Follow existing formatter conventions from
  `format_expr.rs` for `MethodCall`.

- **Q6.** How do async closures interact with Ruby-style trailing-block
  syntax `v.each do |x| await f(x) end`? Only legal if `each` takes an
  async block. Easy rule: trailing-block closure's `is_async` is
  inferred from whether its body contains `await`. But this conflicts
  with Q1 (explicitness). *Decision*: require explicit `async do` —
  no inference.

- **Q7.** Reactor as a library dependency or as part of `riven-core`?
  Proposal: part of `riven-core`, behind a Cargo feature
  `runtime-async` (on by default). Allows embedded / no_std Riven
  later without dragging in epoll.

---

## 16. Success Criteria

Phase 3b is complete when:

1. `async def f() -> Int\n  42\nend` compiles and type-checks.
2. `f().await` in an `async` context type-checks as `Int`.
3. `f().await` in a non-async context is a compiler error.
4. Borrow-across-await rejection emits diagnostic
   `E_borrow_across_await` with correct spans.
5. All Phase 3a hand-written future impls still work unchanged.
6. `cargo test -p riven-core -- async` passes.

Phase 3c is complete when:

7. A 20-line TCP echo client using `runtime::block_on` +
   `async_io::net::TcpStream` compiles and runs against a local netcat
   listener.
8. A 1000-task sleep/wake benchmark completes in < 100 ms on a modern
   Linux laptop with no per-task heap allocation beyond the initial
   `block_on` frame (verified by `valgrind --tool=massif`).

---

## Appendix A — File Citations Summary

| Concern                | File : Line                                                       |
|------------------------|-------------------------------------------------------------------|
| `async`/`await` reserved | `crates/riven-core/src/lexer/token.rs:84-85, 306-307`            |
| `yield` reserved         | `crates/riven-core/src/lexer/token.rs:83, 305`                  |
| `Yield` AST node       | `crates/riven-core/src/parser/ast.rs:324-325`                    |
| `Yield` parsed         | `crates/riven-core/src/parser/expr.rs:434-450`                   |
| `Yield` resolved (stub) | `crates/riven-core/src/resolve/mod.rs:1971-1983`                 |
| `yield` codegen stub    | `crates/riven-core/src/codegen/runtime.rs:69-70`                 |
| `FuncDef` AST           | `crates/riven-core/src/parser/ast.rs:544-556`                   |
| `FuncDef` parser        | `crates/riven-core/src/parser/mod.rs:1285-1398`                 |
| `HirFuncDef`            | `crates/riven-core/src/hir/nodes.rs:378-390`                    |
| `HirExprKind`           | `crates/riven-core/src/hir/nodes.rs:51-242`                     |
| `Ty` enum               | `crates/riven-core/src/hir/types.rs:39-166`                     |
| MIR `Terminator`        | `crates/riven-core/src/mir/nodes.rs:319-342`                    |
| MIR lower entry         | `crates/riven-core/src/mir/lower.rs:47, 115`                    |
| Borrow checker entry    | `crates/riven-core/src/borrow_check/mod.rs:30`                  |
| Runtime C source        | `crates/riven-core/runtime/runtime.c` (no async primitives)     |
| Scope kinds             | `crates/riven-core/src/resolve/scope.rs:16-35`                  |
| Semantic tokens         | `crates/riven-ide/src/semantic_tokens.rs:109-111`               |
