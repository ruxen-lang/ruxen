# Tier 3.02 — Debugger (DWARF + DAP)

Status: draft
Depends on: LLVM backend (feature-gated; already present); Tier-1 doc 05 (derive Debug) for pretty-printers
Blocks: none

---

## 1. Summary & motivation

Riven has no source-level debugger today. A developer cannot
`breakpoint` in VSCode, step over a Riven `let` statement, inspect
locals, see a call stack labeled with Riven function names, or use any
of the standard debugging conveniences that every mainstream
compiled language offers.

The absence is visible: `crates/riven-core/src/codegen/llvm/debug.rs`
is a three-line stub that says "Full DWARF debug info will be
implemented in a follow-up phase." No debug info is emitted on either
Cranelift or LLVM. The `rivenc` CLI has no `--debug`, `--emit-debug`,
or `-g` flag (`crates/rivenc/src/main.rs:40-68`). The `riven-cli`
`build` command has no debug-symbol knob (`crates/riven-cli/src/cli.rs:42-52`).

This doc specifies:

1. DWARF v5 emission through Inkwell's `DebugInfoBuilder` wired into
   the LLVM backend codegen.
2. A lightweight Debug Adapter Protocol (DAP) server, shipped as a new
   crate `riven-dap`, so VSCode / Neovim / any DAP-capable client can
   set breakpoints, step, and inspect locals in a Riven program.
3. Pretty-printer support for the v1 stdlib types (`String`, `Vec`,
   `Option`, `Result`, `HashMap`) — delivered as optional Python
   scripts for `lldb`/`gdb` that the DAP adapter auto-loads.

Out of scope for v1: (a) time-travel / reverse debugging;
(b) multithreaded debugging (tier-1 doc 02 hasn't landed); (c)
Cranelift DWARF emission at production quality; (d) a bespoke debugger
UI.

---

## 2. Current state

### 2.1 LLVM codegen — no debug info at all

`crates/riven-core/src/codegen/llvm/debug.rs:1-3`:

```rust
//! Debug info generation (stub).
//!
//! Full DWARF debug info will be implemented in a follow-up phase.
```

The module is included by `codegen/llvm/mod.rs:16` but contains no
symbols. No `DebugInfoBuilder::create_compile_unit`, no `DISubprogram`,
no `DILocation`, no `dbg.declare` intrinsic calls, no
`.debug_info` section in the emitted object file.

Verification (grep results from §Step 1 research): the only occurrence
of any DWARF-related term in the entire `crates/` tree is the comment
line above.

### 2.2 Cranelift codegen — no debug info

`crates/riven-core/src/codegen/cranelift.rs` (1127 lines) has no
calls into `cranelift-codegen`'s `debug::write_debuginfo` helper and
doesn't enable the `emit_dwarf` flag. Cranelift *can* emit DWARF for
line/column mappings and basic local-variable info, but the
integration work is non-trivial (see §5.3).

### 2.3 CLI flags

`rivenc` supports `--release`, `--backend=cranelift|llvm`,
`--opt-level=0..3|s|z` (`crates/rivenc/src/main.rs:41-57`) but has no
debug-info flag. `opt-level=0` produces unoptimized code but without
DWARF, so gdb/lldb show `??` for function names and `<optimized out>`
for locals.

`riven-cli` `build` has `--release` but no `--debug` / `-g` flag
(`crates/riven-cli/src/cli.rs:42-52`). `cargo`-style conventions
would give `-g` at opt-level-0 by default in debug profile.

### 2.4 Runtime C shim

`crates/riven-core/runtime/runtime.c` is compiled via `cc` in
`crates/riven-core/src/codegen/object.rs::compile_runtime`. A quick
grep shows no `-g` flag is passed. For a working debugger experience,
the runtime must also ship with DWARF — otherwise stepping into
`riven_string_concat` lands in assembly without source.

### 2.5 No DAP adapter

No crate, no source file, no dependency on any DAP library. Editors
cannot launch a Riven debugger today.

---

## 3. Goals & non-goals

### Goals

1. Running `riven build --debug` (or `rivenc --emit-debug`) produces an
   executable with DWARF v5 debug info embedded (or in a separate
   `.dwarf`/dSYM file on macOS).
2. Setting a breakpoint on a line of Riven source in VSCode/Neovim
   pauses execution at that line.
3. `next`, `step`, `finish`, `continue` all work at Riven-source
   granularity.
4. The `locals` panel shows every Riven `let` and function parameter
   currently in scope, by source name.
5. Primitive types (`Int`, `Float`, `Bool`, `String`, `&str`) display
   as their source value, not the underlying pointer / integer.
6. `Vec[T]`, `Option[T]`, `Result[T, E]` display as
   `Vec([1, 2, 3])`, `Some(42)`, `Err("msg")` via a pretty-printer.
7. Works at opt-level 0. Higher opt levels may show
   `<optimized out>` — standard behavior, no special work.
8. Runs on Linux (gdb + lldb) and macOS (lldb). Windows is a stretch.

### Non-goals

- **Cranelift debug-info parity.** Cranelift produces line tables but
  not local variable info of production quality. Ship v1 as
  "debug builds force `--backend=llvm`." Support Cranelift line-only
  debug info as a stretch goal.
- **Reverse / time-travel debugging.** rr is out of scope.
- **Conditional breakpoints evaluated by riven-lang expressions.**
  Use gdb/lldb's own expression evaluator; a Riven-expression parser
  in gdb is a much larger ask.
- **Multithreaded debugging.** Tier-1 doc 02 hasn't shipped. When it
  does, the single-thread assumption below will need updating.
- **A standalone debugger UI.** VSCode is the primary target via DAP.
- **Edit-and-continue.** Never.

---

## 4. Surface

### 4.1 CLI flags

```
rivenc hello.rvn --debug                 # adds -g at opt 0
rivenc hello.rvn --debug --opt-level=2   # debug info at -O2 (works but locals may be optimized out)
rivenc hello.rvn --release --debug       # DWARF + LLVM O2 — best for perf with partial debug
riven build --debug                      # project build with debug info
riven build --profile dev                # alias: --debug + -O0 (default in debug profile)
```

Default profile for `riven build` (no flags) gains `debug = true,
opt_level = 0`. `--release` continues to mean `debug = false, opt_level = 2`.

### 4.2 Debug-info output location

- **Linux:** DWARF sections embedded in the ELF. Split-DWARF (`.dwo`)
  is a stretch goal; not in v1.
- **macOS:** `dsymutil` post-process producing `<executable>.dSYM/`.
  Must be wired into `codegen::object::emit_executable`.
- **Windows:** Out of scope for v1.

### 4.3 DAP adapter

Shipped as `crates/riven-dap/` with a `riven-dap` binary. Communicates
over stdio per DAP convention. Launch arguments (JSON):

```json
{
  "type": "riven",
  "request": "launch",
  "program": "${workspaceFolder}/target/debug/my-program",
  "args": ["--flag"],
  "cwd": "${workspaceFolder}",
  "stopOnEntry": false,
  "backend": "lldb"          // or "gdb"; default "lldb" on macOS, "gdb" on Linux
}
```

The adapter delegates actual debugging to `lldb-dap` (shipped with
LLVM 18) or `gdb --interpreter=mi2`. `riven-dap` is primarily a thin
shim that:

1. Translates DAP requests to the underlying debugger's protocol.
2. Injects the pretty-printer scripts on `initialize`.
3. Normalizes paths to match Riven's `src/**.rvn` layout.
4. Handles `launch` vs `attach` vs `restart`.

Directly reusing `lldb-dap`/`vscode-lldb` is the recommended first
step (§5.4). A bespoke Rust DAP server is a v2.

### 4.4 VSCode integration

A new section in `editors/vscode/package.json`:

```json
"debuggers": [{
  "type": "riven",
  "label": "Riven Debugger",
  "program": "./out/debugger.js",
  "runtime": "node",
  "languages": ["riven"],
  "configurationAttributes": {
    "launch": {
      "required": ["program"],
      "properties": { ... }
    }
  }
}]
```

---

## 5. Architecture / design

### 5.1 DWARF emission via Inkwell

Inkwell exposes LLVM's `DIBuilder` under `inkwell::debug_info`. The
plan:

1. On `CodeGen::new`, if `debug_info_enabled`, create a
   `DebugInfoBuilder` plus a `DICompileUnit`:
   ```rust
   let (di_builder, di_cu) = module.create_debug_info_builder(
       /* allow_unresolved = */ true,
       DWARFSourceLanguage::C,   // closest available; see §9
       &file_name, &dir, "rivenc",
       /* is_optimized = */ opt_level > 0,
       /* flags = */ "",
       /* runtime_ver = */ 0,
       /* split_name = */ "",
       DWARFEmissionKind::Full,
       /* dwo_id = */ 0,
       /* split_debug_inlining = */ false,
       /* debug_info_for_profiling = */ false,
   );
   ```
2. For each MIR function, create a `DISubprogram` at the function's
   source span. Attach it as the LLVM function's `!dbg` metadata.
3. For each MIR local, create a `DILocalVariable` with the local's
   name, source span, and type (see §5.2). Emit
   `llvm.dbg.declare(alloca, var, expr)` at the local's stack slot
   alloca site.
4. For each MIR instruction, compute the source-line location from
   `MirInst`'s span (MIR currently lacks span info — see §5.5 below)
   and attach a `DILocation` to the corresponding LLVM instruction via
   `set_debug_location`.
5. On `CodeGen::finish`, call `di_builder.finalize()` before
   `module.verify()`.

Reference: [Inkwell debug info example][inkwell-dbg] and LLVM's own
[SourceLevelDebugging.rst][llvm-dbg] document.

### 5.2 Type → `DIType` mapping

Each Riven `Ty` maps to a DWARF type tag:

| Riven `Ty` | DWARF kind | DIType construction |
|---|---|---|
| `Int`, `Int64` | `DW_TAG_base_type`, `DW_ATE_signed`, 64 bits | `create_basic_type` |
| `Int32`, `Int16`, `Int8` | `DW_ATE_signed` at the matching width | ditto |
| `UInt`, `USize` | `DW_ATE_unsigned` | ditto |
| `Float`, `Float64` | `DW_ATE_float`, 64 bits | ditto |
| `Float32` | `DW_ATE_float`, 32 bits | ditto |
| `Bool` | `DW_ATE_boolean`, 8 bits | ditto |
| `Char` | `DW_ATE_UTF`, 32 bits | ditto |
| `String` | `DW_TAG_structure_type` with `ptr: *u8`, `len: usize`, `cap: usize` members | `create_struct_type` |
| `&str` | `DW_TAG_structure_type` with `ptr: *u8`, `len: usize` | ditto |
| `Vec[T]` | struct with `ptr: *T, len: usize, cap: usize` | ditto |
| `Option[T]` | `DW_TAG_enumeration_type` with variants `None, Some(T)` | `create_enumeration_type` |
| `Result[T, E]` | similar to Option | ditto |
| `Ref(T)` / `Ref(mut T)` | `DW_TAG_pointer_type` / `DW_TAG_reference_type` | `create_pointer_type` |
| `Class { name }` / `Struct { name }` | `DW_TAG_structure_type` | recurse into fields |
| `Enum { name }` | `DW_TAG_variant_part` under a structure | see §9 OQ-2 |
| `Fn { params, ret }` | `DW_TAG_subroutine_type` | `create_subroutine_type` |
| `Tuple(elements)` | struct with unnamed fields | ditto |
| `Array(T, n)` | `DW_TAG_array_type` with bound `n` | `create_array_type` |

The mapping lives in a new helper `codegen/llvm/debug.rs::type_to_di`
that memoizes per-`Ty` (types can be recursive via class self-reference).

### 5.3 Cranelift debug-info story

Cranelift 0.130's `cranelift-codegen::isa::unwind` emits `.eh_frame`
for stack walking. The `cranelift-codegen::debug` module is incomplete
and internal. `cranelift-object` does not accept a `DebugInfo` directly;
integration requires writing DWARF sections out-of-band via the
`gimli` crate, then attaching them to the object file.

Recommended v1 policy:

- `--debug` with `--backend=cranelift` emits **line-table only** DWARF
  (`DW_TAG_compile_unit` + `.debug_line`) if feasible. Local variables
  and type info: not emitted.
- `--debug` with `--backend=llvm` emits full DWARF (§5.1).
- When the user does `--debug` without specifying a backend, default
  to `--backend=llvm` and print one informational line: `note:
  --debug implies --backend=llvm (Cranelift debug info is limited)`.

Implementation of the line-table-only path for Cranelift is optional.
Don't block v1 on it.

### 5.4 DAP adapter design

Two options:

**Option A (recommended for v1): reuse `lldb-dap`.**
LLVM 18 ships `lldb-dap` — a DAP server that drives lldb. Our
`riven-dap` binary:

1. Starts `lldb-dap` as a child process.
2. Forwards DAP stdin/stdout.
3. Intercepts `initialize` to inject pretty-printer commands
   (`command script import <path>/riven_pp.py`).
4. Intercepts `setBreakpoints` to convert `.rvn` paths — no conversion
   needed in practice, since LLVM emits paths exactly as seen by rivenc.
5. Adds one custom DAP event: `riven/cacheInvalidated` when the user
   rebuilds, prompting the client to reload symbols.

**Option B: bespoke Rust DAP server.**
Implement DAP message handling in Rust using the `dap-rs` crate (or
roll our own). Drive lldb via `lldb-sys` or gdb via MI.
Non-trivial; defer to v2.

### 5.5 MIR span tracking

Current `MirInst` variants (`crates/riven-core/src/mir/nodes.rs:184-304`)
do **not** carry source spans. For DWARF line numbers, each instruction
needs one. Add:

```rust
pub struct MirInstWithSpan {
    pub inst: MirInst,
    pub span: Span,
}
```

or, less invasively, add a parallel `Vec<Span>` aligned with the
`instructions: Vec<MirInst>` inside each `BasicBlock`. The lowerer
already has access to HIR spans — threading them through is O(file-size)
work.

A middle-ground: annotate only `BasicBlock::terminator` and one
representative instruction per "source statement boundary". This loses
fine-grained stepping but lets line-level breakpoints work. Phase the
upgrade: v1 coarse-grained, v2 per-instruction.

### 5.6 Runtime debug info

Update `codegen::object::compile_runtime` to pass `-g` when the
compile request is a debug build. Runtime functions (`riven_print`,
`riven_vec_push`, etc.) then appear by name in the stack trace.
Trivial change; ~5 lines.

### 5.7 Pretty-printers

Ship a single Python script `runtime/debug/riven_pp.py` that:

- Registers an lldb `type_summary` for each v1 stdlib type.
- Registers equivalent gdb pretty-printers via
  `gdb.printing.RegexpCollectionPrettyPrinter`.

Install location: `<installroot>/share/riven/debug/riven_pp.py`
(parallel to the existing runtime.c location at
`<installroot>/lib/runtime.c`). `riven-dap` locates it via the same
search order as `find_runtime_c` (`codegen/mod.rs:27-65`).

Example lldb printer for `Vec[T]`:

```python
def vec_summary(valobj, _):
    ptr = valobj.GetChildMemberWithName('ptr')
    length = valobj.GetChildMemberWithName('len').GetValueAsUnsigned()
    if length == 0:
        return "[]"
    elems = []
    for i in range(min(length, 10)):
        elem_addr = ptr.GetValueAsUnsigned() + i * 8
        # ... load element by type
    suffix = ", ..." if length > 10 else ""
    return "[" + ", ".join(elems) + suffix + "]"
```

---

## 6. Implementation plan

### Files to touch

| Phase | File | Change |
|---|---|---|
| 1 | `crates/rivenc/src/main.rs:40-68` | Add `--debug` flag parsing; propagate to backend |
| 1 | `crates/riven-cli/src/cli.rs:42-52` | Add `--debug` + `--profile` to `Build` and `Run` |
| 1 | `crates/riven-cli/src/build.rs` | Thread debug flag into `codegen::compile_with_options` |
| 1 | `crates/riven-core/src/codegen/mod.rs:67-128` | Extend `Backend::Llvm` with `debug: bool`; pass through |
| 2 | `crates/riven-core/src/codegen/llvm/mod.rs` | Plumb `debug: bool` into `CodeGen::new` |
| 2 | `crates/riven-core/src/codegen/llvm/debug.rs` | Replace stub with `DebugInfoBuilder` wrapper (~300 lines) |
| 2 | `crates/riven-core/src/codegen/llvm/emit.rs` | Attach `DILocation` to each generated LLVM instruction |
| 2 | `crates/riven-core/src/mir/nodes.rs` | Add `Span` to `MirInst` (or parallel vec in `BasicBlock`) |
| 2 | `crates/riven-core/src/mir/lower.rs` | Propagate HIR spans through lowering |
| 2 | `crates/riven-core/src/codegen/object.rs` | Pass `-g` to `cc` when compiling the runtime |
| 2 | `crates/riven-core/runtime/runtime.c` | No change (already compiles with `-g` if passed) |
| 3 | `crates/riven-dap/` *new* | DAP shim (~400 lines) |
| 3 | `crates/riven-dap/Cargo.toml` *new* | Deps: `serde`, `serde_json`, `anyhow` |
| 3 | `runtime/debug/riven_pp.py` *new* | lldb + gdb pretty-printers |
| 4 | `editors/vscode/package.json` | Register `type: "riven"` debugger |
| 4 | `editors/vscode/src/debugger.ts` *new* | Thin debug-adapter descriptor factory |
| 5 | `install.sh` | Copy `share/riven/debug/riven_pp.py`; copy `riven-dap` binary |
| 5 | `.github/workflows/release.yml:79-103` | Stage `riven-dap` into the tarball |

### Phase breakdown

**Phase 1 — CLI plumbing (1 day).**
Add flags, thread them through. No user-visible debug info yet, but
`rivenc --debug hello.rvn` compiles and runs (just without DWARF).

**Phase 2 — LLVM DWARF emission (1 week).**
- Day 1: add `Span` to MIR instructions; propagate through lowering.
- Day 2-3: `DebugInfoBuilder` wiring; `DICompileUnit` + `DISubprogram`.
- Day 4: type → `DIType` mapping for primitives.
- Day 5: `dbg.declare` for locals; `DILocation` on instructions.
- Day 6-7: struct / enum / reference type mapping; `lldb` smoke test
  (breakpoint + step + locals).

**Phase 3 — DAP adapter (3-4 days).**
- Day 1: `riven-dap` skeleton that spawns `lldb-dap` as a child.
- Day 2: `initialize` / `launch` / `setBreakpoints` forwarding.
- Day 3: pretty-printer script loading.
- Day 4: end-to-end smoke test from VSCode.

**Phase 4 — VSCode integration (1 day).**
Register the debugger type; package a launch-config template.

**Phase 5 — Distribution (0.5 day).**
Ship `riven-dap` + `riven_pp.py` in the release tarball.

**Phase 6 — Pretty printers for stdlib (ongoing; co-develop with Tier-1
doc 01).**

Total: 12-16 engineer-days.

---

## 7. Interactions with other tier-3 items

- **Doc 01 (LSP).** LSP can emit DAP run/debug commands via `codeLens`
  once the debugger ships. Not required for v1.
- **Doc 03 (tests).** `riven test --debug` should drop the test binary
  into a debugger session. Plumbing in `riven test` subcommand.
- **Doc 06 (incremental).** Debug builds must not be cached
  differently based on whether `--debug` was passed — the cache key
  already includes `flags` (`rivenc/src/cache/driver.rs` via
  `BuildOptions.flags`), so debug vs non-debug builds produce different
  cache entries naturally. Verify on landing.
- **Doc 07 (MIR opts).** At opt level 0, no MIR opts run. Above opt
  level 0, locals may be eliminated by DCE — this is fine and matches
  rustc/clang behavior. Document it.
- **Tier-1 doc 05 (derive Debug).** Pretty-printers for user classes
  benefit enormously from `#[derive(Debug)]`: if the type derives
  `Debug`, the printer can delegate to that impl instead of hand-rolling
  field walks. Phase 6 is where the two meet.

---

## 8. Phasing

| Phase | Scope | Engineer-days | Value shipped |
|---|---|---|---|
| 1 | CLI plumbing | 1 | `--debug` recognized |
| 2 | LLVM DWARF emission | 7 | `lldb ./prog` shows source lines + locals |
| 3 | DAP adapter | 4 | VSCode breakpoints work |
| 4 | VSCode integration | 1 | One-click debug in VSCode |
| 5 | Distribution | 0.5 | Released binaries include debugger |
| 6 | Stdlib pretty-printers | co-dev | `Vec` / `Option` / etc. display nicely |

---

## 9. Open questions & risks

1. **OQ-1 — DWARF source language tag.**
   DWARF standard has no Riven-specific language code.
   `DW_LANG_C` produces reasonable behavior in lldb and gdb.
   Alternatives: `DW_LANG_Rust` (lldb's rust plugin may misinterpret
   Riven structures). Recommend `DW_LANG_C` for v1.
2. **OQ-2 — Enum DWARF encoding.**
   DWARF v5 added `DW_TAG_variant_part` for sum types. Inkwell/LLVM 18
   support it. For consistency with gdb/lldb versions that might not,
   we may want a fallback: emit enums as a tagged struct. Decide which
   encoding to ship; `DW_TAG_variant_part` is cleaner if tooling handles it.
3. **OQ-3 — Location of DWARF on macOS.**
   dSYM bundles vs embedded? Embedded is simpler for distribution;
   dSYM is the platform convention. Use embedded for v1; add
   `dsymutil` post-processing in v2.
4. **OQ-4 — "Debug" vs "release with debug info".**
   Rust distinguishes `--release` (optimized, no DWARF) from
   `--release` with `[profile.release] debug = true` (optimized + DWARF).
   Riven should follow: `--release --debug` produces a high-opt build
   with debug info. Validated in §4.1.
5. **OQ-5 — `riven-dap` transport.**
   VSCode's DAP usually runs over stdio. For remote debugging, TCP
   transport is needed. Defer TCP to v2.
6. **R1 — Inkwell DIBuilder API surface stability.**
   Inkwell's `debug_info` module has seen churn. Lock the inkwell
   version in `riven-core/Cargo.toml:30` and test against LLVM 18
   specifically. Document LLVM 18 as required for debug builds.
7. **R2 — lldb vs gdb differences.**
   lldb handles some DWARF edges gdb doesn't and vice versa. Test the
   v1 stdlib pretty-printers on both. Budget a day for
   compatibility fixes.
8. **R3 — Span propagation into MIR is an invasive change.**
   Adding `Span` to `MirInst` touches `lower.rs` at ~100+ sites.
   Mitigation: use a parallel `Vec<Span>` keyed by instruction index
   in `BasicBlock`. Smaller diff, slightly less ergonomic.
9. **R4 — Debug info bloats binary size 2-4×.**
   Normal. Document in the release notes. Split-DWARF in v2 if users
   complain.
10. **R5 — LLVM 18 is a hard dependency for debug builds.**
    Users on LLVM 17 can't get debug info. Be clear in docs.
11. **OQ-6 — How do we debug the C runtime?**
    If a Riven program crashes inside `riven_vec_push`, the user
    wants to step into the C source. This works automatically if
    runtime.c is compiled with `-g` (§5.6). Verify on each platform.
12. **OQ-7 — Windows.**
    Debug info on Windows (`pdb` format) is a distinct code path.
    Out of scope v1; revisit when Windows support lands.

---

## 10. Test matrix

| Scenario | Assertion |
|---|---|
| Compile `hello.rvn --debug` | Binary links; `readelf --debug-dump=info` prints a compile unit |
| Breakpoint at `def main` | lldb stops at first line |
| Step over `let x = 42` | Step moves to next source line |
| `frame variable` shows `x = 42` | lldb prints the integer value |
| String local shows source text | `frame var s` prints `"hello"` not pointer address |
| `Vec[Int]` local with pretty-printer | Prints `[1, 2, 3]` |
| `Option::Some(5)` | Prints `Some(5)` |
| Nested function call | Backtrace shows parent → callee with correct names |
| `--release --debug` | Optimized binary still has locals when DCE didn't eliminate them |
| `--debug` with unused local | Either printed or `<optimized out>` — not a crash |
| VSCode launch.json | One-click debug hits breakpoint |
| Crash inside runtime.c | Backtrace shows `riven_vec_push` frame + source if runtime built with -g |

Add one integration test that checks ELF has a `.debug_info` section:
```rust
#[test]
fn debug_build_has_debug_info() {
    // Compile with --debug, parse the ELF header, assert DWARF is present.
}
```

This is the minimum that keeps the pipeline honest.

[inkwell-dbg]: https://thedan64.github.io/inkwell/inkwell/debug_info/index.html
[llvm-dbg]: https://llvm.org/docs/SourceLevelDebugging.html
