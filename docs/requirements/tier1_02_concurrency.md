# Tier 1 Requirement 02 — Concurrency

**Status:** Draft — design proposal
**Target phase:** Post-v0.1 (sits on top of the existing borrow checker and trait
system). Prerequisites: stable closure codegen, stable trait resolution, working
FFI link-flag plumbing.
**Related design principles:** P1 (Implicit Safety, Explicit Danger),
P3 (One Obvious Path), P4 (Own What You Use), P5 (Clarity At The Boundaries).

---

## 1. Summary & Motivation

A language that markets "Rust-flavored safety with Ruby ergonomics" must ship
compile-time-checked concurrency. Shared-memory data races are the single biggest
class of bugs that ownership + borrow checking prevents *in Rust*, but Riven
does not yet enforce any of the cross-thread checks that make Rust's story
coherent. Today Riven has:

- Ownership, borrow checking, move semantics (single-threaded only).
- No thread primitives in the runtime.
- Reserved keywords (`spawn`, `actor`, `send`, `receive`, `async`, `await`) that
  the parser does not yet consume.
- No `Send`, `Sync`, `Mutex`, `Arc`, channel, or atomic in the standard library.

The goal of this document is to specify **Phase 2 Concurrency** — a pragmatic,
Rust-inspired subset that gives Riven *fearless concurrency* using native OS
threads, with opt-in message passing via channels. It deliberately defers green
threads / async-await / actors to a later phase (though reserves syntax for
them).

We pick "Rust-style `Send`/`Sync` auto-traits over OS threads" over the
alternatives because:

| Option | Pro | Con |
|-------|-----|-----|
| **Rust-style (chosen)** | Well-trodden, composes cleanly with existing ownership. | Two auto-traits instead of one. |
| Swift `Sendable` only | Simpler mental model (one marker). | Loses `Sync` nuance — forces `Mutex` around any shared read. |
| Go goroutines + channels only | Ergonomic. | No compile-time race detection — contradicts Riven's "safety by default". |
| Pony ref capabilities | Strongest static guarantees. | Huge learning curve — breaks P3 (One Obvious Path). |
| Ruby Ractor / actor-only | Matches Riven's Ruby heritage. | Data-copy on every message — violates P4 (Own What You Use). |

Rust's model already composes with everything Riven has (ownership, traits,
generics, closures with move semantics). Extending it costs the least new
machinery and preserves the mental model users who came to Riven from Rust will
have. Ruby-style surface syntax (`Thread.spawn do ... end`) keeps the ergonomics.

---

## 2. Current State (the gap)

### 2.1 Types (`crates/riven-core/src/hir/types.rs`)

`Ty` has **no thread-safety markers**. `is_copy()` at line 189 is the only auto-trait
we currently synthesize; no equivalent `is_send()` / `is_sync()` exists.

Smart-pointer types (`Arc`, `Rc`) are absent from the `Ty` enum. The only shared
reference mechanism is `&T` / `&mut T` at lines 88-95, which rely on lexical
borrow scopes that cannot cross a thread boundary.

### 2.2 Traits (`crates/riven-core/src/resolve/mod.rs`, lines 138–170)

Built-in traits currently registered: `Displayable`, `Error`, `Comparable`,
`Hashable`, `Iterable`, `Iterator`, `FromIterator`, `Copy`, `Clone`, `Debug`,
`Drop`. **No `Send`. No `Sync`.**

`TraitResolver` (`crates/riven-core/src/typeck/traits.rs`) supports:
- Nominal satisfaction via `impl Trait for Type` (line 68).
- Structural satisfaction via method-name matching (line 112).

It has **no notion of auto-traits** — traits whose impl is inferred structurally
from a type's fields rather than from its method set. Auto-traits are the core
new mechanic we must add.

### 2.3 Borrow checker (`crates/riven-core/src/borrow_check/`)

`BorrowChecker::check_closure` (lines 949-998) handles move vs. borrow captures
but has **no notion of thread-crossing**. Today, capturing a non-`Send`
value into a closure that gets passed to a hypothetical `Thread::spawn` would
type-check without error. Error codes E1001–E1010
(`borrow_check/errors.rs`) cover single-threaded ownership only.

### 2.4 Runtime (`crates/riven-core/runtime/runtime.c`)

Exposes only: `printf` wrappers, `malloc`/`free`/`realloc`, string ops, a
single-threaded `RivenVec`, Option/Result helpers, and `riven_panic`.

**No `pthread_*`, no atomics, no TLS.** The linker invocation
(`codegen/object.rs` line 64) does `cc <obj> <runtime.o> -o <out> -lc -lm` —
pthread is not linked.

### 2.5 Parser & lexer

`crates/riven-core/src/lexer/token.rs` already reserves the keywords
`Actor` (line 127), `Spawn` (line 128), `Send` (line 129), `Receive` (line 130),
`Async` (line 84), `Await` (line 85). None of them are consumed by the parser
today (`grep` across `crates/riven-core/src/parser` returns zero hits for these
token kinds). **No surface syntax is committed yet** — we are free to design.

### 2.6 Codegen

`compile_runtime` hard-codes `cc -c runtime.c -O2` and does not pass any
thread/atomic flags. Cranelift emits single-threaded code. There is no thread
specific intrinsic support anywhere in the pipeline.

---

## 3. Goals & Non-Goals

### 3.1 Goals

- **G1.** Introduce `Send` and `Sync` as built-in auto-traits with
  compiler-inferred impls, opt-out support, and manual implementation escape
  hatch (behind `unsafe`).
- **G2.** Ship OS-thread-based concurrency (1:1 threading model) via
  `Thread::spawn` with a Ruby-flavored surface syntax (`Thread.spawn do ... end`).
- **G3.** Ship a minimal but complete set of sync primitives: `Mutex[T]`,
  `RwLock[T]`, `Atomic*`, `Arc[T]`, `Condvar`, `Barrier`, `Once`.
- **G4.** Ship `Channel[T]` (MPSC) as the primary message-passing primitive.
- **G5.** Borrow-check thread-crossing closures: every value captured by a
  `spawn`ed closure must be `Send`; every `&T` captured must have `T: Sync`.
- **G6.** Panic in one thread must not corrupt memory; it may be propagated
  through `JoinHandle::join`.
- **G7.** Linux/pthreads is tier 1. macOS is tier 1 (same pthread API). Windows
  is tier 2 (separate `runtime_win.c` later).

### 3.2 Non-Goals (this phase)

- **NG1.** Green threads / M:N scheduling — deferred to a Phase 3 "async" doc.
- **NG2.** `async`/`await` — the keywords stay reserved but unimplemented.
- **NG3.** Actors — the `actor`/`send`/`receive` keywords stay reserved.
- **NG4.** Work-stealing runtime (Tokio/Rayon-style).
- **NG5.** Formal memory model (we inherit the C11/pthreads happens-before
  model). A rigorous spec is future work.
- **NG6.** `no_std`-style stripped-down threading. Every Riven binary links
  libc and libpthread.
- **NG7.** Thread-safe GC — Riven has no GC.

---

## 4. Send / Sync Semantics

### 4.1 Definitions

- **`Send`**: A type `T` is `Send` iff a value of type `T` can safely be
  *transferred* across thread boundaries. The ownership-transfer is the
  operation that changes threads, not sharing.
- **`Sync`**: A type `T` is `Sync` iff `&T` is `Send`, i.e. it is safe for
  multiple threads to hold immutable references to the same `T` simultaneously.

These are **auto-traits**: the compiler infers `Send`/`Sync` structurally from
the type's fields unless the author explicitly opts out.

### 4.2 Inference rules

Given a type `T`, the compiler computes `is_send(T)` and `is_sync(T)` as the
greatest fixed-point under these rules (exactly mirroring Rust):

```
is_send(T) is true iff:
  T is a primitive scalar (Int*, UInt*, Float*, Bool, Char, Unit, Never) → yes
  T is String                                                             → yes
  T is &'a U                                                              → is_sync(U)
  T is &'a mut U                                                          → is_send(U)
  T is *T, *mut T, *Void, *mut Void                                       → no (raw pointers are !Send)
  T is [U; N]                                                             → is_send(U)
  T is (U1, ..., Un)                                                      → all is_send(Ui)
  T is Vec[U] / Hash[K,V] / Set[U] / Option[U] / Result[U,E]              → elementwise is_send
  T is a user struct/class with fields F1..Fn                             → all is_send(Fi) AND not opted-out
  T is an enum with variants V1..Vn whose payloads are U1..Un             → all is_send(Ui) AND not opted-out
  T is a closure capturing C1..Cn                                         → all is_send(Ci)
  T is Mutex[U], RwLock[U]                                                → is_send(U)  (wrapping makes !Sync Sync-safe)
  T is Arc[U]                                                             → is_send(U) AND is_sync(U)
  T is Atomic*                                                            → yes
  T is dyn Trait (without explicit + Send)                                → no
  T is impl Trait (without + Send bound)                                  → depends on concrete type
```

`is_sync(T)` follows the same shape with rules:

```
  is_sync(T) iff is_send(&'_ T)

  Concretely:
  primitives                → yes
  String                    → yes (it's immutable from outside once shared &)
  &U, &mut U                → is_sync(U)
  raw pointers              → no
  arrays/tuples/Vec/...     → elementwise is_sync
  user struct/class         → all fields is_sync AND not opted-out
  Mutex[U]                  → yes (Mutex provides internal synchronization)
  RwLock[U]                 → yes iff is_sync(U) AND is_send(U)
  Arc[U]                    → is_sync(U) AND is_send(U)
  Atomic*                   → yes
  Cell[U] / RefCell[U]      → no (if we add them — interior mutability without sync)
```

### 4.3 HIR representation

Add to `crates/riven-core/src/hir/types.rs`, alongside `is_copy()`:

```rust
impl Ty {
    /// Returns true if this type is Send (safe to transfer across threads).
    pub fn is_send(&self, ctx: &TraitContext) -> bool { ... }

    /// Returns true if this type is Sync (safe to share across threads).
    pub fn is_sync(&self, ctx: &TraitContext) -> bool { ... }
}
```

These take a `&TraitContext` because user-defined structs require looking up
field types and opt-out markers in the symbol table. For the well-known types
(primitives, `Ref`, `Vec`, `Tuple`, etc.) the answer is structural and does not
touch the context.

A new enum tracks why a type is *not* `Send`/`Sync`, for diagnostics:

```rust
#[derive(Debug, Clone)]
pub enum SendSyncViolation {
    RawPointer(Span),
    FieldNotSend { field_name: String, field_ty: Ty, field_span: Span },
    OptedOut { type_name: String, reason: String, span: Span },
    ClosureCaptureNotSend { capture_name: String, capture_ty: Ty, span: Span },
    DynTraitMissingBound { trait_name: String, span: Span },
}
```

### 4.4 Symbol-table additions

Add to `DefKind::Struct` (and `Class`, `Enum`) in
`crates/riven-core/src/resolve/symbols.rs`:

```rust
pub struct StructInfo {
    pub generic_params: Vec<GenericParamInfo>,
    pub fields: Vec<DefId>,
    pub derive_traits: Vec<String>,
    // NEW:
    pub opt_out_send: bool,   // set if `impl !Send for ThisType` appears
    pub opt_out_sync: bool,
    pub manual_send_span: Option<Span>, // `unsafe impl Send for Self`
    pub manual_sync_span: Option<Span>,
}
```

### 4.5 Surface syntax

**Inference is the default.** Users rarely write anything:

```riven
struct Point
  x: Float
  y: Float
end
# Point is Send (both fields are Send) and Sync (both fields are Sync) — inferred.
```

**Opt-out (negative impl)** — a new syntactic form, parsed in the top-level item
parser; the parser currently accepts `impl Trait for Type`, we extend to accept
`impl !Trait for Type`. Only valid for `Send` and `Sync`; rejected for other
traits with a diagnostic.

```riven
class RawHandle
  fd: *mut Void

  # explicit opt-out for clarity, though inference would reach !Send anyway
  impl !Send for RawHandle
  impl !Sync for RawHandle
end
```

**Manual positive impl** (unsafe — the escape hatch):

```riven
# A hand-rolled lock-free queue that the author has verified is thread-safe.
struct LockFreeQueue[T]
  head: *mut Node[T]
  tail: *mut Node[T]
end

unsafe impl[T: Send] Send for LockFreeQueue[T]
unsafe impl[T: Send] Sync for LockFreeQueue[T]
```

`unsafe impl Send` / `unsafe impl Sync` **must** be inside `unsafe` — parsing
an `impl Send` without `unsafe` for an auto-trait is an error (aligns with P1:
crossing an auto-trait boundary manually is explicit danger).

**Trait bound usage**:

```riven
pub def spawn_task[F: FnOnce() -> () + Send + 'static](f: F) -> JoinHandle[()]
  Thread.spawn(f)
end
```

`'static` is an additional lifetime bound — because Riven already has explicit
lifetime parameters (see `Ty::RefLifetime` in `hir/types.rs:93`), this slots in
without new syntax beyond permitting lifetime names in trait-bound lists.

### 4.6 Parser work

Add to `crates/riven-core/src/parser/items.rs` (the file that parses top-level
impl blocks):

1. Accept `!` after `impl` only for trait refs naming `Send` or `Sync`
   (hand-rolled lookup; fully generic negative impls are out of scope).
2. Accept `unsafe impl Trait for Type` as a new production; the `unsafe`
   prefix is only meaningful for auto-traits. Non-auto traits get a diagnostic
   "`unsafe impl` is not meaningful here".
3. Accept trait bounds with `+ Send`, `+ Sync`, `+ 'static` in generic
   parameter lists.

### 4.7 Type-checker integration

Extend `crates/riven-core/src/typeck/traits.rs`:

- `TraitResolver` gains an `AutoTraitResolver` helper. It maintains two maps:
  `send_impls: HashMap<String, Impl>` and `sync_impls: HashMap<String, Impl>`
  where `Impl = Auto | ManualPositive(Span) | NegativeOptOut(Span) |
  GenericallyConditioned(Vec<TraitRef>)`.
- When `TraitResolver::check_satisfaction` is called for `Send` or `Sync`, it
  delegates to `is_send`/`is_sync` rather than scanning method signatures
  (auto-traits have no methods — they're markers).
- Trait-bound checking on generic-function call sites verifies that the
  argument's type satisfies `Send`/`Sync` bounds declared on the parameter.

### 4.8 Borrow-checker integration (the critical part)

In `crates/riven-core/src/borrow_check/mod.rs`, extend
`BorrowChecker::check_closure` (lines 949-998) with a new flag
`requires_send: bool` passed through from the call site:

- When a closure is the argument to a function/method whose parameter bound
  includes `+ Send` (or is the receiver of `Thread::spawn`), the checker marks
  the closure "spawning".
- For a spawning closure, iterate over `captures` (line 958) and check:
  - If `cap.by_move` or `is_move`: require `is_send(cap.ty)`.
  - If borrow capture (`cap.by_move == false`): require `is_sync(deref(cap.ty))`
    AND the borrow must outlive `'static`, i.e. the borrowed value is a
    `'static` reference or the closure is `move` + captures `Arc`.
- On violation, emit one of the new error codes (§8.3).

Detection of "this callee requires `Send`" is done by consulting the callee's
declared trait bounds. `Thread::spawn`'s signature bakes in
`F: FnOnce() -> T + Send + 'static`, so any call to it triggers the rule.

---

## 5. Thread API

### 5.1 Surface syntax

```riven
use std::thread::Thread

# Ruby-ish block form — the primary idiom.
let handle = Thread.spawn do
  puts "hello from thread"
  compute_42()
end

let result = handle.join!   # result is Int (the return type of the block)
```

Equivalent closure form (for point-free style):

```riven
let handle = Thread.spawn({ || compute_42() })
```

Both lower to the same HIR: `Thread::spawn` is a class method with signature

```
def self.spawn[F: FnOnce() -> T + Send + 'static, T: Send + 'static](f: F) -> JoinHandle[T]
```

The Ruby `do ... end` block is desugared by the existing closure/block machinery
to a `move` closure (because `Thread::spawn`'s parameter bound is
`F: FnOnce + Send + 'static`, the typechecker infers `move` as needed).

### 5.2 `JoinHandle[T]`

```
class JoinHandle[T: Send]
  # Opaque — holds a pthread_t under the hood.

  pub def join -> Result[T, ThreadPanic]
  pub def join! -> T                         # panics if joinee panicked
  pub def thread_id -> ThreadId
end
```

- `join` **consumes** `self` (`consume self` in Riven terms — we already have
  `HirSelfMode::Consuming`). You can only join once.
- `join!` is the familiar `!`-panics convention (see
  `docs/tutorial/15-unsafe.md` §3 — `!` suffix = "safe but can panic").
- Dropping an un-joined `JoinHandle` **detaches** the thread (Rust semantics).

### 5.3 Panic propagation

- A panic in a spawned thread is caught by the thread's trampoline
  (`riven_thread_entry`) and stored in the `JoinHandle`'s result slot.
- On `join()`, the caller receives `Err(ThreadPanic { message: String })`.
- `ThreadPanic` implements `Error`.
- Panics do not propagate to the parent thread implicitly — the parent must
  `join` to observe them. This matches Rust.
- The runtime installs a `pthread_cleanup_push` handler so that if a thread is
  cancelled (we do not expose cancellation, but the handler exists for safety),
  owned values in its stack are not leaked — drop glue runs as normal.

### 5.4 Thread-local storage

```riven
thread_local! REQUEST_ID: UInt64 = 0

def current_request -> UInt64
  REQUEST_ID.get
end

def set_request(id: UInt64)
  REQUEST_ID.set(id)
end
```

The `thread_local!` macro (new in the macro set, alongside `vec!` and `hash!`)
expands to a `ThreadLocal[T]` static whose accessors wrap `pthread_key_create`
/ `pthread_getspecific` / `pthread_setspecific`. Phase 2b may defer this to 2d
if schedule pressure requires.

### 5.5 Miscellaneous thread API

- `Thread.current` — returns a `Thread` handle for the current thread (id + name).
- `Thread.sleep(duration: Duration)` — calls `nanosleep`.
- `Thread.yield_now` — calls `sched_yield`.
- `Thread.id` — returns `ThreadId` (newtype on `UInt64`).
- `Thread.name` — returns `Option[String]`.
- Builder pattern via `ThreadBuilder` (set name, stack size):

```riven
let handle = ThreadBuilder.new
  .name("worker")
  .stack_size(2 * 1024 * 1024)
  .spawn do
    work()
  end
```

### 5.6 Runtime ABI (C side)

```c
/* Added to runtime.c — or a new runtime_thread.c compiled alongside. */

typedef struct {
    pthread_t handle;
    int joined;             /* 0 = live, 1 = joined, 2 = detached */
    int panicked;
    char *panic_message;    /* owned — freed on join */
    int64_t return_value;   /* scalar return — aggregates handled via heap */
} RivenJoinHandle;

RivenJoinHandle *riven_thread_spawn(
    int64_t (*entry)(void *env),
    void *env,                  /* closure environment (heap-allocated) */
    void (*env_dropper)(void *) /* drops env on thread exit */
);

int64_t riven_thread_join(RivenJoinHandle *h);        /* returns value or traps on panic */
void    riven_thread_detach(RivenJoinHandle *h);      /* invoked by Drop on JoinHandle */
int64_t riven_thread_current_id(void);
void    riven_thread_sleep_ns(int64_t ns);
void    riven_thread_yield(void);
```

### 5.7 Linker additions

`crates/riven-core/src/codegen/object.rs:64` must append `-lpthread` to the
linker invocation unconditionally (modern glibc lets it be a no-op, but BSD
and musl need it explicit).

---

## 6. Synchronization Primitives

Each primitive is a built-in generic type known to the compiler (like
`Vec[T]`) — it is not expressed in user-level Riven because of the absence of
unsafe internals. Users see a clean surface API; the implementation lives in
`runtime.c` with thin HIR-to-runtime glue.

### 6.1 `Mutex[T]`

```riven
class Mutex[T]
  pub def self.new(value: T) -> Mutex[T]
  pub def lock -> Result[MutexGuard[T], PoisonError]
  pub def lock! -> MutexGuard[T]
  pub def try_lock -> Option[MutexGuard[T]]
  pub def into_inner -> Result[T, PoisonError]  # consume self
end

class MutexGuard[T]
  # Drop implementation unlocks the mutex.
  pub def deref -> &T
  pub def deref_mut -> &mut T
end
```

- **Send/Sync:** `Mutex[T]: Send if T: Send`; `Mutex[T]: Sync if T: Send`
  (note: only `T: Send`, not `T: Sync` — the mutex *provides*
  synchronization).
- **Borrow-check interaction:** `MutexGuard` is a lifetime-bound wrapper. Its
  `deref`/`deref_mut` produce borrows whose region is bounded by the guard.
  The existing `LifetimeChecker` (`borrow_check/lifetimes.rs`) handles this
  the same way it handles any other method returning `&Self::Inner`.
- **Poisoning:** If a thread panics while holding the lock, the mutex is
  marked poisoned. `lock()` returns `Err(PoisonError)` thereafter.
  `lock!` turns that into a panic.
- **Runtime:** wraps `pthread_mutex_t`. `riven_mutex_new` (allocates + inits),
  `riven_mutex_lock`, `riven_mutex_trylock`, `riven_mutex_unlock`, and
  `riven_mutex_free` (called from `Drop`).

### 6.2 `RwLock[T]`

```riven
class RwLock[T]
  pub def self.new(value: T) -> RwLock[T]
  pub def read -> Result[RwLockReadGuard[T], PoisonError]
  pub def read! -> RwLockReadGuard[T]
  pub def write -> Result[RwLockWriteGuard[T], PoisonError]
  pub def write! -> RwLockWriteGuard[T]
  pub def try_read, try_write
end
```

- **Send/Sync:** `RwLock[T]: Send if T: Send`; `RwLock[T]: Sync if T: Send + Sync`.
  Unlike `Mutex`, the read guard hands out `&T` that can genuinely be shared
  concurrently — so `T: Sync` is required.
- **Runtime:** `pthread_rwlock_t`.

### 6.3 `Condvar`

```riven
class Condvar
  pub def self.new -> Condvar
  pub def wait[T](guard: MutexGuard[T]) -> Result[MutexGuard[T], PoisonError]
  pub def wait_timeout[T](guard: MutexGuard[T], dur: Duration) -> Result[(MutexGuard[T], WaitTimeoutResult), PoisonError]
  pub def notify_one
  pub def notify_all
end
```

Follows Rust almost exactly. Runtime: `pthread_cond_*`.

### 6.4 Atomics

Primitive types only — one atomic per scalar width.

```riven
AtomicBool
AtomicI32  AtomicI64  AtomicIsize
AtomicU32  AtomicU64  AtomicUsize
```

Each has `new(initial)`, `load(order)`, `store(v, order)`,
`compare_exchange(expected, new, success_order, failure_order)`,
`fetch_add`, `fetch_sub`, `fetch_and`, `fetch_or`, `fetch_xor`, `swap`.
`Ordering` is an enum matching C11 memory orderings: `Relaxed`, `Acquire`,
`Release`, `AcqRel`, `SeqCst`.

```riven
use std::sync::atomic::{AtomicI64, Ordering}

let counter = AtomicI64.new(0)
counter.fetch_add(1, Ordering.Relaxed)
```

- **Send/Sync:** all atomics are `Send + Sync`. Hard-coded special case in
  `is_send`/`is_sync`.
- **Codegen:** Cranelift has `ins().atomic_*` builtins — we lower atomic ops
  directly without a runtime C hop, for both backends. This is the one place
  we do not dispatch through `runtime.c`.

### 6.5 `Arc[T]`

Atomic reference counting — the primary way to share ownership across threads.

```riven
class Arc[T]
  pub def self.new(value: T) -> Arc[T]
  pub def clone -> Arc[T]                 # increments refcount
  pub def strong_count -> USize
  pub def weak_count -> USize
  pub def downgrade -> Weak[T]
  pub def deref -> &T                      # Arc[T] auto-derefs to &T in method calls
end
```

- **Send/Sync:** `Arc[T]: Send + Sync if T: Send + Sync`.
  Crucially: `Arc[T]` is `Send` only when `T` itself is `Send`, to prevent
  `Arc[RefCell[U]]` escape hatches.
- **P4 (Own What You Use):** `Arc` is user-visible and opt-in. Plain data
  does not silently become refcounted. The existence of a `.clone()` that
  bumps the refcount is the loud signal.
- **Runtime:** single 16-byte header `{ strong: AtomicUsize; weak: AtomicUsize; value: T }`;
  `riven_arc_new`, `riven_arc_clone`, `riven_arc_drop` (decrement + free at 0).
- **Weak[T]** is optional for v1 — list as stretch goal. It avoids cycles but
  adds complexity. If deferred, document: "cycles with `Arc` leak; for now,
  avoid them."

### 6.6 `Barrier`

```riven
class Barrier
  pub def self.new(n: USize) -> Barrier
  pub def wait -> BarrierWaitResult  # .is_leader returns true for one thread
end
```

Runtime: `pthread_barrier_t` on Linux; on macOS (no pthread_barrier), emulate
with `Mutex` + `Condvar` + counter.

### 6.7 `Once`

```riven
class Once
  pub def self.new -> Once
  pub def call_once(f: FnOnce() -> ())
end
```

Runtime: `pthread_once_t` + `pthread_once`.

### 6.8 `LazyStatic`

A `lazy_static!` macro — optional in Phase 2d. Expands to a `Once`-gated
`unsafe` mutable static holding a boxed `T`. Needed for non-trivial globals
(e.g. `LOGGER`).

---

## 7. Channels

Primary message-passing primitive. Start with MPSC unbounded + MPSC bounded;
MPMC deferred.

### 7.1 Surface API

```riven
use std::sync::channel

let (sender, receiver) = channel.unbounded[Int]

# Spawn 4 producers.
for i in 0..4
  let s = sender.clone
  Thread.spawn do
    for j in 0..10
      s.send(i * 100 + j)!
    end
  end
end
drop(sender)   # close the sending side from main

while let Ok(v) = receiver.recv
  puts v
end
```

### 7.2 Types

```riven
class Sender[T]
  pub def send(v: T) -> Result[(), SendError[T]]
  pub def clone -> Sender[T]                 # cloning extends producer count
end

class Receiver[T]
  pub def recv -> Result[T, RecvError]         # blocks until msg or disconnected
  pub def try_recv -> Result[T, TryRecvError]  # non-blocking
  pub def recv_timeout(d: Duration) -> Result[T, RecvTimeoutError]
  pub def iter -> impl Iterator[Item = T]
end

pub def channel.unbounded[T: Send] -> (Sender[T], Receiver[T])
pub def channel.bounded[T: Send](cap: USize) -> (Sender[T], Receiver[T])
```

### 7.3 Semantics

- **MPSC:** many `Sender`, one `Receiver`. `Sender: Clone`, `Receiver: !Clone`.
  Enforced by not exposing `Receiver.clone` and — redundantly — the type's
  `impl !Clone for Receiver`.
- **Closing:** when the *last* `Sender` is dropped, `recv` returns
  `Err(RecvError::Disconnected)`. When the `Receiver` is dropped, further
  `send` returns `Err(SendError(payload))`.
- **Bounded:** `send` blocks when full; `try_send` returns immediately.
- **Ordering:** messages delivered in send-order per sender; FIFO across
  multi-producer merge is not guaranteed (match Rust's `mpsc`).
- **Send/Sync:**
  - `Sender[T]: Send if T: Send`; `Sender[T]: !Sync` (cloning is the
    multi-producer story — no sharing one `Sender` by reference).
  - `Receiver[T]: Send if T: Send`; `Receiver[T]: !Sync`.

### 7.4 Implementation sketch

Internally a channel is:

```
struct Channel[T] {
  mutex:    pthread_mutex_t
  not_full: pthread_cond_t     // bounded only
  not_empty: pthread_cond_t
  queue:    VecDeque[T]
  cap:      Option[USize]
  n_senders: AtomicUsize
  receiver_alive: AtomicBool
}
```

`Sender` and `Receiver` are fat pointers `(Arc[Channel[T]], ())`.
This is O(1) per op, trades raw throughput for simplicity. A lock-free
implementation (Rust's `crossbeam`) can replace it later with no API break.

### 7.5 Why MPSC first

`mpsc::channel` is what ~90% of real Rust thread code uses. MPMC
(`crossbeam::channel`) is a performance win but a superset API — we can add it
later as `channel.mpmc_unbounded`/`mpmc_bounded` without breaking anything.

---

## 8. Borrow-Checker Integration — Specifics

### 8.1 Where the checks go

A new file: `crates/riven-core/src/borrow_check/thread_safety.rs`. It exposes:

```rust
pub struct ThreadSafetyChecker<'a> {
    symbols: &'a SymbolTable,
    traits: &'a TraitResolver,
}

impl<'a> ThreadSafetyChecker<'a> {
    pub fn is_send(&self, ty: &Ty) -> Result<(), SendSyncViolation>;
    pub fn is_sync(&self, ty: &Ty) -> Result<(), SendSyncViolation>;

    /// Called from BorrowChecker::check_closure when closing over values
    /// for a Send-bounded consumer (e.g. Thread::spawn).
    pub fn check_closure_captures_are_send(
        &self,
        captures: &[Capture],
        is_move: bool,
    ) -> Vec<SendSyncViolation>;
}
```

`BorrowChecker::check_closure` (in `borrow_check/mod.rs`) grows a new
parameter `send_required: bool`. It is threaded through from
`check_fn_call` / `check_method_call`, which consult the callee's signature
via `SymbolTable::get(def_id) -> FnSignature -> generic_params` and look for
`Send` in any generic bound on a parameter of the same position as the closure
argument. When `send_required`, `check_closure` invokes
`ThreadSafetyChecker::check_closure_captures_are_send` and pushes any
violations to `self.errors`.

### 8.2 Trait-bound enforcement

On a generic function call `fn foo[T: Send](x: T)`, typeck already records
the bound. A new pass in `typeck/infer.rs` after substitution checks that the
concrete `T` satisfies `Send`/`Sync`. Since these are auto-traits,
satisfaction is decided by `ThreadSafetyChecker::is_send`, not by method-based
`TraitResolver::check_satisfaction`.

### 8.3 New error codes

Extend `crates/riven-core/src/borrow_check/errors.rs`:

```rust
pub enum ErrorCode {
    /* existing ... */
    E1011, // value of type `T` is not `Send`, cannot cross thread boundary
    E1012, // value of type `T` is not `Sync`, cannot be shared across threads
    E1013, // non-'static reference captured by spawned closure
    E1014, // `unsafe impl Send`/`impl Sync` outside unsafe context
    E1015, // use of poisoned mutex (lint-level; default deny → error)
    E1016, // MutexGuard outlives Mutex
}
```

### 8.4 Test cases (fixtures to add)

Place in `crates/riven-core/tests/fixtures/concurrency/`:

**Must compile (positive):**

- `spawn_send_primitive.rvn` — `Thread.spawn do let x = 42; puts x end`.
- `spawn_move_string.rvn` — `let s = String.new("x"); Thread.spawn do puts s end`.
- `arc_mutex_counter.rvn` — classic shared counter with `Arc[Mutex[Int]]` over
  N threads, joins, asserts total.
- `channel_ping_pong.rvn` — two threads exchanging `Int` through a bounded
  channel of capacity 1.
- `rwlock_many_readers.rvn` — multiple `read!` guards held simultaneously.
- `atomic_spin_barrier.rvn` — `AtomicBool` spin-wait as rendezvous.

**Must reject (negative):**

- `spawn_capture_rc.rvn` — capturing a hypothetical `Rc` (or any `!Send` type)
  in `Thread.spawn` → expect E1011.
- `spawn_borrow_local.rvn` — `let x = 42; Thread.spawn do puts &x end` without
  `move` and without `'static` → expect E1013.
- `send_receiver_across_clone.rvn` — `let (_, r) = channel.unbounded[Int]; r.clone` →
  expect "method `clone` not found on `Receiver[Int]`".
- `mutex_guard_escape.rvn` — return `MutexGuard` from function that owns the
  `Mutex` → expect E1010 (existing code, exercised for the new guard type).
- `unsafe_impl_without_unsafe.rvn` — `impl Send for Foo` without `unsafe` prefix
  → expect E1014.
- `sync_raw_pointer_struct.rvn` — struct with `*mut Void` field; call
  `Thread.spawn` moving it → expect E1011 with field-level diagnostic
  (`SendSyncViolation::FieldNotSend`).

### 8.5 Interaction with existing borrow checker

- **NLL:** `MutexGuard` drop expiring the borrow works through the existing
  `BorrowSet::expire_before` mechanism at `borrow_check/mod.rs:143`.
- **Move into closure:** already handled at `check_closure` line 958. The new
  check layers on top without restructuring the existing moves-into-closures
  logic.
- **Return-reference-to-local (E1010):** `MutexGuard` returning from a function
  that owns the `Mutex` is caught by the existing E1010 machinery at
  `check_return` (line 911-945), because the guard's lifetime is bounded by
  the `Mutex`.

---

## 9. Runtime Additions

### 9.1 Split `runtime.c` into modules

Current single-file runtime (`crates/riven-core/runtime/runtime.c`) grows
unwieldy. Proposal: add a second file `runtime_thread.c` in the same
directory, compiled and linked alongside. `codegen/object.rs:compile_runtime`
changes to:

```rust
cc -c runtime.c runtime_thread.c -o runtime.o    // -c with multiple inputs isn't legal;
// actually: compile each to a separate .o and link both into the final binary.
```

Or keep a single translation unit via `#include "runtime_thread.c"` at the
bottom of `runtime.c` — simpler for v1, splittable later.

### 9.2 What goes in `runtime_thread.c`

pthread-wrappers only. One function per C primitive:

```c
/* Threads */
RivenJoinHandle *riven_thread_spawn(int64_t (*entry)(void *), void *env,
                                    void (*env_dropper)(void *));
int64_t riven_thread_join(RivenJoinHandle *h);
void    riven_thread_detach(RivenJoinHandle *h);
int64_t riven_thread_current_id(void);
void    riven_thread_sleep_ns(int64_t ns);
void    riven_thread_yield(void);
void    riven_thread_set_name(const char *name);

/* Mutex */
void *  riven_mutex_new(void);
void    riven_mutex_lock(void *m);
int     riven_mutex_trylock(void *m);        /* 0 = ok, 1 = would block */
void    riven_mutex_unlock(void *m);
void    riven_mutex_free(void *m);

/* RwLock */
void *  riven_rwlock_new(void);
void    riven_rwlock_rdlock(void *);
int     riven_rwlock_tryrdlock(void *);
void    riven_rwlock_wrlock(void *);
int     riven_rwlock_trywrlock(void *);
void    riven_rwlock_unlock(void *);
void    riven_rwlock_free(void *);

/* Condvar */
void *  riven_cond_new(void);
void    riven_cond_wait(void *c, void *m);
int     riven_cond_wait_timeout(void *c, void *m, int64_t ns);
void    riven_cond_notify_one(void *c);
void    riven_cond_notify_all(void *c);
void    riven_cond_free(void *c);

/* Once */
void    riven_once_call(void *once_state, void (*f)(void *), void *env);

/* TLS */
int64_t riven_tls_key_create(void (*dtor)(void *));
void *  riven_tls_get(int64_t key);
void    riven_tls_set(int64_t key, void *value);
```

### 9.3 Atomics do not go through C

They lower directly via Cranelift/LLVM atomic instructions. Rationale: a C
round-trip would defeat the purpose (cost + missed optimizations) and the
memory order is easier to express at the IR level.

### 9.4 Runtime support table (`crates/riven-core/src/codegen/runtime.rs`)

Add entries to the mangled-name table mapping Riven method names to C
runtime symbols:

| Riven call                | C symbol                  |
|---------------------------|---------------------------|
| `Thread_spawn`            | `riven_thread_spawn`      |
| `JoinHandle_join`         | `riven_thread_join`       |
| `JoinHandle_detach`       | `riven_thread_detach`     |
| `Thread_sleep`            | `riven_thread_sleep_ns`   |
| `Mutex_new`               | `riven_mutex_new`         |
| `Mutex_lock`              | `riven_mutex_lock`        |
| `Mutex_unlock`            | `riven_mutex_unlock`      |
| `RwLock_*`, `Condvar_*`, `Once_*` | analogous         |

Channel, Arc, and atomic ops are synthesized by the compiler, not dispatched
through this table.

### 9.5 Platform matrix

| Platform | pthread | pthread_barrier | atomic intrinsics | Status |
|----------|---------|-----------------|-------------------|--------|
| Linux glibc | Yes | Yes | Yes | **Tier 1** |
| Linux musl  | Yes | Yes | Yes | Tier 1 |
| macOS       | Yes | **No** (emulate) | Yes | Tier 1 |
| Windows     | No (Win32 API) | — | Yes | Tier 2 (post-v0.2) |
| FreeBSD     | Yes | Yes | Yes | Best-effort |

### 9.6 Linker invocation

Add to `codegen/object.rs` in `emit_executable`:

```rust
cmd.arg("-lpthread");          // unconditional after Phase 2b lands
```

macOS is a no-op link (`libpthread` is part of `libc`), but passing `-lpthread`
is harmless there. Windows build path will diverge when Tier 2 arrives.

---

## 10. Surface Syntax Examples

### 10.1 Arc + Mutex counter

```riven
use std::thread::Thread
use std::sync::{Arc, Mutex}

def main
  let counter = Arc.new(Mutex.new(0))
  let mut handles = vec![]

  for _ in 0..8
    let c = counter.clone
    let h = Thread.spawn do
      for _ in 0..1000
        let mut guard = c.lock!
        *guard += 1          # deref_mut
      end
    end
    handles.push(h)
  end

  for h in handles
    h.join!
  end

  puts "final = #{counter.lock!}"   # 8000
end
```

Borrow-check expectations:

- `counter.clone` produces a new `Arc[Mutex[Int]]`. `Arc[T]: Send + Sync if T: Send + Sync`.
  `Mutex[Int]: Send + Sync` because `Int: Send`. ✓
- Closure captures `c` by move (inferred because `Thread.spawn`'s bound is
  `F: FnOnce + Send + 'static`). Captured `c: Arc<Mutex<Int>>` is `Send`. ✓
- Inside closure, `c.lock!` returns `MutexGuard<Int>`. The guard owns a live
  borrow of the mutex; NLL expires it at end of loop iteration. ✓

### 10.2 Channel pipeline

```riven
use std::sync::channel
use std::thread::Thread

def main
  let (tx, rx) = channel.bounded[String](16)

  Thread.spawn do
    for i in 0..100
      tx.send("msg-#{i}")!
    end
    # tx dropped → receiver sees Disconnected after draining
  end

  while let Ok(msg) = rx.recv
    puts msg
  end
end
```

### 10.3 Producer group via `Arc`

```riven
let (tx, rx) = channel.unbounded[Int]

for i in 0..4
  let t = tx.clone             # each producer owns a Sender
  Thread.spawn do
    for j in 0..25
      t.send(i * 25 + j)!
    end
  end
end
drop(tx)                       # drop main's handle so rx disconnects when all producers finish

let mut total = 0
while let Ok(v) = rx.recv
  total += v
end
puts total                     # 0+1+...+99 = 4950
```

### 10.4 Borrow-check rejection: capturing non-Send

```riven
class NonSendThing
  ptr: *mut Void

  impl !Send for NonSendThing      # explicit — inferred anyway
end

let x = NonSendThing { ptr: null }
Thread.spawn do
  use_it(&x)           # ERROR E1011: value of type `NonSendThing` is not `Send`
end
```

Diagnostic (rendered via existing `BorrowError::fmt`):

```
error[E1011]: value of type `NonSendThing` is not `Send`
  --> src/main.rvn:9:1
   | 9:3 — `NonSendThing` is not Send because it contains a raw pointer
   | 3:3 — field `ptr: *mut Void` is not Send
   | 9:1 — captured here by spawned closure
   = help: wrap the field in `Arc<Mutex<...>>`, or `unsafe impl Send` if
           you've manually verified thread safety
```

### 10.5 Atomic counter

```riven
use std::sync::atomic::{AtomicI64, Ordering}
use std::sync::Arc
use std::thread::Thread

def main
  let counter = Arc.new(AtomicI64.new(0))
  let mut handles = vec![]

  for _ in 0..4
    let c = counter.clone
    handles.push(Thread.spawn do
      for _ in 0..250_000
        c.fetch_add(1, Ordering.Relaxed)
      end
    end)
  end

  for h in handles
    h.join!
  end

  puts counter.load(Ordering.SeqCst)       # 1_000_000
end
```

### 10.6 `Once` for lazy init

```riven
use std::sync::Once

static INIT: Once = Once.new
static mut LOG_FD: Int = -1

def init_logger
  INIT.call_once do
    unsafe
      LOG_FD = open("app.log")
    end
  end
end
```

(`static mut` is a separate Tier 1 requirement — listed as a dependency below.)

---

## 11. Phasing

Recommend shipping in four sub-phases. Each is independently testable and
produces a runnable subset.

### Phase 2a — Send/Sync as auto-traits (compile-time only)
Adds the marker traits, inference, opt-out, manual `unsafe impl`,
diagnostics. No runtime changes. Validation: the `spawn_capture_*` fixtures
all compile or reject as expected *even though `Thread::spawn` doesn't exist
yet* — we use a sentinel `std::mem::require_send[T: Send](x: T)` helper and
assertions on its use site.
**Deliverable:** `crates/riven-core/src/borrow_check/thread_safety.rs`,
`typeck/traits.rs` extended for auto-traits, E1011/E1012/E1014 errors,
6+ fixture tests.

### Phase 2b — Threads + `Arc` + `Mutex`
`Thread::spawn`/`JoinHandle`, `Mutex[T]`, `Arc[T]`, runtime.c pthread layer,
`-lpthread` linker flag. Programs can now spawn, share a counter, join.
Panic propagation. Includes `Drop` wiring for `MutexGuard` (lock release).
**Deliverable:** `runtime_thread.c`, `Arc`/`Mutex`/`Thread` in stdlib,
10+ fixture tests, panic tests.

### Phase 2c — Channels
`channel.unbounded` + `channel.bounded` (MPSC). `Sender`/`Receiver`/iter.
Pure Riven + `Arc` + `Mutex` + `Condvar` — minimal new C code (Condvar is
the only new primitive).
**Deliverable:** `std::sync::channel` module, disconnect semantics,
bounded back-pressure tests.

### Phase 2d — Atomics + `RwLock` + misc
Full atomic set, `Ordering` enum, Cranelift/LLVM codegen for atomic ops,
`RwLock`, `Barrier`, `Once`, `thread_local!`, `ThreadBuilder`.
**Deliverable:** `std::sync::atomic` module, 2+ fixtures per primitive.

### Post-2 (deferred)
MPMC channels, `Weak[T]` (Arc backrefs), `LazyStatic`/`lazy_static!` macro,
Windows runtime, async/await (separate Tier 2 doc),
actor syntax (separate Tier 2 doc).

---

## 12. Open Questions & Risks

1. **Deref ergonomics on `MutexGuard` and `Arc`.** Riven has no `Deref` trait
   yet. Three options:
   - (a) Add an `AutoDeref` built-in trait that the method resolver consults.
   - (b) Make these types "magic" in the method resolver: hard-code
     "when calling a method on `MutexGuard[T]` or `Arc[T]`, look up methods
     on `T`".
   - (c) Require explicit `.get` / `.get_mut` calls (uglier, but simplest).

   Recommend (a) as a side quest — it benefits `String`→`&str` and
   `Vec[T]`→`&[T]` too. If it's too much scope, (b) unblocks Phase 2b and we
   revisit (a) later.

2. **Placement of `Arc` and channel types.** Are they `std::sync::Arc` /
   `std::sync::channel` or compiler built-ins like `Vec`? A strong argument for
   compiler-intrinsic status: `Arc` needs to emit atomic refcount ops that
   live outside user-writable surface syntax. Once we expose atomics in
   userland (Phase 2d), `Arc` *could* be a regular library type that uses
   `AtomicUsize` — but then we'd need an `unsafe` block inside stdlib. This
   seems fine: stdlib can be "privileged unsafe".

3. **Scoped threads.** Rust 1.63+ `std::thread::scope` allows borrowing
   non-`'static` data in spawned threads with compile-time lifetime checking.
   This requires more elaborate borrow-check machinery (a "scoped closure"
   whose borrow region ends at `scope` exit). Deferred — leave a stub
   `Thread::scope` for Phase 3.

4. **Drop glue in spawned thread environments.** The closure's captured
   environment (the heap-allocated `env` arg passed to
   `riven_thread_spawn`) must be dropped *by the child thread* when its
   `entry` function returns. MIR already emits drop calls at end-of-scope;
   we need to confirm that the synthesized wrapper around a spawned closure
   emits drops for all captures. Test with a `Drop`-implementing type
   captured by a spawned closure.

5. **Panic-in-destructor double faults.** If a thread panics inside a
   `Drop`, C++-style double-panic is possible. Match Rust: second panic aborts
   the process. Implement via a thread-local "panic count" incremented on
   entry to panic handler.

6. **Signature synthesis for built-in generic methods.** `Mutex::lock`
   returns a `MutexGuard` whose lifetime is tied to `&self`. Riven's
   inference engine (`typeck/infer.rs`) needs to understand self-borrowing
   return types. Check whether `Vec::iter` (which has the same shape) is
   already modeled; if so, reuse.

7. **`'static` bound.** Riven has `Ty::RefLifetime(String, ...)`. Need to
   make the name `"static"` reserved and recognized as "outlives all scopes".
   The bound `T: 'static` on a type parameter means "contains no non-static
   references" — same rule as Rust. Implementable as a simple traversal over
   `Ty` checking for any `RefLifetime(name, _)` with `name != "static"`.

8. **Interaction with existing `Copy` inference.** Today `Ty::is_copy()` (at
   `hir/types.rs:189`) is hand-rolled. For a user struct to be `Copy`, it
   must `derive Copy`. `Send`/`Sync` should parallel this: auto-inferred
   structurally, with `derive` not strictly necessary but permitted
   (`derive Send`, `derive Sync` being a no-op — inference already said yes).

9. **Cache compatibility.** The incremental-compilation cache (`rivenc::cache`)
   keys on the hash of compilation inputs. Adding `Send`/`Sync` inference to
   a definition's effective signature means cached entries from pre-2a
   builds will be invalidated. Not a blocker — bump the cache version.

10. **Error-message quality.** E1011 (not `Send`) is notorious for being
    cryptic in Rust. Design the diagnostic to *walk the field path* —
    "`Foo.bar.baz: *mut Void` is not Send", not just "`Foo` is not Send".
    `SendSyncViolation::FieldNotSend` carries enough information to do this;
    make sure the renderer uses it.

11. **Memory-model documentation.** We inherit C11's model transitively via
    pthreads and atomic intrinsics. Decide whether to document a formal
    subset or just say "matches Rust". Recommend the latter for v0.2 with
    a pointer to the Rust Reference §Behavior considered undefined and
    §Memory model sections.

---

## 13. Appendix — Affected Files Checklist

Files to modify (Phase 2a):

- `crates/riven-core/src/hir/types.rs` — add `Ty::is_send`, `Ty::is_sync`,
  `SendSyncViolation` enum.
- `crates/riven-core/src/resolve/mod.rs` — add `Send`, `Sync` to
  `builtin_traits` (lines 139-151).
- `crates/riven-core/src/resolve/symbols.rs` — extend `StructInfo`,
  `ClassInfo`, `EnumInfo` with opt-out fields.
- `crates/riven-core/src/parser/items.rs` — parse `impl !Send`, `unsafe impl Send`.
- `crates/riven-core/src/typeck/traits.rs` — auto-trait resolution path.
- `crates/riven-core/src/borrow_check/thread_safety.rs` — **NEW FILE**.
- `crates/riven-core/src/borrow_check/errors.rs` — add E1011-E1016.
- `crates/riven-core/src/borrow_check/mod.rs` — thread `send_required` through
  `check_closure`, `check_fn_call`, `check_method_call`.
- `crates/riven-core/tests/fixtures/concurrency/*.rvn` — **NEW FIXTURES**.

Files to modify (Phase 2b):

- `crates/riven-core/runtime/runtime_thread.c` — **NEW FILE** (or inlined).
- `crates/riven-core/runtime/runtime.c` — include thread module.
- `crates/riven-core/src/codegen/runtime.rs` — new mangled-name entries.
- `crates/riven-core/src/codegen/object.rs` — add `-lpthread`.
- `crates/riven-core/src/hir/types.rs` — add `Ty::Arc`, `Ty::Mutex` (or
  keep them as `Ty::Class { name: "Arc", ... }` with special-case handling).
- Standard library Riven source for `Thread`, `JoinHandle`, `Mutex`,
  `MutexGuard`, `Arc`, `PoisonError`, `ThreadPanic`.

Files to modify (Phase 2c):

- `crates/riven-core/src/codegen/runtime.rs` — Condvar entries.
- `runtime_thread.c` — `pthread_cond_*` wrappers.
- Stdlib Riven source: `std::sync::channel`, `Sender`, `Receiver`,
  `SendError`, `RecvError`, `TryRecvError`.

Files to modify (Phase 2d):

- `crates/riven-core/src/codegen/cranelift.rs` — lower atomic ops via
  `ins().atomic_rmw`, `atomic_cas`, `atomic_load`, `atomic_store`.
- `crates/riven-core/src/codegen/llvm.rs` — analogous via LLVM atomic
  intrinsics.
- Stdlib Riven source: `std::sync::atomic`, `Ordering`, each `AtomicX`,
  `RwLock`, `Barrier`, `Once`, `ThreadBuilder`.
- `thread_local!` macro handler in the macro dispatcher (wherever `vec!` and
  `hash!` are handled today).

---

## 14. Acceptance Criteria

Phase 2 is considered complete when:

- [ ] All fixtures in `tests/fixtures/concurrency/` behave as documented in §8.4.
- [ ] `arc_mutex_counter.rvn` runs to completion with the correct total
      under TSan (when the sanitize-build path runs with ThreadSanitizer).
- [ ] `cargo test -p riven-core concurrency` passes on Linux and macOS in CI.
- [ ] `docs/tutorial/` gains a chapter `17-concurrency.md` mirroring this doc
      for end users.
- [ ] At least one end-to-end `cargo run -p rivenc -- examples/threads/*.rvn`
      example exists per primitive.
- [ ] The rendered error for E1011 points at the offending field, not just the
      struct.
- [ ] Un-joined `JoinHandle` detaches cleanly (no leak warnings under
      ASan/LSan).
- [ ] Mutex poisoning works: a panicking thread's mutex produces
      `Err(PoisonError)` on subsequent `lock`.
- [ ] Dropping the last `Sender` causes `Receiver.recv` to return
      `Err(Disconnected)`.
