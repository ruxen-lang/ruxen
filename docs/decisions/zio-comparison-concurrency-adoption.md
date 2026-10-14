# What Ruxen Should Take From the Ruby-vs-Scala/ZIO Comparison

*Authoritative analysis for the Ruxen maintainers — 2026-06-18*

## TL;DR

**Ruxen is not, and must not become, an effect-system language.** The value of comparing Ruxen against Scala/ZIO is *not* the effect monad (`ZIO[R,E,A]`), `ZLayer` dependency injection, or `STM` — those are JVM/GC patterns that fight Ruxen's eager-ownership, no-GC, "one obvious path" model. The real, high-leverage signal is a narrow set of **concurrency capabilities** that *extend* Ruxen's existing poll-based async (`async def` → synthesized `Future` state machine, `Context`/`Waker`, reactor, `Task.spawn_raw`) and its ownership/`drop` story rather than replacing them: **structured concurrency scopes**, **first-class cancellation with leak-free `def drop` across `.await`**, a **general `Race` combinator**, **async backpressured channels/queues**, **async streams**, **retry/`Schedule` combinators**, and a **deterministic virtual `TestClock`**. These primarily serve **Wedge #3 (the rondo flagship web framework)** and partly **Wedge #2 (WASM/Cloudflare Workers, where async is load-bearing)**. They are deliberately *not* a wedge of their own and they do nothing for **Wedge #1 (the synchronous Ruby-FFI C-extension replacement, which ships first)**. The defensible market slot they unlock is precise and small: *"fearless **structured** concurrency, no GC, reads like Ruby"* — a row neither Crystal, async-Rust, nor Go occupies. Per `docs/STRATEGY.md`, this is a Crystal-sized slice, not a Go-sized one, and the messaging must stay that honest.

---

## 1. Where Ruxen already stands

Ruxen is a compiled, statically-typed, ownership + borrow-checked language with **no GC**, emitting native binaries (Cranelift in dev, LLVM in release). Its identity — *"reads like Ruby, compiles like Rust"* — is anchored by five design principles: **P1** Implicit Safety / Explicit Danger, **P2** The Compiler Works For You, **P3** One Obvious Path, **P4** Own What You Use, **P5** Clarity At The Boundaries.

The in-tree concurrency/async surface is already substantial:

- **Threads & sync primitives.** `Thread.spawn(closure)`, `JoinHandle[T: Send]`, `Thread.sleep`/`yield_now`/`current_id`, `Signal(SIGINT)`. Auto-mixins `mixin Send` + `mixin Sync` (core/lib.rx) give compile-time data-race prevention. `Mutex[T: Send]` + `MutexGuard` + poisoning, the `Atomic*` family (`load`/`store`/`fetch_add`/CAS), and `SharedSync[T: Send]` (atomic-refcount, the `Arc`-equivalent). A **synchronous** MPSC channel (`Sender`/`Receiver`) whose `recv` **blocks the OS thread**.
- **Async core.** Poll-based `async def` lowered to a synthesized `Future` state-machine class; `enum Poll[T]`; `mixin Future` (runtime-dispatched `poll(cx) -> Poll[Output]`); `Context`/`Waker`; a reactor (epoll/kqueue, edge-triggered persistent fd registration — the tokio `AsyncFd` shape); `block_on`; `Async.sleep`.
- **Executor & I/O.** `Task.spawn_raw(future_ptr)` (single-thread, per-reactor, **raw / fire-and-forget / detached** — no parent/child lifetime, no cancellation), `Task.yield_now`, `Task.join`, `TaskHandle`. Async I/O via `async_fs`, `async_io`, and full async TCP in `async_net`. The **only** race-like primitive that exists is `read_with_timeout`, a bespoke 2-way race (read vs per-poll timer).

**What is already solved that a naive ZIO comparison would otherwise suggest re-adding** — do not propose these as "borrow from ZIO":

1. **Typed errors + the failure-vs-defect split.** `Result[T,E]` + `Option[T]` + `?` propagation are the recoverable-failure channel; `unwrap!`/`panic` are unrecoverable *defects*. Ruxen already has ZIO's typed-error virtue without the monad.
2. **Data-race freedom.** `mixin Send` + `mixin Sync` + the borrow checker already prevent data races at compile time. This is the core pitch, not a gap.
3. **Deterministic *synchronous* resource release.** RAII `def drop` at scope exit is the established discipline. ZIO's `acquireRelease` solves the same problem with a GC'd runtime; Ruxen already has the synchronous half.

The honest gaps are all on the **async lifecycle** axis — exactly where the ZIO/structured-concurrency literature has the most to teach, and exactly where the rondo flagship has already drawn blood.

---

## 2. What we are deliberately NOT taking from ZIO, and why

The synthesis must *justify* these exclusions, not silently drop them. Each is off the eager-ownership model, off the locked wedges, or in direct tension with **P3 One Obvious Path**.

- **Full effect system / effects-as-values (`ZIO[R,E,A]`).** An effect monad turns every side-effecting operation into a deferred description interpreted by a runtime. That runtime *is* a scheduler + a GC's worth of allocation, and the monadic composition is a second control-flow universe layered over the language's own. Ruxen already executes effects eagerly with ownership tracking the resources; adding an effect channel would mean **two** ways to sequence work (direct + monadic), violating **P3**, and it presumes a managed runtime Ruxen does not have and does not want (no GC, **P4**). Typed errors — the one genuinely portable benefit — are *already* in-tree via `Result`/`?`. Reject.
- **`ZLayer` DI / the `R` environment channel.** `ZLayer` is a typed, memoized dependency graph threaded through the `R` parameter of every effect. In an eager, ownership-based language, dependencies are *values you own and pass* — construct them, hand them in, and `def drop` releases them at scope exit. A reified environment channel would re-introduce the monad it rides on and obscure the boundary where a dependency is acquired and released (**P5 Clarity At The Boundaries**, **P4 Own What You Use**). Reject; ordinary constructor injection over owned values is the obvious path.
- **Software Transactional Memory (STM / `TRef`).** STM offers composable optimistic concurrency by re-running transactions on conflict. That model assumes cheap rollback of arbitrary effects — which in a no-GC, RAII world means re-running `def drop`/re-acquire cycles and reasoning about partial effects under retry, precisely the kind of hidden control flow **P1/P5** push against. Ruxen's shared-mutable-state story is `Mutex`/`Atomic`/`SharedSync` with explicit, owned, scoped locking — *explicit danger* made visible, not optimistic magic. Reject; revisit only if a concrete, ownership-compatible motivating workload appears.

The through-line: ZIO's *guarantees* (typed errors, leak-free-on-cancel, structured lifetimes, backpressure taxonomy) are worth importing; ZIO's *mechanism* (a GC'd effect runtime) is not. Ruxen reaches the same guarantees through ownership + `drop` + the existing reactor.

---

## 3. Ranked recommendations

Ranked by leverage. Structured concurrency and cancellation come first because they are *prerequisites* that unlock the soundness of everything below them (a `Race` that cancels losers, a `Scope` that joins children, a stream that frees its reactor slot on disconnect all depend on cancellation actually running `def drop`).

| # | Feature | Why (1 line) | Wedge | Effort | Differentiator |
|---|---------|--------------|-------|--------|----------------|
| 1 | **First-class cancellation + leak-free `def drop` across `.await`** | Make `mixin Drop` runtime-dispatched + state-indexed drop glue so a cancelled/partial future runs `def drop` — the prerequisite for all structured async | #3, #2 | medium | Guaranteed sync finalizers on async cancel; Rust's `AsyncDrop` is still unstable in 2026 |
| 2 | **Structured concurrency: ownership-owned task scopes (`Scope.open do \|s\| … end`)** | The scope value *owns* its children; its `def drop` joins-then-cancels them, so a detached/leaked task is unrepresentable | #3, #2 | medium | Closes withoutboats' Scoped Task Trilemma for the single-thread case — async-Rust cannot |
| 3 | **General `Race[T]` over N futures (losers cancelled)** | Replaces the bespoke `read_with_timeout` with one combinator; loser-cancel = deterministic `def drop` | #2, #3 | medium | No silent-drop footgun of `tokio::select!`; loser cleanup is structural |
| 4 | **Async backpressured `Queue[T]` (bounded, suspends the task)** | The sync MPSC blocks the OS thread — fatal in rondo's thread-per-core model; need a reactor-aware queue with an overflow policy | #2, #3 | medium | ZIO's overflow taxonomy + Go's no-unbounded discipline, leak-free by ownership |
| 5 | **Async streams (`mixin Stream` over `poll_next`)** | SSE / chunked bodies need "a sequence of chunks over time with backpressure"; pull semantics + `drop` = leak-free-on-disconnect | #3, #2 | medium | Stable named stream + deterministic cancel cleanup; Rust's `AsyncIterator` is still nightly |
| 6 | **Retry / `Schedule` combinators** | No retry surface exists today; every call site hand-rolls a counter+sleep loop | #1, #2 | medium | In-std, drop-correct retry; ZIO ergonomics with no GC/effect monad |
| 7 | **Deterministic virtual `TestClock`** | Time-based async tests need real sleeps today (slow + flaky); virtualize the timer seam | #2, #3 | medium | Single-thread reactor makes deterministic test-time *easy* — the property Swift cannot ship |

---

## 4. Detailed recommendations

### 4.1 First-class cancellation + leak-free `def drop` across `.await`

**Why for Ruxen.** Ruxen's signature promise is deterministic `def drop` at scope exit, no GC, ownership-enforced safety. Async is the one place that promise is currently broken. Three coordinated pieces fix it, all reusing in-tree machinery: (1) upgrade `mixin Drop` to `mixin Drop dispatch runtime` — the exact modifier `mixin Future` already carries — so `def drop` fires polymorphically via a synthesized `Drop_dynamic_drop(ptr)` vtable entry; (2) emit **state-indexed drop glue** on the synthesized `async def` state machine so a partially-driven future drops exactly the fields/sub-futures live at its current `self.__state`; (3) thread a cooperative `Cancelled` bit through `Context` (`cx.cancelled?`) that leaf futures may observe at an await boundary.

The rondo evidence is direct. `mixin Drop` in `library/std/core/src/lib.rx` is today a plain marker with one empty `def drop` and does **not** carry `dispatch runtime` — so `def drop` is not runtime-dispatched. The scheduler concedes the consequence: `ruxen_executor_drain_remaining` ships *without* calling user-defined `def drop` on dropped/cancelled future heap blocks, and its own comment says the futures' OS-side registrations leak per task and to "swap in the call to `Drop_dynamic_drop(future_ptr)` here" once Drop joins the runtime-dispatched set. The drain leak shares its root cause (Drop not runtime-dispatched) with **rondo W17** — a future's class-typed *field* whose `def drop` did not fire on drop, leaking an fd. The hand-written async futures already have correct `def drop`s waiting to be called (e.g. `AsyncReadWithTimeoutFuture` deregisters its reactor timer + frees its read-state); they are simply unreachable polymorphically on cancel/drain.

> **Correction applied.** W17 is precisely a drop/lifetime bug and was the live motivator; it is now fixed in-tree (commit `1b888b8`). **rondo W18** (keep-alive cycles destabilising a worker via reactor-slot churn, fixed by persistent fd registration, `c6f1e2d`) was a *stability* bug, lifecycle-adjacent but **not** a drop bug — do **not** cite it as a current motivating drop bug. Present W17/W18 as evidence of the lifecycle failure *mode* this work must avoid re-introducing, not as live bugs it fixes. Also: the synthesized state machine *does* hoist each cross-suspend local into a `self.<binding>` field, so a parked future owns its sub-futures/resources — but `segment_cfg` performs **no liveness analysis** and stores no live set. It stores per-segment entry statements and the single await binding per suspend. The drop glue must *derive* which `self.<…>` fields are live per state from these; it cannot "reuse an existing live set."

**Design sketch.**

```ruxen
class DbConn
  fd: Int
  def drop                              # guaranteed to fire even on partial-drive cancel
    ruxen_reactor_deregister(ruxen_reactor_current_handle(), self.fd)
    close_fd(self.fd)
  end
end

async def handle(conn: DbConn) -> Result[Response, IoError]
  let req = read_request(conn).await?    # suspend #0: conn hoisted to self.conn
  if cx_cancelled?                       # cooperative early-out (P1 explicit opt-in)
    return Err(IoError.Cancelled)
  end
  let resp = run_query(conn, req).await? # suspend #1
  Ok(resp)
  # If cancelled while parked at suspend #0, the state-machine drop glue
  # runs conn.drop for state==1's live set — fd released, no leak.
end
```

The synthesized future gets a `def drop` that switches on `self.__state` and drops exactly the fields initialized-and-live at that state, in reverse-initialization order (matching sync RAII). `ruxen_executor_drain_remaining` and the `block_on`-exit path then call `Drop_dynamic_drop(future_ptr)` on every still-queued/parked future before freeing the heap block.

**Market impact.** Rust's cancellation model is "drop = silent cancel" with cancel-safety as an *unenforced per-API doc convention* — [Tokio's `select!` docs](https://docs.rs/tokio/latest/tokio/macro.select.html) hand-maintain cancel-safe vs not-cancel-safe lists, and [Oxide RFD 400](https://rfd.shared.oxide.computer/rfd/0400) documents production outages from cancel-incorrectness (lost serial-console bytes, an installinator stuck Invalid, zones left inconsistent). Critically, Rust has **no guaranteed async cleanup on cancel**: [`AsyncDrop` is still unstable](https://doc.rust-lang.org/std/future/trait.AsyncDrop.html) behind `feature(async_drop)` (issue #126482) as of mid-2026, and even it cannot guarantee it runs. Ruxen's win is narrow and real: because finalizers are *synchronous* `def drop` and the state machine guarantees they fire on partial-drive, Ruxen delivers leak-free-on-cancel for the resource class that matters most (fds, reactor slots, buffers) without waiting on a feature Rust hasn't shipped. [ZIO's `acquireRelease`](https://zio.dev/overview/handling-resources/) runs release uninterruptibly and guarantees it even under interruption *including async release* — Ruxen matches the guarantee only for *synchronous* cleanup, via RAII instead of a GC'd effect runtime. Be honest about the limit: Ruxen does **not** solve cancel-safety for logical mid-mutation invariants any better than Rust does; it solves the *resource-leak* half, deterministically.

**Effort & what it reuses.** Medium. Reuses the existing runtime-dispatched mixin path (`Future` already pays the 8-byte class_info cost), the `async def` lowering, and the per-op futures' hand-written `def drop`s. The new work is the `Drop_dynamic_drop` vtable synthesis, the state-indexed drop glue (which must derive per-state liveness — *new* analysis, not a reuse), the scheduler/drain call sites, and the `Context` cancel bit.

**Risks.** (1) Double-drop / drop-order: the Ready path already drops fields via match drop-elaboration; the cancel path must not double-drop — needs a consumed flag per field or a shared discriminant. Highest correctness risk. (2) Partial-init at a suspend: a field set in segment K must not be dropped if cancellation happens at K−1, or it drops uninitialized memory (UB). (3) `ruxen_reactor_current_handle()` must still resolve the right per-thread reactor at drain time; deregister must be a safe no-op if the reactor is already torn down. (4) The 8-byte header on every class with a `def drop` is a broad ABI change — needs a layout-stability pin. (5) `cx.cancelled?` is cooperative — it does not preempt a compute loop with no awaits (same honest limit as Swift/Kotlin/Go); document it. (6) The drop glue is only as complete as the CFG shapes `segment_cfg` accepts; await-in-loop bodies need the live-set per back-edge derived correctly.

---

### 4.2 Structured concurrency: ownership-owned task scopes

**Why for Ruxen.** Ruxen's only async spawn surface today is `Task.spawn_raw(future_ptr)` — explicitly *raw / fire-and-forget / detached, no parent-child lifetime, no cancellation*. rondo spawns exactly one such raw task per accepted connection in its accept loop, with no structured ownership. A `Scope` value that *owns* its children and whose `def drop` joins-then-cancels every child before control leaves the block makes a leaked or detached async task **unrepresentable**. Children may borrow scope-local data because the borrow checker can prove the scope (and its drop-fence) outlives them — RAII at scope exit is exactly the join point the compiler already sees. This is the single feature that lets borrow-check + no-GC + leak-free actually *compose*. It is built as a `.rx` class over the existing thread-local scheduler, `Future` mixin, and reactor — not a new runtime. Cancellation runs each child future's `def drop` (§4.1), which is why §4.1 must land first or co-deliver.

> **Correction applied.** Cite **only W17** (the open-then-fixed drop bug) as the motivating lifecycle bug for scopes — **not W18** (a reactor-slot-churn stability bug, already fixed, not a drop bug).

**Design sketch.**

```ruxen
async def fan_out(urls: &Array[String]) -> Result[Array[Body], FetchError]
  Scope.open do |s|                # s: &var Scope; the Scope owns every child
    var i = 0
    while i < urls.len
      let u = urls.get(i)          # borrow proven to outlive children by the drop-fence
      s.spawn { || fetch(u).await }
      i = i + 1
    end
    s.collect.await                # parent BLOCKS here: joins all children, aggregates
  end                              # <- Scope#drop: any still-pending child is cancelled
end                                #    (its future's def drop fires) before we leave
```

`Scope.open` is fail-fast (first `Err` cancels siblings, propagates); `Scope.supervised` is the named exception (children fail independently, errors aggregated to `Result[_, Array[E]]`) — mirroring Kotlin's `coroutineScope`/`supervisorScope` and Java's `Joiner`. Default is fail-fast (**P3**). Falling off the block end auto-joins (Swift/Trio semantics), so forgetting to `collect` is still safe. `s.spawn` enqueues on the same thread-local scheduler via the existing `ruxen_executor_spawn` and records the handle in the scope's child list; v1 single-reactor scope requires neither `Send` nor `'static` — that is the whole point.

**Market impact.** This is the headline gap in async Rust. [withoutboats' Scoped Task Trilemma](https://without.boats/blog/the-scoped-task-trilemma/) proves async Rust can give at most two of {concurrency, parallelism, borrowing} *because leaks are safe* (`mem::forget`); [`tokio::spawn` detaches and `JoinSet`](https://docs.rs/tokio/latest/tokio/task/struct.JoinSet.html) is `'static`-only + abort-on-drop. [Tyler Mandry](https://tmandry.gitlab.io/blog/posts/2023-03-01-scoped-tasks/) and the [wg-async roadmap](https://rust-lang.github.io/wg-async/vision/roadmap/scopes.html) confirm this is unresolved by design. Ruxen's single-thread scope claims concurrency + borrowing (not cross-thread parallelism in v1) with **no leak escape hatch**, and cancellation runs each child's `def drop` deterministically. Against Crystal — the closest syntactic rival — [structured concurrency (#6468)](https://github.com/crystal-lang/crystal/issues/6468) has been open since 2018, never shipped, with no `Fiber.cancel`, a Boehm GC, and unsafe pointers. Against [Go](https://go.dev/blog/pipelines), `go` is the canonical unstructured spawn and goroutine leaks are documented; Ruxen makes the leak a scope-structure impossibility. The paradigm itself is validated and on-trend — [Swift SE-0304](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0304-structured-concurrency.md), [Python `asyncio.TaskGroup`](https://docs.python.org/3/library/asyncio-task.html), [Java `StructuredTaskScope`](https://openjdk.org/jeps/505) (still in preview as of JDK 25), all echoing [Nathaniel Smith's 2018 thesis](https://vorpus.org/blog/notes-on-structured-concurrency-or-go-statement-considered-harmful/). Be honest: Ruxen does **not** match BEAM/ZIO supervision-tree resilience or distribution. State plainly that v1 is single-thread-per-reactor (no work-stealing) — "structured concurrency *within a worker*," matching rondo's thread-per-core model, a deliberate scoping, not a hidden weakness.

**Effort & what it reuses.** Medium. A `.rx` class over the thread-local scheduler, `Future` mixin, and reactor. No new runtime, no new borrow-check rule (returning `s` is an ordinary borrow-escape error).

**Risks.** (1) **Hard dependency on §4.1.** The cancel guarantee depends on firing children's `def drop` from `Scope#drop`; until runtime-dispatched Drop lands, the scope can drop handle refcounts but not run the futures' cleanup — i.e. it would not actually fix the W17 class. Sequence after, or co-deliver with, §4.1. (2) `s.collect.await` would be an **async method on a class**, which the ground truth says is unsupported today (only top-level free fns lower). *Correction:* the often-cited `Task.join`/`collect_future_outputs` AST-walk gap is now **closed** — `collect_future_outputs_into` walks bootstrap classes (`lower_async_defs_with_bootstrap`), so `Task.join(h).await` lowers and typechecks today (§4.3). The real blocker for `s.collect.await` is that it would be an **async method on a class**, which is not yet lowered (only top-level free fns are). So v1 `Scope` likely exposes a hand-written-future / free-fn `collect` factory rather than an `async def` method. (3) `s.collect` polling a variable-length child list is a `while`-with-`.await` shape; only `segment_cfg`-gated CFG shapes work — needs a pin test that this shape is in the supported set. (4) The raw-`Int` handle ABI lets a child handle be copied out as a bare `Int`, bypassing ownership and breaking the borrow guarantee — that escape hatch must be walled off inside `Scope`. (5) Pre-empt the "why not parallel?" critique by tying v1 to thread-per-core, not general parallelism.

---

### 4.3 General `Race[T]` over N futures

**Why for Ruxen.** Today the *only* race-like primitive is the bespoke `AsyncReadWithTimeoutFuture` — a hand-written 2-way race (read vs per-poll timer) with manual "who owns which reactor registration" bookkeeping. Every new race shape (read-vs-shutdown, accept-vs-timeout, N-way fan-in) currently means copy-pasting that C-state-machine + reactor-juggling pattern. rondo already wants these shapes: per-connection idle timeout (read race shutdown), graceful worker shutdown (accept race stop-flag), replica fan-in. A general `Race` makes `read_with_timeout` a two-line `Race.of(stream.read(n), Async.sleep(d))` derivation and gives rondo a single audited cancellation path. First-to-complete wins; all losers are deterministically cancelled (their `def drop` runs) before the winner's value returns.

**Design sketch.**

```ruxen
async def read_or_timeout(stream: &var AsyncTcpStream, n: Int, ms: Int) -> Result[String, IoError]
  match Race.of(stream.read(n), Async.sleep_ms(ms)).await
    Race.First(bytes) -> bytes               # read won; sleep future dropped
    Race.Second(_)    -> Err(IoError.TimedOut) # timer won; read future dropped
  end
end
```

`Race.of(a, b)` / `Race.any(arr)` are constructors returning a synthesized `RaceFuture` that `include Future` with `type Output = Race2[A,B]` (or `Won[T]` for the array form) — the same "factory returns a Future class" shape `Async.sleep`/`Task.join` already use. `poll(cx)` polls each live child via the exported `Future_dynamic_poll(self, ctx)`; first child `Ready` records the winner, then runs each remaining child's `drop` and returns. Loser-cancel cleanup is synchronous (reactor deregister + free), so no async-drop machinery is needed — RAII already runs it.

> **Correction applied (a claim was refuted).** It is **false** that "async `.await` on a bootstrap-loaded stdlib Future class does not yet resolve its Output." `collect_future_outputs_into` *does* walk bootstrap classes (via `lower_async_defs_with_bootstrap`), and `Class.static_method(...).await` on a stdlib Future (e.g. `Async.sleep(d).await`) **resolves today** — `731_class_static_call_await` is a *passing* pin, no longer deferred. The real residual gaps a `Race` must still respect: (a) async **methods on classes** are still not lowered (only top-level free fns); and (b) `Task.join(h).await` **does resolve, lower, and typecheck today** — the bootstrap-aware lowering landed, and the test `task_join_await_via_method_call_lowers` (`compiler/ruxen_core/tests/task_scheduler.rs:484`) asserts the synth state machine carries a `__sub_*: TaskJoinFuture` field and typechecks clean — but only as `Poll[Int]`, because `TaskJoinFuture`'s `type Output = Int` is hardcoded (`library/std/future/src/task_join_future.rx:40`) and typed `TaskHandle[T]` / generic-output recovery is deferred. So the *conclusion* shifts: a `Race.of` over concrete arms `.await`s fine; a fully generic-`Output` `Race` inherits the same generic-output-recovery limitation `TaskJoinFuture` has (the Output must currently resolve to a concrete type) — **not** a "cannot resolve `.await`" problem. Both the old reason (bootstrap classes invisible to the awaitee tables) and the "still cannot resolve" framing were wrong; note the `task_join_future.rx` header comment is itself **stale** on this point and should not be trusted over the passing test.

**Market impact.** [`tokio::select!`](https://docs.rs/tokio/latest/tokio/macro.select.html) is the canonical "footgun factory" — losing branches are silently dropped and cancel-safety is unenforced ([Oxide RFD 400](https://rfd.shared.oxide.computer/rfd/0400) documents the outages). Ruxen's edge is not "we have select!" — it is that loser-cancel runs deterministic `drop` finalizers; the children need only sync cleanup (reactor deregister + free), so the most-cited async-Rust footgun is structurally absent. [Go's `select`](https://pkg.go.dev/context) only multiplexes channel ops and does **not** cancel unselected goroutines (the "forgotten sender" leak). [ZIO `race`](https://zio.dev/reference/interruption/) interrupts the loser with guaranteed finalizers — the gold standard — but pays with a GC'd effect runtime; Ruxen reaches the same loser-cleanup guarantee through plain ownership + drop. Crystal has no composable race-with-loser-cancel at all (no stable `Fiber.cancel`).

**Effort & what it reuses.** Medium. Reuses async-lowering, the `Future` vtable, `Future_dynamic_poll`, the raw-`Int` child-pointer ABI, and the reactor's existing multiplexing of multiple registrations on one `park_current`. **No new reactor primitive.**

**Risks.** (1) The 2-arm `Race.of` is straightforward; the N-way `Race.any(arr)` needs polling a runtime-length array of futures (per-element generic dispatch + `Array[future]` storage) — ship `Race.of` first, `Race.any` second. (2) `.await` resolving `RaceFuture`'s Output is fine for `Race.of` over concrete arms; a fully generic-`Output` `Race` inherits the same generic-output-recovery limitation as `TaskJoinFuture` (its `Output` is currently pinned to a concrete type, `task_join_future.rx:40`) — a typed-generic-output gap, **not** a `.await` resolution problem. (3) Drop-ordering under partial completion is the W17 risk re-incarnated: a loser with a class-typed FFI-bound `def drop` field must fire that nested drop — gate on / co-validate §4.1. (4) Fairness: document a fixed left-to-right poll order (matches `read_with_timeout`), do not claim fairness. (5) Children borrowing non-`'static` parent stack data sit in the "concurrency + borrowing, no cross-thread parallelism" corner (single-reactor poll) — state this; do not promise parallel race across threads.

---

### 4.4 Async backpressured `Queue[T]`

**Why for Ruxen.** The only channel in-tree is the synchronous MPSC whose `recv` **blocks the OS thread**. In rondo's thread-per-core model that is catastrophic: blocking the thread blocks the whole per-core reactor, stalling every other task on that core. rondo spawns `Task.spawn_raw` per connection with no async-aware way to hand work from an acceptor to worker tasks or to push backpressure to a producer. A reactor-aware bounded `Queue[T]` that *parks the consumer through the reactor* (not the OS thread) is the missing piece for thread-per-core work distribution and producer/consumer pipelines.

> **Correction applied.** The load-bearing claims (sync MPSC only, blocking `recv`, no async channel) are correct and verified. But the *implementation-level specifics* are **uncited / unverifiable** from ground truth and should be stated as plausible-but-uncited rather than fact: the exact file paths, a C backing file, `pthread_cond_wait`, `channel.c` line numbers, `sender_clone`/`sender_drop`, and — importantly — a *cross-thread* reactor wake primitive (`eventfd`/`EVFILT_USER`, "Safe to call from any thread"). Ground truth describes the executor as **single-thread / per-reactor**, so a documented cross-thread wake primitive is an *additional assertion not evidenced* and the "route a wake to a remote-core reactor handle" design is **forward-looking, not an existing capability**. Also: given W17, "deterministic drop ⇒ leak-free" is the design *intent/goal*, not a settled property.

**Design sketch.** One bounded `Queue[T: Send]` split into `AsyncSender`/`AsyncReceiver` ends (mirrors the existing `Sender`/`Receiver` split so the borrow checker tracks producer and consumer separately). Capacity is mandatory; **no unbounded constructor** (Go's deliberate choice). The overflow policy is the ZIO taxonomy as a named enum:

```ruxen
enum Overflow
  Suspend     # producer parks until space frees (backpressure)
  DropOldest  # evict head, enqueue new          (sliding)
  DropNewest  # reject incoming, keep buffer      (dropping)
end

let (tx, rx) = Queue[Request].bounded(256, Overflow.Suspend)

async def worker(rx: &var AsyncReceiver[Request])
  loop do
    let job = rx.recv.await?    # parks the TASK via the reactor, not the OS thread
    handle(job)
  end
end
```

`send`/`recv` are future-returning factories shaped like `AsyncReadWithTimeoutFuture` (`state_ptr`/`handle`/`done` + `include Future` + `def var poll` + `def drop`). On would-block, `poll` registers on the queue's persistent wake slot and returns `Poll.Pending`; the peer's enqueue/dequeue fires a wake. Each end carries `def drop`: dropping the **last** `AsyncSender` marks the queue send-closed so a parked `recv` resolves `Err(QueueClosed)` (solves Go's abandoned-receiver leak); dropping `AsyncReceiver` resolves a parked `Suspend`-send as `Err(QueueClosed)` (Go's forgotten-sender leak).

**Market impact.** [Tokio documents `unbounded_channel` as a memory/OOM hazard](https://docs.rs/tokio/latest/tokio/sync/mpsc/fn.unbounded_channel.html) and `tx.send().await` under a timeout as **not cancel-safe** (silent message loss); Ruxen ships no unbounded constructor and makes the only "cancel" a deterministic drop that runs `def drop`. [Go deliberately has only fixed-capacity channels](https://go.dev/blog/pipelines) but leaks goroutines on forgotten-sender/abandoned-receiver; Ruxen's last-owner `def drop` makes that close automatic and compiler-tracked. *Correction:* the "Go *deliberately* ships no unbounded channel to steer users away from leaks" framing is interpretive editorializing — the *fact* (Go has only fixed-capacity `make` buffers) is correct; the *intent* attribution is not documented. [ZIO Queue](https://zio.dev/reference/concurrency/queue/) exposes exactly three **bounded** strategies — back-pressure (suspend), dropping (drop-newest), sliding (drop-oldest) — plus an *unbounded* variant; the "exactly three" count is accurate only as the bounded count, which this design scopes correctly. [Kotlin's Channel](https://kotlinlang.org/api/kotlinx.coroutines/kotlinx-coroutines-core/kotlinx.coroutines.channels/-channel/) exposes the same axis via capacity + [`BufferOverflow`](https://github.com/Kotlin/kotlinx.coroutines/blob/master/kotlinx-coroutines-core/common/src/channels/BufferOverflow.kt). Against Crystal (Boehm GC, no typed backpressure-policy choice, no stable `Fiber.cancel`), the typed bounded queue with explicit overflow and leak-free close is a clean win. Defensible one-liner: *"tokio's bounded mpsc semantics and ZIO's overflow taxonomy, with Go's no-unbounded discipline, made leak-free by ownership instead of GC — in Ruby syntax."*

**Effort & what it reuses.** Medium. Reuses the per-op future shape, the `Future` mixin, the reactor's `set_waker`/`park_current`, and the `SharedSync`-style atomic refcount in a C control block.

**Risks.** (1) Async methods on classes are not lowered, so `send`/`recv` must be hand-written futures, not `async def` methods; `recv`-in-a-loop must match a `segment_cfg`-gated CFG shape or fall back to hand-poll. (2) Wake ordering: set the waker *before* returning `Pending` to avoid stranding a parked task. (3) **Multi-core is genuinely new plumbing** — rondo runs a reactor per core, so an `AsyncSender` on core A waking an `AsyncReceiver` on core B needs a cross-thread wake routed to the *right* reactor handle; current futures assume `current_handle`/single reactor, and a cross-thread wake primitive is *not* an evidenced in-tree capability. This is the largest unknown. (4) Close/refcount races: the sender-refcount decrement + close signal must be atomic against a concurrent parked `recv`. (5) `Task.spawn_raw` is detached — a leaked task holding an end never drops it; this is the boundary where the primitive's leak-freedom stops, which §4.2 (scoped tasks) closes.

---

### 4.5 Async streams (`mixin Stream`)

**Why for Ruxen.** Ruxen has a sync `Enumerable[T]` combinator surface defined over `each`, plus a working reactor + `Future` mixin, but **no async streaming**: every async source resolves to exactly one value (`AsyncReadFuture -> Result[String, IoError]`, `Async.sleep -> nil`). rondo cannot express a body that yields N chunks over time without hand-rolling a poll loop per route. SSE / chunked-transfer responses and streaming request bodies are the canonical rondo need, and there is no type for "a sequence of byte chunks that arrives asynchronously with backpressure." A pull-based `mixin Stream` defined over one primitive `def var poll_next(cx) -> Poll[Option[T]]`, with `map`/`filter`/`take`/`for_each` as default bodies — exactly mirroring `Enumerable[T]`/`each` — fills it. Backpressure is intrinsic to pull semantics; resource safety comes from ownership + `def drop` (a stream owns its reactor registration / `state_ptr` and tears it down at scope exit, including mid-stream cancellation on client disconnect).

> **Correction applied.** W17 is specifically about an FFI-bound `def drop` on a *class-typed field* of a future not firing (drop-elaboration walking field types) — **not** about timer-slot teardown. W18 was a reactor-slot-churn crash fixed by *persistent fd registration*, **not** by a per-future drop. The `AsyncReadWithTimeoutFuture` drop pattern is real and relevant, but it addresses W17's field-drop class only insofar as drop elaboration fires; its timer-deregister-on-drop is its own timeout-future-specific mechanism. Do not conflate the two fixes under "exact bug class."

**Design sketch.**

```ruxen
mixin Stream dispatch runtime
  type Item
  def var poll_next(cx: &var Context) -> Poll[Option[Self.Item]]  # the one required primitive
  def map[U](f: any Fn[Fn(Self.Item) -> U]) -> MapStream[Self, U]  # default bodies, built on poll_next
  def take(n: Int) -> TakeStream[Self]
end

async def serve(stream: &var AsyncTcpStream, path: &String) -> Result[Int, IoError]
  let body = AsyncFile.chunks(path, 16384)?    # ChunkStream, Item = String
  var sent = 0
  for chunk in body.take(1000) do |c|          # `for ... in` = await poll_next per item
    sent = sent + stream.write(c).await?       # backpressure: next chunk pulled only after write resolves
  end
  Ok(sent)
  # body (and its reactor slot) dropped here — or on early return / client disconnect.
end
```

An adapter owns its inner stream by value, so dropping the chain drops every stage, each running its own `def drop`. The `for x in stream` driver desugars to a `loop` around `match poll_next(cx).await` with the arms `Some(x) -> body` and `nil -> break`.

**Market impact.** [Rust's `AsyncIterator`/`Stream`](https://doc.rust-lang.org/std/async_iter/trait.AsyncIterator.html) is **not** stabilized in std (nightly-only, `feature(async_iterator)`, issue #79024); the ecosystem leans on [`futures::Stream`](https://rust-lang.github.io/rfcs/2996-async-iterator.html) + [`async-stream`](https://github.com/tokio-rs/async-stream) as a temporary syntax, and the [`async fn next` vs `poll_next` debate](https://blog.yoshuawuyts.com/async-iterator-trait/) is unresolved. Ruxen can ship a stable, named `Stream` mixin with no `Pin` in the user signature, plus the cancel story Rust lacks (drop a stream on disconnect → `def drop` tears down the fd registration; Rust's `AsyncDrop` is still unstable). Against [Go](https://go.dev/blog/pipelines), a downstream that stops reading leaks the producing goroutine; Ruxen's pull model means the upstream is simply never polled again and is dropped with its owner. [ZIO `ZStream`](https://zio.dev/reference/stream/) is the gold standard for pull-based + scoped resource-safe streams, but runs on the JVM with GC + an effect monad; Ruxen offers the same pull-backpressure + scoped-cleanup as native, no-GC code. Against Crystal (Boehm GC, unsafe pointers, no borrow checker), "resource-safe streaming without a GC" is a clean, true claim. **Not** claimed: ZIO's operator catalog, concurrent `merge`/`mapPar`, or [Reactive-Streams demand-protocol interop](https://www.reactive-streams.org/) — later work. v1 is single-consumer pull streams with leak-free cancel.

**Effort & what it reuses.** Medium. One new mixin file + source/adapter classes, each following the established `include Future` per-op pattern. Reuses the reactor's persistent read registration and the `Enumerable` default-body shape.

**Risks.** (1) **Biggest:** `async def` *methods on classes* are not lowered (only top-level free fns) — the ergonomic `for chunk in stream` inside an `async def serve` on a router class depends on method-level async lowering landing. *Correction:* this gap is real, but the tests `728g_async_instance_method_await`/`729_async_method_await` are the **wrong evidence** — they test awaitee Shape-3 (`<obj>.method(args).await` where the receiver returns a Future), a *supported, passing* path landed in B2b; they do **not** test defining an `async def` method on a class. (2) The `for x in stream` driver desugars to `.await`-in-loop inside `match`/`loop` — only `segment_cfg`-gated shapes pass; prove the `Some/None/break` shape fits or widen `segment_cfg`. (3) Generic adapters over a mixin-typed inner stream need class-generics-over-a-mixin-bound to monomorphize per implementor (Q17 fixed it for generic free fns; verify it holds for class generics). (4) `Stream dispatch runtime` adds the 8-byte header like `Future`. (5) Resist concurrent stages (`mapPar`/`merge`) in v1 — that reintroduces the scoped-task trilemma.

---

### 4.6 Retry / `Schedule` combinators

**Why for Ruxen.** Ruxen has `Result`/`?` and a working reactor but **zero retry surface** — every fallible network call hand-rolls `var attempts = 0; loop { …; Async.sleep(d) }`. A composable immutable `Schedule` value (recurs / exponential / jittered / `while_input` / `and_then`) plus two drivers — `.retry`/`run do … end` on `Result` (sync) and `Async.retry(sched) do … end` for futures (async) — removes the boilerplate and gives connection/upstream handling a single drop-correct retry path. This compounds rondo's lifecycle concern: ad-hoc retry loops each re-implement the sleep-future lifecycle and can mishandle drop-on-cancel.

> **Correction applied.** W17 and W18 are real and were genuine async-lifecycle bugs, but **both are already fixed in-tree** (W17 = `1b888b8`, W18 = `c6f1e2d`). Present them as evidence of the failure *mode* the combinator's centralized drop must avoid re-introducing — **not** as live bugs it fixes. Also: async methods on classes are *grammatically permitted* but **not yet lowered** (only top-level free fns are) — phrase the constraint as "not yet lowered/implemented," not "not allowed by the grammar."

**Design sketch.**

```ruxen
let conn = Schedule.exponential(Duration.from_millis(50))
             .max_attempts(5)
             .max_delay(Duration.from_secs(2))
             .jittered                       # full-jitter: random in [0, delay]
             .while_input do |e| e.transient? end
             .run do
               Database.connect(url)          # re-evaluated each attempt
             end?

let resp = Async.retry(sched) do
             http.get(url).await
           end.await?
```

`Async.retry` returns a hand-written `RetryFuture` (`include Future`) holding the current inner future, the `Schedule`, the attempt counter, and a child `TimeSleepFuture` during backoff. Its `def drop` drops whichever child is live, so a cancelled retry tears down the in-flight backoff timer deterministically.

**Market impact.** Retry/backoff as a composable value is **settled industry consensus**, not novel: [ZIO `Schedule`](https://zio.dev/reference/schedule/), [resilience4j](https://resilience4j.readme.io/docs/retry), [.NET Polly](https://www.pollydocs.org/strategies/retry.html), and [tokio-retry](https://docs.rs/tokio-retry/latest/tokio_retry/strategy/struct.ExponentialBackoff.html) all converge on bounded attempts + exponential + max-delay cap + predicate + jitter; [full-jitter is the AWS/Brooker (2015) default](https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/). So Ruxen is matching table stakes — the differentiation is *who ships it and how it reads*. Crystal ships **no** first-party retry/Schedule in stdlib (third-party shards); Ruxen in-std with a Ruby-ish surface is "batteries included where Crystal isn't." Against async Rust, [the canonical tokio-retry crate is lightly maintained](https://crates.io/crates/tokio-retry2) (active fork `tokio-retry2`), and its drop-to-cancel model cannot guarantee async cleanup ([`AsyncDrop` unstable](https://doc.rust-lang.org/std/future/trait.AsyncDrop.html)); Ruxen's `def drop` deterministically tears down a `RetryFuture`'s child timer on mid-backoff cancel. ZIO `Schedule` is the high-water-mark algebra (union/intersect, retry-on-error unified with repeat-on-success) — Ruxen ships a deliberately small subset (`and_then` only, no union/intersection, no repeat-on-success) and compiles native with no GC. This is a **credibility/completeness** feature, not a standalone wedge-mover.

**Effort & what it reuses.** Medium. Pure `.rx` over existing pieces: closures (`any Fn[Fn(A)->B]`, called `f.(x)`), `Duration`, `Rand.random_u64`, `Async.sleep -> TimeSleepFuture`, and the `include Future` + `def drop` pattern. No compiler edit, no new FFI.

**Risks.** (1) `Async.retry` must be a free fn / namespace static returning a hand-written `RetryFuture`, not an `async def` method (not yet lowered). (2) `.await`-in-loop is `segment_cfg`-gated, so the async driver must be a hand-written poll state machine, not a natural async loop — the main reason effort is medium not small. (3) Stored closures must be `Fn` (re-invokable per attempt) and `Send` for the async path; the body cannot move-capture a value it needs again next attempt. (4) Cancel-correctness depends on the `RetryFuture`'s `def drop` dropping exactly the one live child — a focused drop-mid-backoff / drop-mid-inner-poll test is required. (5) **No virtual test clock exists** (the `time` module is `Duration`/`Instant` shells); testing backoff timing deterministically is hard without real sleeps — see §4.7, which is a prerequisite/follow-up, not a bundle. (6) `Rand.random_u64` jitter is fine (decorrelation, not crypto) — a one-line doc note.

---

### 4.7 Deterministic virtual `TestClock`

**Why for Ruxen.** Testing anything time-based in rondo (keep-alive idle timeout, `read_with_timeout`) currently requires **real sleeps** — slow and flaky, the exact rationale Tokio/ZIO/Kotlin cite. A `--test`-gated mode where the **per-thread** reactor swaps its real OS-timer for an in-memory virtual clock with an ordered timer queue — exposed as `TestClock.advance`, `TestClock.set`, and auto-advance-to-next-deadline — makes timeouts/retries/`Async.sleep` resolve instantly and deterministically. The reactor is per-thread (`_Thread_local RuxenReactor *t_current_reactor`, `reactor.c:148`) and **persists across `block_on` calls** — `ruxen_executor_context_drop` intentionally does *not* release it (`executor.c:197`, to avoid a kqueue/epoll create+destroy pair per `block_on` under load). So there is one stable per-thread object to host the virtual clock; a test installs and restores virtual time explicitly via the `TestClock` install guard (its `def drop` restores the real clock), since the reactor is not torn down between `block_on`s on a thread. This turns "register a 30s timeout, advance 30s, assert the fd was deregistered via `def drop`" into a millisecond unit test — directly hardening the rondo flagship.

> **Correction applied (a claim was refuted).** The design premise that virtualizing **one** function covers both async and sync time is **false** against current source. `ruxen_time_now_ns` backs **only** the async sleep deadline (read in `time_sleep_future.rx`). Sync `Instant.now`/`Instant.elapsed` sample `clock_gettime(CLOCK_MONOTONIC)` **independently** and never route through `ruxen_time_now_ns` — so virtualizing it alone would **not** affect sync `Instant` time inside a test. You must *additionally* virtualize `ruxen_instant_now`/`ruxen_instant_elapsed` (or refactor them to call `ruxen_time_now_ns` first). The in-repo comment claiming `ruxen_time_now_ns` "backs `ruxen_instant_now`" is itself inaccurate to the C body. The "no compiler changes" headline still holds (this is C-runtime + one `.rx` namespace class), but the *scope* of the C-runtime work is larger than "virtualize one function."

**Design sketch.**

```ruxen
class TestClock
  lib "runtime/test_clock.c"
    def self.install as "ruxen_test_clock_install" -> TestClock   # guard; def drop restores real clock
    def advance as "ruxen_test_clock_advance"(d: &Duration)        # fire due timers in deadline order
    def advance_to_next as "ruxen_test_clock_advance_to_next" -> Option[Duration]
  end
  def drop
    ruxen_test_clock_uninstall()
  end
end
```

Mechanism: a per-reactor `virtual_mode` flag + `virtual_now_ns` + a sorted pending-timer list. In virtual mode, `register_timer` inserts into the list instead of arming a timerfd/kevent (same handle ABI, so `TimeSleepFuture` is unchanged); `advance(d)` bumps `virtual_now_ns`, pops every timer with `deadline <= now` in order and wakes its parked task; `park_current` returns immediately rather than blocking on `epoll_wait(-1)`. **Only the timer + clock seam is virtualized** — real epoll/kqueue fd readiness is untouched, so a test mixing real sockets + virtual time gets real I/O wakeups and virtual timer wakeups (matches Tokio's `pause()` affecting only time).

**Market impact.** Virtual-time async testing is **table stakes**: [Tokio `time::pause`/`advance`/`start_paused`](https://tokio.rs/tokio/topics/testing) (stable, `test-util`), [ZIO `TestClock.adjust`/`setTime`](https://zio.dev/reference/test/services/clock/) (default test env), [Kotlin `runTest`](https://github.com/Kotlin/kotlinx.coroutines/blob/master/kotlinx-coroutines-test/README.md) virtual time. *Correction:* Kotlin `runTest`/`TestScope` was **introduced** (experimental) in kotlinx-coroutines 1.6.0 and reached **stable in 1.7.0** — do not write "stable since 1.6.0." Not having a test clock is a visible gap a Rubyist-evaluator (used to RSpec time stubbing) hits immediately. Against Crystal (no built-in virtual test clock, multicore still preview), "fast deterministic async time tests out of the box" is a clean tooling win. [Swift](https://github.com/swiftlang/swift-evolution/blob/main/proposals/0329-clock-instant-duration.md) standardized the `Clock` protocol but ships **no first-party test clock** — teams use [Point-Free's swift-clocks](https://github.com/pointfreeco/swift-clocks). *Correction:* the supported fact is "no first-party Swift test clock"; the embellishment that this "remains an open 2025 Swift Forums discussion blocked on async-scheduler nondeterminism" is **unsubstantiated** (the located threads are 2021–2022 reviews), and "single-threaded reactor makes it straightforward" is a design opinion, not a verifiable fact — frame both as our assessment, not established fact. The honest pitch: Ruxen's single-threaded, per-thread reactor *has none of the multi-thread scheduler nondeterminism that complicates a deterministic clock*, so the clock is tractable here — laddering to the STRATEGY great-tooling / "Cargo-bar" pillar.

**Effort & what it reuses.** Medium. C-runtime + one `.rx` namespace class. **No** compiler / async-lowering / `block_on`-rewriter changes. Reuses the per-thread reactor object, the timer handle ABI, and `ruxen_executor_wake_task`.

**Risks.** (1) The `ruxen_time_now_ns` indirection is a hot-path read — must compile to a single thread-local NULL-check or be absent in non-test builds; verify zero release overhead. (2) **Per the refuted claim**, covering sync `Instant` time *also* requires intercepting `ruxen_instant_now`/`ruxen_instant_elapsed` — larger C work than "one function." (3) Hybrid correctness: keep real I/O wakeups working while virtualizing only the timer seam (get `park_current`'s virtual branch wrong → busy-spin or deadlock). (4) Auto-advance must reuse the reactor's existing queue-empty / no-real-registration predicates, not a parallel notion, or it fires while real work is pending. (5) Single-thread-only by construction — document it as matching the rest of the executor's v1 contract; revisit when multi-threaded scheduling lands. (6) Scope creep: the clock is the substrate; the retry/Schedule library (§4.6) is what it makes testable — keep them separate.

---

## 5. Market impact summary

The through-line is one defensible sentence: **"fearless *structured* concurrency, with no GC, that reads like Ruby."** No shipping language occupies that slot:

- **Crystal** — the closest syntactic rival — has a [Boehm GC](https://crystal-lang.org/reference/1.19/guides/concurrency.html), [no borrow checker and unsafe raw pointers](https://crystal-lang.org/reference/1.19/syntax_and_semantics/unsafe.html), [multicore still a preview flag in 1.20 (2026)](https://crystal-lang.org/2026/04/16/1.20.0-released/), [no stable `Fiber.cancel`, and structured concurrency open since 2018 (#6468)](https://github.com/crystal-lang/crystal/issues/6468). Ruxen wins on three independent axes: no GC, borrow-checked safety, and enforced structured concurrency.
- **async-Rust** has the safety pedigree but provably **cannot** offer ergonomic scoped async — [the Scoped Task Trilemma](https://without.boats/blog/the-scoped-task-trilemma/) is a soundness consequence of leaks being safe, and [the 4-year plan](https://without.boats/blog/a-four-year-plan/) does not propose structured concurrency as the default. [`AsyncDrop` remains unstable](https://doc.rust-lang.org/std/future/trait.AsyncDrop.html), so Rust has no guaranteed cleanup on cancel. Ruxen's leak-free-on-cancel-by-RAII is a capability Rust does not have stable.
- **Go** ([`select`/channels](https://go.dev/blog/pipelines)) and **Elixir/BEAM** solve task-leak and resilience *at runtime* via GC + isolation + supervision — a different tradeoff axis. Ruxen's edge is compile-time guarantees + no GC + predictable latency. Be honest: **Ruxen will not match BEAM's supervision trees or hot-reload, nor displace Elixir for distributed always-up systems.** The niche is "BEAM-like structured/leak-free concurrency with C-like performance and no GC," not "kill Elixir."

These features make the **rondo flagship a credible Wedge #3 story**: a thread-per-core web framework whose connection handlers are scoped (no detached tasks), whose timeouts and shutdowns are general `Race`s (no bespoke per-shape futures), whose work distribution is a backpressured `Queue` (no thread-blocking), whose SSE/chunked bodies are `Stream`s that free their fd on disconnect, whose upstream calls retry with drop-correct `Schedule`s, and whose time-based behavior is tested deterministically — all enforced by ownership, all without a GC.

**The honest size of the slice.** Per `docs/STRATEGY.md`, the target user is *"senior Rubyists who've done some Rust, hated the syntax, but loved the guarantees"* — a real but **Crystal-sized** slice, not Go-sized. These concurrency features do not widen that audience; they make Ruxen's *one owned matrix row* (Ruby syntax + no GC + borrow checker + AOT) credible on the concurrency axis specifically, so that when a Rubyist evaluates rondo the async story holds up instead of revealing `Task.spawn_raw` and a single bespoke timeout future. They are a *retention/credibility* investment in the wedges already locked, not a new wedge.

---

## 6. Sequencing & open questions

**Build order** (each later item depends on earlier ones for *soundness*, not just convenience):

1. **§4.1 Cancellation + runtime-dispatched `Drop` + state-indexed drop glue.** The foundation. Until `def drop` fires polymorphically on cancel/drain, *every* structured-async guarantee below is hollow (the scope drops handle refcounts but not the futures' cleanup). Co-deliver, or land immediately before, anything else here.
2. **§4.2 Structured `Scope`.** The keystone differentiator; unlocks the "detached task is unrepresentable" claim. Blocked on (1) and on the async-methods-on-classes decision (see below).
3. **§4.3 `Race.of`** (2-arm first), then **`Race.any`.** Cheap once (1) lands; immediately retires the bespoke `read_with_timeout`.
4. **§4.4 Async `Queue`** and **§4.5 `Stream`** — parallelizable. `Queue`'s multi-core wake routing is the largest genuine unknown; `Stream` is gated on async methods on classes for the ergonomic form.
5. **§4.7 `TestClock`** — independent of the compiler work; can land early to make (6) testable, but its C scope is larger than first stated (see §4.7 correction).
6. **§4.6 Retry/`Schedule`** — last; rides everything above and is best validated with (5) in place.

**Open questions requiring a maintainer decision:**

- **Async methods on classes.** Multiple features (`Scope.collect`, ergonomic `Stream` handlers, async `Queue` ends written as methods) want `async def` *methods*, which are **not yet lowered** (only top-level free fns are). Decide whether to (a) prioritize method-level async lowering as an enabling investment, or (b) ship all of these as hand-written-future / free-fn factory surfaces (the `Async.sleep` / `Task.join` precedent — a free fn or static returns a hand-written `Future` class, which `.await`s fine; only *defining* `async def` on a class is blocked). This single decision gates the *ergonomics* of items 2, 4, and 5.
- **`segment_cfg` widening.** The `.await`-in-loop shapes needed by `Stream`'s `for` driver, `Scope.collect`'s poll-all-children loop, and `Queue.recv`-in-a-loop must each be proven to fit the gated CFG set, or `segment_cfg` widened. Decide whether to widen proactively or add per-feature pin tests as each lands.
- **Raw-`Int` handle ABI.** `Task.spawn_raw` and the future-pointer ABI expose handles as bare `Int`, which lets a child handle escape ownership and break the `Scope` borrow guarantee. Decide whether to wall this off inside `Scope`/`Race` or tighten the ABI.
- **Cross-thread reactor wake.** The async `Queue` multi-core design assumes a cross-thread wake routed to a specific (possibly remote-core) reactor handle. Ground truth describes the executor as single-thread/per-reactor and does **not** evidence such a primitive. Decide whether v1 async `Queue` is single-reactor-only (matching `Task.spawn_raw`) with multi-core deferred, or whether the cross-thread wake is in scope now.
- **`Scope` fail-fast vs supervised default.** Confirm fail-fast is the default (per **P3**) with `Scope.supervised` as the named exception, mirroring Kotlin/Java — or decide a different default.
- **Drop-elaboration / liveness.** The state-indexed drop glue must *derive* per-state liveness from `segment_cfg`'s per-segment entry statements + await bindings (no live set exists today). Decide whether this analysis lives in async-lowering or extends the existing drop-elaboration pass (`docs/decisions/drop-elaboration.md`).
