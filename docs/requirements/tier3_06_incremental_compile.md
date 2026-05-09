# Tier 3.06 — Incremental Compilation (Core-Level, Query-Based)

Status: draft
Depends on: none (but see §7 for strong coupling with LSP)
Blocks: high-perf LSP for large files; fast `riven check` iteration

---

## 1. Summary & motivation

Riven has file-level incremental compilation in `rivenc` via
`crates/rivenc/src/cache/`. That layer is excellent at its job: two
`rivenc` invocations on an unchanged project re-link the binary in
<100 ms without recompiling anything. But inside a single compilation
of a single file, every phase runs from scratch: lex → parse →
resolve → typeck → borrow-check → MIR lowering → codegen.

This matters in three scenarios:

1. **LSP latency.** A 1000-line Riven file takes ~80-200 ms to fully
   analyze. Per-keystroke analysis is infeasible at this cost. The
   LSP (doc 01) already debounces to 200 ms and can feel sluggish.
2. **Large projects with many small changes.** The rivenc cache
   invalidates the *whole file* on any edit. If a user changes a
   single function body, every other function in the same file is
   re-typechecked unnecessarily.
3. **`riven check` — a build that runs everything except codegen.**
   Today it's almost as slow as a full build because there is no
   memoization of typeck results.

This doc specifies a query-based (salsa-style) incremental compilation
layer inside `riven-core`, with the following properties:

- Fine-grained: functions and items, not just whole files.
- Memoized on content-addressed fingerprints — the same input produces
  the same output, always.
- Backwards-compatible with the file-level cache in `rivenc`.
- Available from both the `rivenc` CLI and from `riven-ide` (consumed
  by the LSP).

---

## 2. Current state

### 2.1 File-level cache in `rivenc/src/cache/`

The existing layer (Phase 13 per `~/.claude1/.../project_phase13_incremental.md`)
keys a whole-file compile on:

- Content hash of the `.rvn` file.
- Compiler version (`cache::hash::compiler_version()`).
- Build flags (`backend`, `opt-level`, `release`).
- Dependency file signatures (other `.rvn` files the current one
  imports).

On cache hit: re-use the `.o` file and `FileSignature` from disk.
On miss: run lex → parse → typeck → borrow-check → MIR → codegen from
scratch (see `rivenc/src/main.rs::compile_to_object` at `main.rs:503-576`).

Files:

- `cache/mod.rs:1-55` — module overview.
- `cache/driver.rs` — orchestrates the build (`build`, `BuildOptions`,
  `CompileFn`, `SourceFile`).
- `cache/hash.rs` — content hashing.
- `cache/manifest.rs` — on-disk manifest.
- `cache/signature.rs` — public-signature extraction (used for
  transitive invalidation).
- `cache/store.rs` — atomic write and storage location.
- `cache/graph.rs` — dependency graph.

### 2.2 In-process analysis always full-pipeline

Both `rivenc` (`main.rs:503-576`) and `riven-ide` (`analysis.rs:43-88`)
call the full pipeline in sequence:

```rust
lexer.tokenize()  → parser.parse()  → typeck::type_check()
                 → borrow_check::borrow_check()
                 → mir::lower::lower_program()
                 → codegen
```

There is no memoization between phases. Each call to `analyze()` in
the LSP re-lexes, re-parses, re-resolves, re-typechecks. The
`TypeCheckResult` and `HirProgram` are not cached anywhere in-memory
between invocations.

### 2.3 No query infrastructure

Grep for `salsa`, `query`, `memoize` under `crates/riven-core`: zero
matches. No `salsa` dependency. No `DashMap`-based memoizer. No
`OnceCell` / `Lazy` statics keyed by content hash.

### 2.4 Symbol table is stateless

The resolver produces a fresh `SymbolTable` per call
(`resolve/mod.rs:97` `register_builtins` runs inside `Resolver::new`).
There is no persistent/shared state between two resolves. This is
both good (no bugs from state pollution) and limiting (no way to
avoid re-registering builtins).

### 2.5 `TypeContext` is per-compile

`TypeCheckResult.type_context` (`typeck/mod.rs:23-28`) holds unification
state for a single compile. Not reused.

---

## 3. Goals & non-goals

### Goals

1. Function-level re-typecheck: changing one function body re-runs
   typeck only for that function.
2. Item-level re-resolve: adding a new `use` statement only invalidates
   items that reference the newly-imported name.
3. Persistent memoization across LSP requests, keyed by content hash.
4. Typecheck result for an unchanged function is an O(1) hash lookup.
5. Backward compatibility: the existing file-level cache continues to
   work; the new layer sits *inside* a file compile.
6. Multi-thread safety: multiple LSP handlers can issue queries
   concurrently.
7. Clear "cache invalidation" story: a single change correctly
   invalidates all dependent queries.

### Non-goals

- **Cross-file incremental** beyond what the file-level cache already
  does. This doc focuses on within-file incremental. Cross-file
  (via dependencies) is an emergent benefit but not the design
  target.
- **Incremental MIR lowering** inside a function. The unit of work
  is a whole function. Don't chase sub-function granularity v1.
- **Incremental codegen.** LLVM and Cranelift both operate per-function
  internally; we memoize at the MIR-function boundary, not inside.
- **Persist queries to disk.** The file-level cache already persists
  final outputs. Query memoization is in-memory only (lives for the
  lifetime of a `Database` handle).
- **Support for self-modifying code** / plugin reloading. Static
  compile model.
- **Salsa 0.x's full feature set** (cancellation, durability, etc.)
  in v1. Scope down to what we need.

---

## 4. Surface

### 4.1 `Database` handle

```rust
// crates/riven-core/src/db/mod.rs (new)

pub struct Database {
    // Internal: memoization tables indexed by query key
}

impl Database {
    pub fn new() -> Self;

    // Set inputs — the leaves of the query graph
    pub fn set_source(&mut self, file: FileId, text: Arc<str>);
    pub fn set_compiler_version(&mut self, ver: &str);

    // Run queries — returns memoized or computed result
    pub fn tokens(&self, file: FileId) -> Arc<Result<Vec<Token>, Vec<Diagnostic>>>;
    pub fn ast(&self, file: FileId) -> Arc<Result<ast::Program, Vec<Diagnostic>>>;
    pub fn hir(&self, file: FileId) -> Arc<HirProgram>;
    pub fn symbols(&self, file: FileId) -> Arc<SymbolTable>;
    pub fn typeck(&self, file: FileId) -> Arc<TypeCheckResult>;
    pub fn typeck_fn(&self, fn_id: DefId) -> Arc<HirFuncDef>;
    pub fn borrow_check(&self, file: FileId) -> Arc<Vec<BorrowError>>;
    pub fn mir(&self, fn_id: DefId) -> Arc<MirFunction>;

    pub fn invalidate_file(&mut self, file: FileId);
}

pub type FileId = u32;
```

### 4.2 Migration shim

Existing APIs continue to work — they just become thin wrappers:

```rust
// typeck/mod.rs
pub fn type_check(program: &ast::Program) -> TypeCheckResult {
    let mut db = Database::new();
    let file = db.intern_program(program);
    Arc::try_unwrap(db.typeck(file)).unwrap_or_else(|a| (*a).clone())
}
```

No call site changes.

### 4.3 LSP consumption

`riven-ide/src/analysis.rs::analyze` becomes:

```rust
pub fn analyze_incremental(db: &mut Database, source: &str) -> AnalysisResult {
    let file = db.intern_source(source);
    AnalysisResult {
        program: Some((*db.hir(file)).clone()),
        symbols: Some((*db.symbols(file)).clone()),
        type_context: Some((*db.typeck(file)).type_context.clone()),
        diagnostics: (*db.typeck(file)).diagnostics.clone(),
        borrow_errors: (*db.borrow_check(file)).clone(),
        source: source.to_string(),
        line_index: LineIndex::new(source),
    }
}
```

The LSP holds a single `Database` across requests. Per-document
`set_source` on `didChange`.

---

## 5. Architecture / design

### 5.1 Query framework choice

Three options:

**Option A: Adopt `salsa`.** The de facto Rust query framework;
rust-analyzer uses it. Pro: battle-tested, rich feature set,
incremental recomputation built in. Con: 10-20k lines of dep, macro-heavy,
salsa 0.x → 0.y migration pain has historically been real.

**Option B: Adopt a minimal hand-rolled memoizer.** `DashMap<Key,
Arc<Value>>` per query, with manual invalidation. Pro: tiny, full
control. Con: reinvent-the-wheel for dependency tracking; manual
invalidation is error-prone.

**Option C: Adopt `salsa-macros` minimal subset or similar lightweight
lib.** Middle ground. Evaluate `dashmap` + a minimal query trait.

**Recommendation: Option B (hand-rolled) for v1.** Riven's query
graph is small (~8 query types). The invalidation discipline is
achievable without salsa's machinery. Revisit if the graph grows
past ~20 nodes.

Rationale: salsa is a big dep for the shape of the problem. rust-analyzer
has 200+ queries and benefits from salsa enormously. Riven has ~8
queries (§5.2). The manual code is maybe 400 lines.

### 5.2 Query graph

```
set_source(file, text)               [input]
  └─▶ tokens(file)                   [derived]
        └─▶ ast(file)                [derived]
              └─▶ symbols(file)      [derived]
                    └─▶ hir(file)    [derived]
                          └─▶ typeck(file) ─┬─▶ diagnostics(file)
                                            └─▶ typeck_fn(fn_id) [per fn]
                                                  └─▶ mir(fn_id)
                                                        └─▶ codegen_fn(fn_id)
                          └─▶ borrow_check(file) ─▶ borrow_diag(file)
```

Each arrow = dependency. When `set_source` fires, the downstream
memoization entries are invalidated transitively.

For within-file per-function invalidation, `typeck_fn(fn_id)` keys on:

- The function's AST node (content hash of its body)
- The file's full `symbols` (to resolve references outside the fn)

If two edits modify different functions, their `typeck_fn` entries
are independent. This is the per-function granularity win.

### 5.3 Keying strategy

Every query has a **key type** and a **memoized value**:

| Query | Key | Value |
|---|---|---|
| `tokens(file)` | `(FileId, content_hash)` | `Arc<Result<Vec<Token>, Vec<Diagnostic>>>` |
| `ast(file)` | `(FileId, tokens_hash)` | `Arc<Result<ast::Program, Vec<Diagnostic>>>` |
| `symbols(file)` | `(FileId, ast_hash)` | `Arc<SymbolTable>` |
| `hir(file)` | `(FileId, symbols_hash)` | `Arc<HirProgram>` |
| `typeck(file)` | `(FileId, hir_hash)` | `Arc<TypeCheckResult>` |
| `typeck_fn(fn_id)` | `(DefId, fn_ast_hash, symbols_hash)` | `Arc<HirFuncDef>` |
| `borrow_check(file)` | `(FileId, typeck_hash)` | `Arc<Vec<BorrowError>>` |
| `mir(fn_id)` | `(DefId, typeck_fn_hash)` | `Arc<MirFunction>` |

Hashes cascade: each level's hash feeds the next level's key, so
changing content auto-invalidates everything downstream.

Implementation: `DashMap<Key, Arc<Value>>` per query. Thread-safe
concurrent reads; single-writer per key via `entry()`.

### 5.4 Invalidation

Invalidation is structural: when `set_source` mutates the input,
all memoized entries tied to the old content hash remain in place
but are never looked up again (the new hash bypasses them). A periodic
sweep (on `invalidate_file` or a LRU policy) can reclaim memory.

Because queries are content-addressed, **there is no explicit
invalidation**. Old entries linger until evicted. Memory bound: cap
each query's `DashMap` at e.g. 512 entries with LRU eviction. Details
in §5.8.

### 5.5 Per-function typecheck decomposition

The current `typeck::type_check` is a big function that walks the
whole program. To split per-function, refactor into:

1. **Global phase** — resolve names, build `SymbolTable`, collect
   impls. Depends only on AST. Cache as `symbols(file)`.
2. **Per-function phase** — infer types within one function body,
   given the `SymbolTable`. Cache as `typeck_fn(fn_id)`.

The per-function phase is already mostly scoped: `InferenceEngine`
(`typeck/infer.rs`) walks functions one at a time in `infer_program`
(grep for the loop). The refactor is mechanical.

Borrow-check similarly factors — it already runs per-function.

### 5.6 Content hashes

Use SHA-256 (already in `rivenc/src/cache/hash.rs`). Hash:

- `tokens`: the source text + compiler version.
- `ast`: the full token stream serialized.
- `fn_ast_hash`: the function's `ast::FuncDef` serialized (via
  `bincode` or similar).

Serialization cost is the main perf concern. For `fn_ast_hash`, use
a cheaper identity: the function's start/end byte span combined with
the file's current tokens hash. This is not content-addressed but is
sufficient for invalidation-on-edit — two different functions with
identical bodies at different locations are still correctly
distinguished.

### 5.7 Thread safety

`Database` uses `DashMap` + `Arc<Value>` so readers never block
writers for the *same* key. Multiple threads calling `db.typeck(file)`
on the first read will race to compute; the loser of the race drops
its result. Acceptable.

`set_source` takes `&mut self`; the LSP guards with its existing
`RwLock`.

### 5.8 Memory management

Each query map is capped at 512 entries. LRU eviction on insertion.
Tests should include a stress path: edit a file 1000 times, assert
the `Database` stays under N MB.

For LSP, we expect at most ~10 open files × ~8 queries × ~500 entries
each = 40k entries. Most `Arc<HirProgram>` values are ~10-100 KB, so
upper bound ~4 GB, lower bound ~400 MB. Cap map sizes aggressively;
monitor in practice.

### 5.9 Integration with `rivenc` file-level cache

The file-level cache (`rivenc/src/cache/`) remains the source of
truth for cached *object files*. The new `Database` holds only
in-memory intermediate results for the lifetime of a `rivenc`
invocation.

Relationship:

```
rivenc invocation
  └── Database (in-memory, one per process)
         └── File-level cache (on-disk, shared across processes)
```

On cache hit at the file level, the in-memory Database isn't populated
for that file (we don't re-parse). On cache miss, the Database
populates as queries run.

### 5.10 Error recovery

Failed queries (lex errors, parse errors) still produce values (an
`Err` variant). They're memoized like successful ones. Downstream
queries that depend on a failed upstream short-circuit and produce
their own "no input" variants. No panics propagate through the query
layer.

---

## 6. Implementation plan

### Files to touch

| Phase | File | Change |
|---|---|---|
| 1 | `crates/riven-core/src/db/mod.rs` *new* | Database skeleton |
| 1 | `crates/riven-core/src/db/query.rs` *new* | Memoization primitive (DashMap wrapper) |
| 1 | `crates/riven-core/Cargo.toml:29-36` | Add `dashmap` dep |
| 2 | `crates/riven-core/src/lib.rs` | Re-export `db::Database` |
| 2 | `crates/riven-core/src/db/queries/tokens.rs` *new* | Lex query |
| 2 | `crates/riven-core/src/db/queries/ast.rs` *new* | Parse query |
| 2 | `crates/riven-core/src/db/queries/symbols.rs` *new* | Resolve query |
| 2 | `crates/riven-core/src/db/queries/hir.rs` *new* | HIR query |
| 3 | `crates/riven-core/src/typeck/mod.rs:37-80` | Refactor into split global + per-fn |
| 3 | `crates/riven-core/src/db/queries/typeck.rs` *new* | File-level typeck query |
| 3 | `crates/riven-core/src/db/queries/typeck_fn.rs` *new* | Per-fn typeck query |
| 4 | `crates/riven-core/src/borrow_check/mod.rs` | Factor to per-fn |
| 4 | `crates/riven-core/src/db/queries/borrow_check.rs` *new* | Borrow-check query |
| 4 | `crates/riven-core/src/db/queries/mir.rs` *new* | MIR query |
| 5 | `crates/riven-ide/src/analysis.rs:43-88` | Accept optional `&mut Database` |
| 5 | `crates/riven-lsp/src/server.rs:17-20` | Hold a `Database` in `ServerState` |
| 6 | `crates/rivenc/src/main.rs:503-576` | Optionally use `Database` for within-file caching |

### Phase breakdown

**Phase 1 — Skeleton + one query (2 days).**
Database + tokens-query only. Prove the shape works end-to-end on a
single pipeline stage.

**Phase 2 — Full pipeline, file-level only (5 days).**
tokens → ast → symbols → hir → typeck → borrow_check, all as whole-file
queries. No per-function yet. This already delivers value for the LSP
(repeated hovers on the same file reuse the HIR).

**Phase 3 — Per-function typeck (5 days).**
The hard refactor. Split typeck into global + per-fn. Add
`typeck_fn(fn_id)` query. Validate with benchmarks.

**Phase 4 — Per-function borrow-check + MIR (3 days).**
Same pattern as Phase 3.

**Phase 5 — LSP integration (2 days).**
Plumb `Database` through `riven-ide::analyze_incremental`. Benchmark
LSP latency before/after.

**Phase 6 — `rivenc` integration (2 days, optional).**
For multi-file projects, use `Database` to avoid re-parsing shared
header-like modules. Often a no-op because of the file-level cache.

Total: 15-20 engineer-days for Phases 1-5.

### Benchmarks (separate, ongoing)

- 1000-line file, no changes → typeck should be <1 ms (pure cache
  lookup).
- 1000-line file, edit one function body → typeck should be
  <5 ms (invalidate one fn, re-check it).
- 1000-line file, add a `use` at top → typeck should be <50 ms
  (invalidates `symbols`, cascades to all `typeck_fn`).

Fold into `rivenc/benches/` or a new `riven-core/benches/incremental.rs`.

---

## 7. Interactions with other tier-3 items

- **Doc 01 (LSP).** This is the single biggest beneficiary. Phase 2
  lets LSP hover / definition / semantic-tokens reuse results between
  requests; Phase 3 makes on-keystroke analysis viable.
- **Doc 03 (test), doc 05 (bench).** Test/bench binaries benefit from
  faster incremental builds. No direct API change.
- **Doc 04 (doc generator).** `rivendoc` can hold a long-lived
  `Database` for watch-mode HTML regeneration.
- **Doc 07 (MIR opts).** New opt passes slot in as additional per-fn
  queries (`mir_optimized(fn_id)` depends on `mir(fn_id)`). Cache-friendly.
- **Doc 02 (debugger).** No direct interaction.

### Tier-1 dependencies

None hard. The query layer is lateral — it accelerates what's there.

---

## 8. Phasing

| Phase | Scope | Days | Ship value |
|---|---|---|---|
| 1 | Skeleton | 2 | Internal — unblocks phase 2 |
| 2 | Whole-file queries | 5 | LSP: repeated requests are fast |
| 3 | Per-fn typeck | 5 | LSP: on-keystroke typeck viable |
| 4 | Per-fn borrow + MIR | 3 | Full per-fn incremental |
| 5 | LSP integration | 2 | User-visible LSP speedup |
| 6 | rivenc integration (opt) | 2 | Minor |

---

## 9. Open questions & risks

1. **OQ-1 — Salsa vs hand-rolled.**
   Recommendation above (hand-rolled). If query count exceeds ~20,
   reconsider.
2. **OQ-2 — Shared mutable state in typeck.**
   `InferenceEngine` uses `TypeContext` for unification, and
   `SymbolTable` is mutated during inference (`update_ty` at
   `resolve/symbols.rs:206-217`). Per-fn parallel typeck requires
   either (a) cloning the context per fn, or (b) refactoring so
   inference is pure. (a) is simpler. Measure clone cost; typical
   TypeContext is small.
3. **OQ-3 — DefId stability across edits.**
   If a user adds a line at the top of a file, every subsequent `DefId`
   might shift (they're assigned sequentially in `SymbolTable`).
   That would bust per-fn caches for every later function. Solve by
   keying on (source-path, fn-name, item-index-within-parent) rather
   than raw DefId. Or: assign DefIds by name hash, not sequence.
4. **OQ-4 — Cancellation.**
   When the LSP receives a second `didChange` while a first
   `typeck_fn` is in flight, ideally the first cancels. salsa has
   cancellation; our hand-rolled version does not. v1: no cancellation;
   accept that stale work runs to completion. v2: use
   `AtomicBool::load` in tight loops for cooperative cancellation.
5. **R1 — Memory unbounded.**
   Need LRU eviction. Without it, long-running LSP sessions drift up.
   Test with a 24-hour fuzzer.
6. **R2 — Query correctness.**
   A missed invalidation (stale cache returning wrong result) is a
   silent bug. Mitigation: differential testing — run
   `analyze_incremental` vs `analyze_fresh` and assert identical
   output in CI.
7. **R3 — Thread-safety of `SymbolTable`.**
   `SymbolTable::update_ty` mutates. Per-fn queries access the same
   `Arc<SymbolTable>`. Either (a) make `SymbolTable` immutable after
   the global phase (recommended), or (b) wrap in `RwLock`. (a) is
   cleaner.
8. **OQ-5 — Query identity for anonymous functions (closures).**
   Closures get synthesized names (`closure_0`, `closure_1`) by the
   lowerer. These must be stable across edits. Recommendation: key
   by (enclosing-fn DefId, closure-index).
9. **OQ-6 — Serialization format for `fn_ast_hash`.**
   `bincode` for speed, `serde_json` for readability, a custom
   visit-and-hash pass for speed+stability. Recommend custom visitor
   that avoids deriving `Hash` on every AST node.
10. **R4 — Backward compat with existing typeck tests.**
    Phase 3's refactor changes `type_check` internally. The compat
    shim at §4.2 keeps the signature. Validate every existing
    `riven-core/src/typeck/tests.rs` test still passes.
11. **OQ-7 — When to evict the database.**
    On file close in LSP? On project switch? Policy to be decided.
    v1: never evict in-memory; flush on server shutdown.
12. **OQ-8 — Intern source strings.**
    Strings are currently `String` with owned bytes. An `Arc<str>` /
    `lasso` interner cuts memory for repeated copies. Low priority.

---

## 10. Test matrix

| Case | Assertion |
|---|---|
| Same source twice → second call is O(1) | Measure: second <1 ms |
| Edit one fn body | Only that fn's typeck_fn recomputes |
| Edit top of file | symbols invalidates; all typeck_fn's recompute |
| `Database` held across 100 edits | Memory bounded under cap |
| Concurrent `typeck(file)` calls | Single computation, both get same `Arc` |
| Query error cascades | Lex error → downstream queries return "no input" results |
| DefId remap doesn't break caches | Add blank lines, verify typeck_fn still hits |
| LSP stale-response avoidance | In-flight query result discarded on cancel (v2) |
| Differential: incremental vs fresh | Identical `diagnostics` + `borrow_errors` |
| File-level cache + Database | Both hit = O(1); file miss + DB miss = full compile |
| Closures with same body at different sites | Distinct queries (by enclosing-fn) |
| LRU eviction correctness | Query result still re-computable after eviction |
| 10 open files in LSP | Each has its own Database entries; cross-talk free |

Dedicated bench file `crates/riven-core/benches/incremental.rs`:

- `bench_repeated_typeck(c)`: 1000 identical calls to `db.typeck(file)`.
- `bench_edit_one_fn(c)`: 100 alternating edits to two fns.
- `bench_large_file(c)`: 5000-line generated fixture, repeated queries.
