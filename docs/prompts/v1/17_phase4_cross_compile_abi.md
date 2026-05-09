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
Riven types annotated `@[repr(C)]` are FFI-stable. `riven cbindgen`
emits a `.h` for `extern fn` exports.

### TDD
- Unit test: `@[repr(C)] struct Point { x: Int, y: Int }` lowers to
  a 16-byte struct with predictable field order.
- Integration test: build a Riven cdylib + a C harness; assert C
  can call `extern fn riven_double(x: Int) -> Int`.
- E2E fixture exercising ABI round-trip.

### Implementation
- `extern "C" fn` syntax → ABI-stable function (already partially:
  P0.4 split repr/derive). Verify lowering uses C ABI.
- `riven cbindgen` walks public `extern fn` decls and emits a header.
- Document ABI stability promise in `docs/abi-stability.md`.

## Reserved error codes

- E1500 — `extern "C"` on non-`@[repr(C)]` aggregate
- E1501 — invalid target triple
- E1502 — sysroot for target not installed

## Definition of done

- [ ] Cross-compile to ≥4 triples in CI matrix.
- [ ] cbindgen emits a usable `.h` for a sample cdylib.
- [ ] C harness calls Riven and round-trips a `Point` struct.
- [ ] CHANGELOG bullet.
