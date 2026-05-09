# 18 — Phase 4: debugger + DWARF (T3.02 + P0.14)

**Depends on:** prompt 17.
**Reads:** `docs/requirements/tier3_02_debugger.md`.

## Decision

LLVM-only debug info for v1. Cranelift backend stays
release-mode-only. `riven build --debug` requires `--backend=llvm`.

## Goal

Generated binaries carry DWARF v5 line info, function info, and
local-variable info usable by `lldb` and `gdb`.

## TDD

- Unit test: compile a `.rvn` with `--backend=llvm --debug`, dump
  with `dwarfdump`, assert `.debug_line`, `.debug_info`,
  `.debug_str` present.
- Integration test: run lldb on the binary, run `b main`, assert
  the breakpoint hits with correct file:line.
- Test stepping: lldb `step` advances source line by source line.
- Test variable inspection: `frame variable` shows local names +
  values.

## Implementation

- Replace the 3-line stub at `crates/riven-core/src/codegen/llvm/debug.rs`
  with full DWARF emission via LLVM's `DIBuilder` API.
- Map MIR locals → DWARF DIE entries.
- Map MIR span info → `.debug_line` table.
- Source-level types: `Int` → DW_TAG_base_type, struct/class →
  DW_TAG_structure_type, enum → DW_TAG_enumeration_type, etc.
- Handle generics: monomorphized fn name + concrete type DIEs.

## Cross-platform

- macOS: dsymutil step bundles DWARF into `.dSYM`; CLI must invoke
  it for `--debug` macOS builds.
- Linux: DWARF in-place in the ELF.

## Definition of done

- [ ] `riven build --debug --backend=llvm` produces fully debuggable
      binaries.
- [ ] lldb test suite (golden output) passes on Ubuntu + macOS.
- [ ] P0.14 closed.
- [ ] CHANGELOG bullet.
