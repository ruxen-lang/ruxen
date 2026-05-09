# 11 — Phase 3: incremental compilation (T3.06)

**Depends on:** prompt 10 (LSP needs the query layer to scale).
**Reads:** `docs/requirements/tier3_06_incremental_compile.md`.

## Goal

Replace full-pipeline-on-every-change with a salsa-style query system
that re-runs only what's downstream of the actual edit.

## Surface

```rust
trait QueryDb {
    fn parse(&self, file: FileId) -> Arc<HirProgram>;
    fn symbols(&self, file: FileId) -> Arc<SymbolTable>;
    fn typeck(&self, file: FileId) -> Arc<TypeckResult>;
    fn hir(&self, def: DefId) -> Arc<HirItem>;
    fn mir(&self, def: DefId) -> Arc<MirFunction>;
}
```

Memoize each query by input hash. Invalidate downstream when an
input file changes.

## TDD

1. Unit test: changing one method body must NOT re-typecheck unrelated
   modules.
2. Unit test: changing a public signature DOES re-typecheck callers.
3. Benchmark test: 100-file project edit-loop average <50ms after
   first build.
4. Integration test: LSP server using the query db produces
   identical diagnostics to full re-build.

## Implementation

- Adopt `salsa` 0.18+ as the query engine.
- Each existing pass (`parse`, `resolve`, `typeck`, `lower`) becomes
  a query.
- File contents stored as inputs.
- `cache/` directory persists query results across runs (already
  exists at `crates/rivenc/src/cache/`).

## Cross-cutting

- LSP (prompt 10) switches from debounced full re-analysis to query
  invalidation.
- `riven build` uses incremental cache; `riven build --release`
  always full-rebuilds for predictable artifacts.

## Definition of done

- [ ] Salsa query DB wired to all four pipeline stages.
- [ ] Edit benchmark <50ms p50 after first build.
- [ ] LSP uses queries (no debouncing required).
- [ ] CI green.
- [ ] CHANGELOG bullet.
