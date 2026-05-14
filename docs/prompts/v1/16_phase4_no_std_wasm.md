# 16 — Phase 4: `no_std` (T4.04) + WASM target (T4.03)

**Depends on:** prompt 14 (Phase 4 platform splits begin here).
**Reads:** `docs/requirements/tier4_04_no_std_embedded.md`,
`docs/requirements/tier4_03_wasm_target.md`.

## A. core / std split (T4.04)

### Goal
- Move type-system-only fundamentals (Option, Result, Iterator,
  Drop, basic numerics) into the `core` package.
- `std` re-exports `core` and adds heap-using types (String, Array,
  Map, io, fs, thread).
- A package-level `no_std` directive opts out of std (the directive
  sits at the top of the package's manifest module body, mirroring
  every other body directive).

### TDD
- Unit test: a `no_std`-marked package builds a minimal binary with
  only `core`.
- E2E fixture: `no_std` hello-world on a synthetic embedded target
  (LLVM `--target thumbv7em-none-eabi` build; run-test optional).
- Negative: heap allocation without the `alloc` package → E1400.

### Implementation
- Carve `crates/riven-core/runtime/std/core.rvn` and `std.rvn`.
- Compiler accepts the `no_std` directive and rejects heap-allocating
  types in scope.
- Panic strategy: `abort` only (Open Decision #5 ruling).

## B. WASM target (T4.03)

### Goal
- `riven build --target wasm32-unknown-unknown` produces a `.wasm`.
- Demonstrate calling Riven from JS via a wasm-bindgen-style shim.

### TDD
- Unit test invokes the LLVM backend with wasm32 triple, asserts
  artifact is valid wasm.
- Integration test: build the no_std hello-world for wasm; load
  with `wasmer` runner; assert exit success.

### Implementation
- LLVM backend already supports wasm32 targets; wire CLI to pass
  the triple through.
- Riven runtime needs a wasm-compatible variant: `runtime_wasm.c`
  with no syscalls (provide imports for `print`, `read`).
- Examples: add `examples/05-wasm-hello/` building a wasm artifact
  loaded by an HTML harness.

## Reserved error codes

- E1400 — heap allocation in `no_std` context
- E1401 — `std.*` import in `no_std` package
- E1402 — target triple unsupported

## Definition of done

- [ ] `core` and `std` packages separated; `no_std` directive works.
- [ ] WASM build green; example loads + executes in a JS host.
- [ ] CHANGELOG bullet.
