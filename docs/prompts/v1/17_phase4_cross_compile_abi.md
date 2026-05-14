# 17 — Phase 4: cross-compile (T4.02) + stable ABI / cbindgen (T4.05)

**Depends on:** prompt 16.
**Reads:** `docs/requirements/tier4_02_cross_compilation.md`,
`docs/requirements/tier4_05_stable_abi_cbindgen.md`.

## A. Cross-compilation (T4.02)

### Goal
`riven build --target <triple>` emits artifacts for any LLVM-supported
triple. The release.yml already cross-builds aarch64-linux; generalize.

### TDD
- Unit test: `--target x86_64-unknown-linux-gnu` from macOS host
  produces an ELF.
- CI matrix expanded with at least: linux-aarch64, linux-x64,
  darwin-aarch64, darwin-x64.

### Implementation
- CLI accepts `--target` flag.
- Toolchain installs sysroot via `riven sysroot install <triple>`
  (new subcommand wrapping rustup-style downloads).
- `riven.toml` `[target.<triple>]` sections override link flags.

## B. Stable ABI + cbindgen (T4.05)

### Goal
Riven structs that carry `layout c` are FFI-stable. `riven cbindgen`
emits a `.h` for functions declared inside a `lib` block (the FFI-
export surface).

### TDD
- Unit test: `struct Point { layout c; x: Int; y: Int end }` lowers
  to a 16-byte struct with predictable field order.
- Integration test: build a Riven cdylib + a C harness; assert C
  can call a Riven-defined function with ABI-stable parameters
  (use the C-export surface that maps to `lib` semantics in
  reverse — final export-side syntax is the prompt-17 design
  decision; the spec covers `lib "..." ... end` only for imports).
- E2E fixture exercising ABI round-trip.

### Implementation
- For C imports, `lib "..." ... end` blocks declare the foreign
  surface (already partially: P0.4 split layout/derive).
- For C exports, decide the surface form during this prompt
  (candidates: a `lib "self" ... end` block, an `export` body
  directive, etc.). Whatever lands must lower to a function with
  C ABI.
- `riven cbindgen` walks every C-export declaration and emits a
  header.
- Document ABI stability promise in `docs/abi-stability.md`.

## Reserved error codes

- E1500 — C-exported function passes a non-`layout c` aggregate
- E1501 — invalid target triple
- E1502 — sysroot for target not installed

## Definition of done

- [ ] Cross-compile to ≥4 triples in CI matrix.
- [ ] cbindgen emits a usable `.h` for a sample cdylib.
- [ ] C harness calls Riven and round-trips a `Point` struct.
- [ ] CHANGELOG bullet.
