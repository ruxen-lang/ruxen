# Tier 1 Roadmap — Cross-Doc Synthesis

Companion index for the five Tier-1 requirements documents. Read this first.

## The docs

| # | Feature | Doc |
|---|---------|-----|
| 01 | Standard library | [tier1_01_stdlib.md](tier1_01_stdlib.md) |
| 02 | Concurrency (threads, sync, Send/Sync) | [tier1_02_concurrency.md](tier1_02_concurrency.md) |
| 03 | Async / await | [tier1_03_async.md](tier1_03_async.md) |
| 04 | Drop, Copy, Clone | [tier1_04_drop_copy_clone.md](tier1_04_drop_copy_clone.md) |
| 05 | Implicit includes | [tier1_05_implicit_includes.md](tier1_05_implicit_includes.md) |

## Pre-existing bugs surfaced during research (fix first)

The five agents independently surfaced issues in code that already exists. None of the Tier-1 work is safe to build on top of the current state without addressing these.

### B1. `Drop` is a no-op; every program leaks heap memory
- `MirInst::Drop` is emitted by `insert_drops` (`compiler/ruxen_core/src/mir/lower.rs:3346-3407`) but both codegen backends silently discard it (`codegen/cranelift.rs:692-698`, `codegen/llvm/emit.rs:790-792`).
- The whitelist explicitly excludes `String`/`Array`/`Option`/`Result`, so even the existing infrastructure wouldn't free them.
- Tutorials at `docs/tutorial/15-unsafe.md:62-68` describe `include Drop` as if it works.
- **Consequence:** all heap allocations survive until process exit. Blocking for any non-trivial stdlib program.

### B2. `derive` is parsed but never consumed (resolved)

**Resolved:** the `derive` directive was removed from the parser. Structural mixins are implicit includes (explicit `include X` is the loud form; see `docs/specs/mixins/implicit_includes.spec.md`), and `layout` args have their own field. The findings below are kept as history.

- `derive Mixin1, Mixin2` in-body form: parses into `HirStructDef::derive_traits` (`hir/nodes.rs:431`), but no pass in `typeck`, `borrow_check`, `mir`, or `codegen` reads it.
- `Ty::is_copy()` (`hir/types.rs:189-221`) makes a purely structural decision and ignores `derive_traits` — so `derive Copy` on a struct has zero ownership effect.
- `parser/mod.rs:499-503` stuffs `layout c` and the like into the same `derive_traits: Vec<String>` field as a string. Must be untangled.
- Classes and enums have no `derive_traits` field at all.

### B3. `Hash` name collision (flagged by both the stdlib and implicit-includes docs)
- `Hash[K,V]` is registered as a collection type constructor (`resolve/mod.rs:200`).
- A hashing mixin also named `Hash` can't coexist with it.
- Resolution: the collection keeps Ruby's name `Hash[K, V]`; the hashing mixin is `Hashable`. No rename.

### B4. `?T..._method` codegen fallback masks failures
- `codegen/runtime.rs`'s `runtime_name()` has a fallback that maps unresolved generic method calls to `ruxen_noop_passthrough`.
- Some currently-passing tests exercise code that is, in fact, a no-op.
- Fixing this is a prerequisite for real `Array.map`/`.filter`/`.find` implementations — and will likely surface test regressions.

### B5. String-literal ownership model
- String literals flow through `MirInst::StringLiteral` directly into locals typed `Ty::String`.
- Lowering `String::drop` to `free()` would double-free literal pointers.
- Fix: either retype literals as `Ty::Str` or wrap at MIR lowering in an implicit `String::from`. Doc 04 §OQ-3 details this.

### B6. Reserved-but-unused keywords
- `async`, `await`, `spawn`, `actor`, `send`, `receive` are reserved in the lexer (`lexer/token.rs:83-85`, `:127-130`) but the parser never consumes them. No functional bug, but they signal design intent that needs to be either delivered or removed — especially the `yield`/`async` relationship (doc 03 §R1).

## Cross-doc dependency graph

```
          ┌───────────────────────────────────────┐
          │ B1-B5: pre-existing bug fixes         │
          └──────────────────┬────────────────────┘
                             │
                             ▼
          ┌───────────────────────────────────────┐
          │ 04: Drop / Copy / Clone               │
          │   (mixin registration + MIR drop      │
          │    elaboration + real codegen)        │
          └──────────────────┬────────────────────┘
                             │
                 ┌───────────┴───────────┐
                 ▼                       ▼
    ┌────────────────────────┐  ┌─────────────────────┐
    │ 05: Implicit includes  │  │ 01: Stdlib phase 1a │
    │ (Debug/Clone/Copy/…)   │  │ (io/fmt/string/     │
    │                        │  │  collection methods)│
    └──────────────┬─────────┘  └──────────┬──────────┘
                   │                       │
                   └───────────┬───────────┘
                               ▼
                  ┌────────────────────────┐
                  │ 01: Stdlib phase 1b/1c │
                  │ (fs/env/process/time/  │
                  │  path/net/hash)        │
                  └────────────┬───────────┘
                               ▼
                  ┌────────────────────────┐
                  │ 02: Concurrency        │
                  │ (Send/Sync + Thread +  │
                  │  Mutex + SharedSync +  │
                  │  channels)             │
                  └────────────┬───────────┘
                               ▼
                  ┌────────────────────────┐
                  │ 03: Async / await      │
                  │ (single-threaded first)│
                  └────────────────────────┘
```

Key dependencies:

- **Implicit includes ↔ Drop/Copy/Clone are mutually dependent.** An implicit `Copy` is meaningless without `Ty::is_copy_with(&SymbolTable)` consulting the include data; implicit `Clone`/`Debug` is how users avoid hand-writing boilerplate *for stdlib types*. Implement together.
- **Stdlib needs implicit includes.** Without an implicit `Debug` every struct hand-writes a `Display` `include`. Stdlib phase 1a should ship alongside the first implicit-include set (5a: Debug + Clone).
- **Stdlib depends on working Drop.** Heap-backed types (`String`, `Array`, `Hash`, `File`, `TcpStream`) leak without it.
- **Concurrency's Send/Sync auto-mixins parallel Copy's structural-inference model** (doc 02 §4). Doing Copy first makes Send/Sync cheap to add.
- **Async depends on concurrency** for the multi-threaded executor path. Single-threaded `block_on` can ship earlier — doc 03 §8 explicitly sequences this.
- **Runtime int64-slot erasure** (`runtime/runtime.c:220-225`) blocks generic stdlib collections with user-defined keys until monomorphization lands. Doc 01 §Risks R5 restricts v1 `Hash` keys to `{Int, UInt, USize, String}`.

## Recommended implementation order

**Phase 0 — pre-flight (1-2 weeks).** Fix B1-B5. Without these, everything downstream is built on sand.

1. B3: keep the collection as `Hash[K, V]` and name the hashing mixin `Hashable` (no rename).
2. B5: fix string-literal ownership model (retype to `Ty::Str` or wrap).
3. B4: remove the `?T..._method` no-op fallback; accept that some existing tests will fail and fix them.
4. B2: done. `derive` is retired in favour of implicit includes, and `layout` has its own storage.
5. B1: don't fix Drop codegen here — that's phase 1.

**Phase 1 — Drop/Copy/Clone + implicit-include foundations (3-4 weeks).**

1. Drop/Copy/Clone mixin infrastructure (doc 04 phase 4a).
2. Implicit-include infrastructure + implicit `Debug`, `Clone` (doc 05 phase 5a).
3. User-written `include Drop` + `def var drop` (doc 04 phase 4b).
4. MIR drop elaboration with drop flags + real codegen (doc 04 phase 4c). **Closes B1.**
5. Implicit `Copy`, `PartialEq` + `Ty::is_copy_with` (doc 05 phase 5b + doc 04 phase 4c integration).
6. Built-in drops for `String`/`Array`/`Option`/`Result` (doc 04 phase 4d).

**Phase 2 — stdlib phase 1a (2-3 weeks).** Doc 01 phase 1a.
- `io`: stdin/stdout/stderr, `print`/`println`/`eprintln`/`read_line`.
- `fmt`: `Debug`/`Display` mixins, formatter, string interpolation (`"#{x}"`).
- `String` method surface (split/strip/include?/start_with?/…).
- `Array`, `Hash`, `Set`, `Option`, `Result` method surface.
- `Enumerable` over `each` (`map`, `select`, `to_a`, …).
- Remaining implicit includes: `Eq`/`Hashable`/`Default`/`Ord`/`PartialOrd` (doc 05 phase 5c).

**Phase 3 — stdlib phase 1b/1c (3-4 weeks).** Doc 01 phase 1b/1c.
- `fs`: `read_to_string`/`write`/`File`/`exists`.
- `env`: `args`/`var` (needs `main(argc, argv)` shim — doc 01 §Risks).
- `process`: `exit`/`Command`/`spawn`.
- `time`: `Instant`/`Duration`/`SystemTime`.
- `path`: `Path`/`PathBuf`.
- `net`: `TcpStream`/`TcpListener`.
- `hash`: `Hasher` mixin + `DefaultHasher`.

**Phase 4 — concurrency (4-6 weeks).** Doc 02 phases 2a-2d.
- 2a: `Send`/`Sync` auto-mixins + `ThreadSafetyChecker` module (reuses Copy's inference pattern).
- 2b: `Thread.spawn` + `JoinHandle` + `Mutex[T]` + `SharedSync[T]`.
- 2c: MPSC channels.
- 2d: `Atomic*` + `RwLock[T]` + `Condvar` + `Barrier` + `Once`.

**Phase 5 — async (4-6 weeks).** Doc 03 phases 3a-3d.
- 3a: `Future` mixin + manual `Poll` `include`s.
- 3b: `async def` + `.await` syntax + MIR state-machine lowering.
- 3c: single-threaded `block_on` executor + reactor C ABI.
- 3d: async I/O (epoll on Linux first).

## Total estimate

~17-25 weeks of focused work for one engineer, faster if parallelized across the independent axes (implicit includes + Drop can overlap with concurrency infra once Phase 1 finishes). Phase 0 alone is 1-2 weeks and unblocks everything.

## Open decisions for the project lead

These surfaced across multiple docs and need a single ruling before implementation begins.

1. **`Hash` naming.** Decided: the collection keeps Ruby's name `Hash[K, V]`; the hashing mixin is `Hashable`.
2. **Module system syntax.** Doc 01 §6 proposes Ruby-style `.`. Confirm canonical form.
3. **`core` vs `std` split.** Doc 01 §6.3 defers it. Decide now whether to pre-adapt so no-std code can land without a later split.
4. **Async surface syntax.** Doc 03 §5 proposes `.await` postfix with `await expr` prefix allowed. Pick the canonical form.
5. **`Pin` for futures.** Doc 03 §6 recommends skipping `Pin` with compiler-internal `!Move`. Accept the risk or commit to `Pin` now.
6. **Closed-class `actor`/`spawn`/`send`/`receive` keywords.** Doc 02 §R3 and doc 03 §R1: either deliver an actor model or un-reserve them.
7. **Stable ABI for runtime C shim.** Docs 01 and 02 both propose C runtime growth. Agree a calling-convention contract before it sprawls.
