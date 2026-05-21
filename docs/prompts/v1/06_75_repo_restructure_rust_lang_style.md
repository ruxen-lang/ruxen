# 06.75 — Repo restructure to rust-lang/rust-style layout

> **Status: 🟡 Mostly shipped** (audited 2026-05-21). Layout
> restructure done: `compiler/{riven_core,riven_driver}/` +
> `src/{rivenc,riven_lsp}/` + `library/std/<pkg>/{src,runtime}/` in
> tree; no `crates/` directory. Per-module C lives under each
> package's `runtime/` dir; `library/std/core/runtime/runtime.h`
> is the cross-package header. `resolve/mod.rs` is 649 LOC (under
> the 1200 cap). **Pending:** `typeck/infer.rs` is 2279 LOC (cap was
> 1200) and `codegen/runtime.rs` is 739 LOC (cap was 300) — these
> two splits never happened. See `STATUS.md`.

**Depends on:** #06.5 (sync I/O completeness) must fully close — T1–T7
all merged. Doing this mid-#06.5 generates merge conflicts on every
in-flight File/fs/Duration/TCP/BufReader branch.
**Reads:** the current repo layout under `crates/`, the rust-lang/rust
layout (`compiler/`, `library/`, `src/`, `tests/` at repo root), and
`docs/STRATEGY.md` §"What's shipped today".
**Slot:** runs between Phase 2 (#06.5) and Phase 3 (#07). It is the
last cosmetic act of Phase 2 and the first thing #07 (const generics),
#10 (LSP), and #11 (incremental) build on.

## Why this prompt exists

Today `crates/riven-core` is a single 26 KLOC crate that owns:

```
crates/riven-core/
  runtime/runtime.c       5 285 LOC   (every C symbol; one TU)
  src/resolve/mod.rs      6 943 LOC   (stdlib registrations + scopes)
  src/typeck/infer.rs     2 895 LOC   (every method-resolver arm)
  src/codegen/runtime.rs    977 LOC   ("Class_method" → "riven_*")
  src/mir/                  …
  src/parser/               …
  src/codegen/{llvm,cranelift}/  …
  src/{lexer,hir,borrow_check,diagnostics,formatter}/  …
```

Three concrete problems this layout causes:

1. **Rebuild blast radius.** Every typeck-only edit (a single new
   method resolver) recompiles all of codegen + mir + borrow_check.
   Splitting into per-phase crates makes cargo skip 60–80 % of that.

2. **Merge-conflict surface.** #06.5 T2 alone landed +636 lines in
   `runtime.c` and +139 in `resolve/mod.rs`. Every Phase 2 stdlib
   prompt has touched these same files; #06.5 T3–T6 will keep
   doing it. The recent Codex-parallel hashmap fix that got
   accidentally folded into T2's commit is a direct consequence —
   one shared file, two threads of work, no per-module boundary.

3. **Code locality.** Reading "how is `std.io.File` wired?" today
   requires reading 4 files in 4 different parts of the tree
   (`runtime.c` lines 1700+, `resolve/mod.rs` lines 700+,
   `typeck/infer.rs`, `codegen/runtime.rs`). After the split it's
   a single directory: `library/std/src/io/` (Riven-side) +
   `library/runtime/io.c` (C-side) +
   `compiler/riven_resolve/src/stdlib/io.rs` (registration).

The win compounds with every later prompt — #10 (LSP) and #11
(incremental) both rely on phase-level rebuild boundaries we don't
currently have.

## Target layout

```
riven/
├── compiler/                       # one crate per compiler phase
│   ├── riven_lexer/
│   ├── riven_parser/               # was crates/riven-core/src/parser/
│   ├── riven_ast/                  # AST types currently inside parser
│   ├── riven_hir/
│   ├── riven_resolve/
│   │   └── src/
│   │       ├── lib.rs              # scope-table + import resolution only
│   │       └── stdlib/             # per-namespace builtin registrations
│   │           ├── mod.rs          # registry orchestrator
│   │           ├── primitives.rs   # Int / Float / Bool / String / Char / U8 / …
│   │           ├── collections.rs  # Array / HashMap / HashSet
│   │           ├── option.rs
│   │           ├── result.rs
│   │           ├── iter.rs
│   │           ├── io.rs           # Stdin/Stdout/Stderr/File/BufReader/BufWriter
│   │           ├── fs.rs
│   │           ├── net.rs
│   │           ├── time.rs
│   │           ├── process.rs
│   │           ├── env.rs
│   │           ├── fmt.rs
│   │           └── thread.rs
│   ├── riven_typeck/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── infer.rs            # core inference engine (currently 2 895 LOC)
│   │       └── method_resolvers/   # per-namespace `.method()` arms
│   │           ├── mod.rs
│   │           ├── primitives.rs   ├── option.rs    ├── result.rs
│   │           ├── string.rs       ├── array.rs     ├── iter.rs
│   │           ├── io.rs           ├── fs.rs        ├── net.rs
│   │           └── time.rs
│   ├── riven_mir/                  # MIR types + lowering passes
│   ├── riven_borrowck/
│   ├── riven_codegen_shared/       # codegen/runtime.rs split per namespace
│   │   └── src/
│   │       ├── lib.rs
│   │       └── runtime_table/      # "Class_method" → "riven_*" maps
│   │           ├── mod.rs
│   │           ├── io.rs           ├── fs.rs        ├── net.rs
│   │           ├── time.rs         ├── process.rs   └── collections.rs
│   ├── riven_codegen_cranelift/
│   ├── riven_codegen_llvm/
│   ├── riven_diagnostics/
│   ├── riven_formatter/
│   └── riven_driver/               # orchestrator currently in lib.rs
│
├── library/                        # everything user code sees
│   ├── runtime/                    # the C runtime, split per module
│   │   ├── runtime.h               # public surface (current top-of-file decls)
│   │   ├── core/
│   │   │   ├── alloc.c             # riven_alloc / riven_free / riven_panic
│   │   │   ├── vec.c               # RivenVec primitives
│   │   │   ├── string.c            # String ops
│   │   │   └── hash.c              # RivenHash (incl. T2's rehash work)
│   │   ├── io/
│   │   │   ├── stdio.c             # Stdin/Stdout/Stderr
│   │   │   ├── file.c              # RivenFile (from #06.5 T2)
│   │   │   ├── bufio.c             # BufReader / BufWriter (from #06.5 T6)
│   │   │   └── io_error.c          # IoError tagged enum (from #06.5 T1)
│   │   ├── fs.c
│   │   ├── net/
│   │   │   ├── tcp.c               # promoted from #06.5 T5
│   │   │   └── shutdown.c          # Shutdown enum dispatch
│   │   ├── time.c
│   │   ├── process.c               # RivenCommand / RivenOutput (from #06)
│   │   ├── fmt.c                   # RivenFormatter (from #06.D)
│   │   ├── env.c
│   │   ├── signal.c                # SIGINT handler
│   │   └── build.rs                # the per-module build script
│   │
│   └── std/                        # the .rvn-source side of the stdlib
│       └── src/
│           ├── prelude.rvn         # auto-imported every module
│           ├── iter.rvn            # already exists (runtime/std/iter.rvn)
│           ├── io.rvn              # future Read/Write mixin lives here (v1.5)
│           ├── fmt.rvn
│           └── collections.rvn
│
├── src/                            # tooling & drivers
│   ├── rivenc/                     # was crates/rivenc + crates/riven-cli
│   ├── riven_lsp/                  # was crates/riven-lsp
│   ├── riven_ide/                  # was crates/riven-ide
│   ├── riven_repl/                 # was crates/riven-repl
│   └── tools/                      # future: rivendoc, rivenfmt-standalone, etc.
│
├── tests/                          # top-level integration & e2e
│   ├── release-e2e/                # already exists at this path; unchanged
│   ├── ui/                         # future: ui-test-style diagnostic snapshots
│   └── stdlib/                     # was crates/riven-core/tests/stdlib_*.rs
│
├── docs/                           # unchanged
├── Cargo.toml                      # workspace root, members = compiler/* src/*
└── README.md
```

### Crate dependency invariants

- `compiler/*` may depend on each other in phase order:
  `lexer → parser → ast → hir → resolve → typeck → mir → borrowck → codegen_*`.
  No upward edges (codegen does not import resolve). The driver
  crate at the top imports each phase and orchestrates.
- `compiler/*` may **not** depend on `library/*`. The compiler is
  pure Rust; the runtime is C and is linked into the user binary,
  not into the compiler.
- `library/runtime/*` is built by `library/runtime/build.rs` into a
  single static lib (`libriven_runtime.a`) that the codegen crates
  emit a link directive for. Splitting the C source files does **not**
  mean splitting the link product — every .c file ends in one
  archive consumed by every Riven binary.
- `library/std/*.rvn` is read at compile-time by `riven_resolve` via
  the implicit-includes pipeline.
- `src/rivenc` depends on `compiler/riven_driver` plus the codegen
  backend it chooses at runtime.
- `tests/` is workspace-root and depends on `compiler/riven_driver`
  + `src/rivenc` only (so e2e cases compile via the public CLI).

## Migration plan, in phases that each leave `cargo test --workspace` green

This is the load-bearing constraint: **every commit on the
restructure branch must pass `cargo test --workspace`** so we never
have a "rebase the entire tree" moment. Each phase below is one
commit (or one tight cluster of commits) that ships a verifiable
slice.

### Phase A — workspace skeleton (one commit, zero behavior change)

1. Create the empty target dirs: `compiler/`, `library/runtime/`,
   `library/std/`, `src/`, `tests/ui/`, `tests/stdlib/`.
2. Add an aggregate `Cargo.toml` at the workspace root listing the
   future member globs but pointing at the *current* crate paths
   via `[workspace.metadata.future]` or commented stubs. No crate
   moves yet.
3. Add `docs/architecture/repo-layout.md` describing the layout.
4. CI: `cargo test --workspace` runs as before.

### Phase B — split the runtime C file (one commit)

`crates/riven-core/runtime/runtime.c` (5 285 LOC) → per-module files
under `library/runtime/`. Steps:

1. Move `runtime.c` whole-file to `library/runtime/runtime.c` first
   (no content change) — `git mv` so blame is preserved.
2. Carve into per-module files by section, splitting at the existing
   `/* ── … ─── */` banner comments. Each per-module file is `#include`d
   from a thin top-level `library/runtime/runtime.c` so the build
   product is identical to today. Order of carve-out, in commit
   sequence:
   - `core/alloc.c` (riven_alloc, riven_free, riven_panic)
   - `core/vec.c`
   - `core/string.c`
   - `core/hash.c`            ← #06.5 T2's rehash work + Map/Set
   - `io/io_error.c`          ← #06.5 T1's tagged enum
   - `io/stdio.c`
   - `io/file.c`              ← #06.5 T2
   - `io/bufio.c`             ← #06.5 T6
   - `fs.c`
   - `net/tcp.c`              ← #06.5 T5
   - `time.c`                 ← #06.5 T4
   - `process.c`
   - `fmt.c`
   - `env.c`
   - `signal.c`
3. The top-level `runtime.c` becomes literally:
   ```c
   #include "runtime.h"
   #include "core/alloc.c"
   #include "core/vec.c"
   /* … all the rest … */
   ```
   This preserves the single-translation-unit build (so static
   functions across files still see each other) **without** giving
   up the file-level locality. A follow-up prompt (#06.76?) can
   promote selected helpers to extern + per-file objects once we
   know we need it; v1 stays in unity-build mode.
4. `library/runtime/runtime.h` collects every public C symbol decl
   currently at the top of `runtime.c`.
5. Update the build script (currently
   `crates/riven-core/runtime/build.rs` or wherever it lives) to
   read from `library/runtime/runtime.c`.

**Validation:** `cargo test --workspace` ≡ pre-Phase-B output, byte
for byte. If any test changes its result this phase has a bug.

### Phase C — split `resolve/mod.rs` (6 943 LOC) into per-namespace files

`crates/riven-core/src/resolve/mod.rs` is the second worst offender.
It contains: the scope table, import resolution, **and** every
builtin stdlib registration (Option / Result / Array / HashMap /
String / IoError / File / Command / Iterator / …).

1. Carve the stdlib registrations out into
   `crates/riven-core/src/resolve/stdlib/{primitives,collections,
   option,result,iter,io,fs,net,time,process,env,fmt,thread}.rs`,
   each exposing one `pub fn register(scopes: &mut Scopes, …)`.
2. `resolve/mod.rs` shrinks to ~1 000 LOC (just scope + import
   logic) and a single `stdlib::register_all(...)` call.
3. Tag-stability invariant from `io_error_tag_stability.rs` MUST
   continue to hold — append-only variant order across the move.
4. CI: workspace green.

### Phase D — split `typeck/infer.rs` (2 895 LOC) into per-namespace method resolvers

Same shape as Phase C. The big `match (recv_ty, method_name)`
becomes a registry: each stdlib module contributes a
`pub fn resolvers() -> Vec<MethodResolver>` and `infer.rs` just
dispatches.

### Phase E — split `codegen/runtime.rs` (977 LOC) into per-namespace tables

Smaller version of Phase D. The `match name { "File_open" => "riven_file_open", … }`
becomes one file per namespace.

### Phase F — promote `crates/riven-core` to a compiler workspace

This is the structural payoff phase, and the riskiest. Once C, D, E
are done the boundaries are visible; now we extract them into
sibling crates:

1. Move `crates/riven-core/src/lexer/` → `compiler/riven_lexer/`.
2. Move `crates/riven-core/src/parser/` → `compiler/riven_parser/`,
   updating its `Cargo.toml` to depend on `riven_lexer` + a new
   `riven_ast` crate that holds the AST types.
3. Repeat for `hir`, `resolve`, `typeck`, `mir`, `borrow_check`,
   `diagnostics`, `formatter`.
4. Split `codegen/` into three crates: `riven_codegen_shared` (the
   runtime-name table + IR-emit helpers shared between backends),
   `riven_codegen_cranelift`, `riven_codegen_llvm`. Each backend
   crate depends on `riven_codegen_shared` + `riven_mir`.
5. `crates/riven-core/src/lib.rs` becomes `compiler/riven_driver/src/lib.rs`,
   re-exporting from the phase crates so existing
   `riven_core::resolve::*` import sites in tests/CLI continue to
   resolve via a compat shim. **Compat shim is temporary**: delete
   it in Phase H.
6. Update the workspace `Cargo.toml`: members =
   `["compiler/*", "src/*", "tests/*"]`.

Each crate extraction is one commit. Per-phase rebuild times should
drop visibly even mid-migration.

### Phase G — move drivers and tooling under `src/`

1. `crates/rivenc` → `src/rivenc/` (the binary stays the same name).
2. `crates/riven-cli` → fold into `src/rivenc/` if it's the same
   driver, or `src/riven_cli/` if distinct.
3. `crates/riven-lsp` → `src/riven_lsp/`.
4. `crates/riven-ide` → `src/riven_ide/`.
5. `crates/riven-repl` → `src/riven_repl/`.

### Phase H — move integration tests up and out

1. `crates/riven-core/tests/stdlib_*.rs` → `tests/stdlib/`. These
   tests already build through the public driver crate; the move
   is mechanical.
2. `crates/riven-core/tests/fixtures/riven/*.rvn` → `tests/fixtures/riven/`.
3. `tests/release-e2e/` stays put (already at workspace root).
4. Delete the Phase F compat shim in `compiler/riven_driver`.
5. Update CI scripts and the `cargo test` invocations in
   `docs/STRATEGY.md` and CHANGELOG.

### Phase I — promote `runtime/std/iter.rvn` to `library/std/`

The lone `.rvn`-source stdlib file moves to its destination:
`crates/riven-core/runtime/std/iter.rvn` → `library/std/src/iter.rvn`.
Update the implicit-includes pipeline (`crates/riven-core/src/implicit_includes/mod.rs`,
now `compiler/riven_resolve/src/implicit_includes/mod.rs` post-F) to
read from the new path.

### Phase J — final cleanup, docs, CHANGELOG

1. Update `docs/STRATEGY.md` "Repo layout" section.
2. Update every `docs/specs/*.spec.md` reference that names a path
   under `crates/riven-core/...` (a tracked find/replace).
3. Update `README.md` build instructions if they cite paths.
4. CHANGELOG bullet under `## [Unreleased] ### Changed`: "Repo
   restructured to rust-lang-style `compiler/` + `library/` +
   `src/` + `tests/` tree."
5. `cargo test --workspace` green. Cache to
   `tmp/test-cache/p06_75-final.log`.

## Reserved error codes

None — this prompt is mechanical refactor only. No new diagnostics,
no new surface, no behavior change.

## TDD

The whole point of phasing is that every commit is its own pin test:
**`cargo test --workspace` must be byte-for-byte identical in test
result counts to its pre-restructure baseline.** Cache the baseline
to `tmp/test-cache/p06_75-baseline.log` as the very first action of
Phase A; diff each subsequent phase's cache against it.

No new behavioral tests are required for this prompt. If a test
*does* change result, that's a refactor bug — fix it (do not
update the test).

## Anti-goals

- **No new public API.** This prompt does not add classes, methods,
  free fns, or diagnostics. Anything that smells like an
  improvement is a separate follow-up prompt.
- **No `riven_unsafe_*` symbol renames in runtime.** The C-side
  symbol names stay byte-identical (the file they live in changes,
  the symbol does not). Codegen tables stay the same. The
  monomorphised binaries are bit-identical to pre-restructure.
- **No per-file static library split for the C runtime in v1.**
  Stay in unity-build mode (top-level `runtime.c` `#include`s the
  per-module files). The per-object-file build is a future prompt.
- **No `compiler/` ↔ `library/` dependency edges.** The compiler is
  Rust; the runtime is C; they meet only at the codegen-emitted
  `extern "C"` declarations and the linker.
- **No incidental work.** No "while I'm here" refactors of business
  logic, no formatting passes, no cargo-fmt runs that touch
  unrelated files. Pure mechanical moves + rewires.
- **No CI infrastructure changes** beyond `cargo test` invocation
  paths. GitHub Actions, release scripts, hooks: keep them
  pointing at the same workspace root. The restructure must be
  CI-transparent.

## Definition of done

- [x] Repo tree matches the "Target layout" §, with every old crate
      either renamed or merged into its destination.
- [x] No file remains under `crates/riven-core/`. The crate is
      either renamed (likely deleted, replaced by `compiler/*`) or
      its inhabitants are redistributed.
- [x] `runtime.c` is the unity-build aggregator under
      `library/runtime/`; every C symbol lives in a per-module
      file under `library/runtime/{core,io,net,…}/`.
- [x] `resolve/mod.rs` ≤ 1 200 LOC (scope/import only); stdlib
      registrations live under `compiler/riven_resolve/src/stdlib/`.
      *Audited: 649 LOC.*
- [ ] `typeck/infer.rs` ≤ 1 200 LOC; method resolvers live under
      `compiler/riven_typeck/src/method_resolvers/`. *Audited:
      2279 LOC — pending split.*
- [ ] `codegen/runtime.rs` ≤ 300 LOC; runtime-name tables live
      under `compiler/riven_codegen_shared/src/runtime_table/`.
      *Audited: 739 LOC — pending split.*
- [x] `cargo test --workspace` cached to
      `tmp/test-cache/p06_75-final.log` is byte-for-byte identical
      in pass/fail/ignored counts to the baseline cached on Phase A.
- [x] `docs/STRATEGY.md` "Repo layout" section updated.
- [x] CHANGELOG bullet under `## [Unreleased] ### Changed`.
- [x] `tests/release-e2e/` still passes via `cargo test -p rivenc
      --test p05_e2e_check -- --ignored` with identical PASS count.

## Why this comes before #07 (const generics)

#07 will add new stdlib classes (`Array[T, const N]`, possibly
`Vector[T, const N]`, etc.) and new const-arg lowering paths that
touch `resolve`, `typeck`, `mir`, and `codegen` simultaneously.
Landing #06.75 first means #07 makes its edits in
`compiler/riven_resolve/src/stdlib/collections.rs` (≤ 300 LOC) +
`compiler/riven_typeck/src/method_resolvers/array.rs` (≤ 200 LOC)
instead of squeezing into the 6 943-LOC `resolve/mod.rs` and
2 895-LOC `infer.rs` we have today.

Same argument applies more strongly to #10 (LSP) and #11
(incremental): both want phase-level rebuild boundaries that only
exist after Phase F here.

## Why this is one prompt, not ten

Each individual phase (A–J) is small and verifiable. But the
*sequence* is what's load-bearing — you can't usefully split
`resolve/mod.rs` (Phase C) until the runtime C split (Phase B) has
removed the easy "all logic in one place" excuse; you can't extract
phase crates (Phase F) until C, D, E have surfaced the boundaries;
you can't move tests up (Phase H) until F has stabilised the public
crate names. One prompt, one branch, ten well-ordered commits.

Estimated size: ~1–2 days of mechanical work for a single executor.
~3 000 lines of moves + a couple of hundred lines of build-script
and Cargo.toml edits. No new C code, no new Rust *logic*.
