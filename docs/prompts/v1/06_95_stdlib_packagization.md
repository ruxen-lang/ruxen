# 06.95 — stdlib packagization: per-module packages, class-only surface, compiler-as-loader

**Depends on:** #06.8 (Wave 2 surface migration of `.rvn` files into
`library/std/src/`), #06.9 (closure dispatch — so `Fn[…]_call` mangling
has a stable home before we delete `runtime_table`), **#06.93
(module-qualified class resolution — gating Phase C; see
`docs/prompts/v1/06_93_module_qualified_class_resolution.md`)**.
**Reads:**
`compiler/riven_core/src/resolve/stdlib/mod.rs` (1082 LOC of Rust-side
registrations that this prompt deletes),
`compiler/riven_core/src/codegen/runtime_table/mod.rs` (525 LOC of
mangled-name C-symbol dispatch — also deleted),
`compiler/riven_core/src/resolve/bootstrap.rs` (the loader that this
prompt generalises from a flat file list to a package discovery walk),
`library/std/src/*.rvn` (18 files, 847 LOC — this is the *starting*
content that gets re-sliced into packages),
`src/riven_cli/src/manifest.rs` + `deps.rs` + `resolve_deps.rs` (the
existing package machinery the sysroot loader will reuse),
`examples/01-cli-utility/Riven.toml` (reference shape for a v1
manifest).

**Status:** plan only — no code changes in this prompt. Phasing is
spelled out so each phase can land as its own commit with its own
narrow pin tests; the final phase is the only one that touches the
workspace-wide test surface.

---

## Why this prompt exists

Three product decisions from the 2026-05-19 stdlib review converge
into a single architectural reshape:

1. **The compiler must not declare the stdlib.** Today
   `resolve/stdlib/mod.rs` (1082 LOC) still owns: 3 builtin traits,
   3 free fns (`sleep`, `signal_install_sigint`,
   `signal_received_sigint`), the `Arc → SharedSync` backward-compat
   alias, the `ThreadId` value-scope shim, the `std.*` module
   skeleton, and the value-scope type-constructor variables for
   `Array.new`, `String.from`, etc. `codegen/runtime_table/mod.rs`
   (525 LOC, 195 `riven_*` C-symbol references) owns the mangled-name
   dispatch for every method whose `.rvn` declaration hasn't
   reached the FFI-alias-rewrite path yet. All of this is stdlib
   surface masquerading as compiler internals. **End state: the
   compiler defines primitive types and language intrinsics only;
   everything else is parsed from `.rvn`.**

2. **No free functions in the stdlib.** Today users write
   `println("hi")`, `read_to_string(path)`, `sleep(d)`, `exit(0)`,
   `unix_ns()`. The `.rvn` files declare these as bare `def` entries
   inside top-level `lib "riven_runtime"` blocks. The product
   decision is to require every stdlib surface to be a class method
   — static (`IO.println("hi")`, `FS.read_to_string(path)`,
   `Thread.sleep(d)`, `Process.exit(0)`, `Time.unix_ns()`) or
   instance (`f.read_to_string()`, `i.elapsed()`). This is a hard
   break of every existing user program that calls a free stdlib fn.

3. **One package per stdlib module.** Today `library/std/src/`
   is a flat directory of 18 `.rvn` files loaded by a hand-ordered
   `BOOTSTRAP_FILES` constant. The product decision is to give each
   module its own package (`library/std/io/`, `library/std/fs/`,
   `library/std/net/`, …) with its own `Riven.toml` declaring its
   own dependencies. The compiler discovers packages from the sysroot
   instead of consulting a hard-coded list. This matches Rust's
   `library/std/`, `library/core/`, `library/alloc/` split and
   unblocks the long-term "ship a stdlib patch as a package" goal
   from #06.8 (architectural smell #6).

Together these three changes deliver the end state every other
production language has reached: **the compiler is in Rust, the
stdlib is in Riven (every module shipped as its own package), the
runtime layer is the narrow waist between them.**

---

## What today actually looks like

### Free fns still present (the surface that becomes class-based)

Inventoried from `library/std/src/*.rvn` `lib "riven_runtime"` blocks:

| Module       | Free fns                                                                                                                                                          | Count |
| ------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| `io.rvn`     | `puts`, `eputs`, `print`, `println`, `eprintln`, `read_line`, `stdin`, `stdout`, `stderr`                                                                         | 9     |
| `fs.rvn`     | `read_to_string`, `write`, `exists`, `is_file`, `is_dir`, `read_dir`, `metadata`, `remove_file`, `create_dir`, `create_dir_all`, `rename`, `copy`, `remove_dir_all`, `canonicalize`, `write_atomic`, `read_link`, `symlink` | 17    |
| `env.rvn`    | `args`, `get`, `vars`, `current_dir`                                                                                                                              | 4     |
| `path.rvn`   | `path_join`, `path_parent`, `path_file_name`, `path_extension`, `path_is_absolute`                                                                                | 5     |
| `rand.rvn`   | `random_bytes`, `random_u64`, `random_fill`                                                                                                                       | 3     |
| `process.rvn`| `exit`                                                                                                                                                            | 1     |
| `time.rvn`   | `unix_ns`                                                                                                                                                         | 1     |
| `_bootstrap_smoke.rvn` | one smoke symbol                                                                                                                                        | 1     |

Plus the three free fns still in `resolve/stdlib/mod.rs`:
`sleep(&Duration) -> ()`, `signal_install_sigint() -> ()`,
`signal_received_sigint() -> Int`.

**Total: 44 free fns that must become class statics.**

### Class shells already present (just need methods + FFI block consolidated)

`Duration`, `Instant` (`time.rvn`); `File`, `OpenOptions`, `BufReader[R]`,
`BufWriter[W]`, `Stdin`, `Stdout`, `Stderr`, `SeekFrom`, `IoError`,
`IoErrorKind` (`io.rvn`); `Metadata` (`fs.rvn`); `Command`, `Output`,
`ExitStatus` (`process.rvn`); `TcpListener`, `TcpStream`, `Shutdown`
(`net.rvn`); `Formatter`, `FmtError` (`fmt.rvn`); 9 sync class shells
(`sync.rvn`); collection anchors `Array`, `Map`, `Set`, `String`
(distributed across 4 files); `Option`/`Result` anchors
(`option_result.rvn`).

### Still in Rust (`resolve/stdlib/mod.rs`)

After stripping migration-marker comments, the live registrations are:

- Primitive types (`Int`, `String`, …) — **stay in Rust forever;
  they're the type system.**
- 3 builtin traits: `Displayable`, `Error`, `Comparable` — **move
  to `library/std/core/`.**
- 3 free fns above — **become `Thread.sleep`, `Signal.install_sigint`,
  `Signal.received_sigint`.**
- `Arc → SharedSync` alias class — **move to `library/std/sync/`.**
- `ThreadId` value-scope shim — **move to `library/std/sync/`.**
- `std.*` module skeleton — **derived from package discovery;
  delete.**
- Type-constructor variables in value scope (`Array.new`,
  `String.from`, …) — **derived from each package's `class`
  declarations; delete.**

### The runtime_table dispatch (525 LOC)

195 `riven_*` C-symbol references doing four jobs:

1. **Plain alias dispatch** for methods whose `.rvn` declaration
   hasn't reached the FFI-alias-rewrite path. Moves directly to a
   `lib "riven_runtime" def foo as "riven_foo"(...)` block in the
   appropriate package's `lib.rvn`. ~140 of the 195 are this shape.
2. **Generic stripping**: `Vec[Int]_push` → `Array_push`. Mechanism
   already exists (T#14/T#17 fallback in
   `mir/lower/expr/method_call.rs`). Pre-condition for moving the
   collection methods.
3. **Static-ctor fast paths**: `Duration.from_secs(5)` /
   `Instant.now()` skip the synthetic `self` prepend. Needs a
   declarative marker in `.rvn` (proposal: parser already accepts
   `def self.foo` for class-method shape; codegen needs to honour it
   end-to-end).
4. **Inner-type discrimination**: `BufReader.new(file)` →
   `riven_bufreader_new_file`; `BufReader.new(tcp)` →
   `riven_bufreader_new_tcp`. Currently a hand-coded peek at the
   argument type in `mir/lower/expr/method_call.rs:180-200`.
   **Resolved by Decision #3** — module + mixin reshape names each
   inner class explicitly (`BufReader.FromFile.new(f)` /
   `BufReader.FromTcp.new(s)`), so the discriminator becomes the
   class name itself and the MIR sniff is deleted. See
   §"Module + mixin pattern" below.

Plus 4 dispatch arms that aren't really "stdlib" — `Fn(…)_call`,
`Fn[…]_call`, `VecIter`, `VecIntoIter`, `SplitIter` — these are
language intrinsics (closure calling, iterator desugaring). **They
stay in the compiler under a renamed module
(`codegen/lang_intrinsics.rs`) so the deletion of `runtime_table`
doesn't conflate "stdlib FFI" with "closure ABI lowering".**

### Ergonomics of "no free fns"

Hard-breaking `println("hi")` is the riskiest cosmetic decision in
this plan. Three concrete options for the user to choose between
*before* Phase 2 lands:

- **(a) Hard break + codemod.** Every user call becomes
  `IO.println("hi")`. Ship a `riven fix --rule=stdlib-class-prefix`
  pass that rewrites the existing example programs (`examples/*`)
  and any user code on contact. **Closest to user's stated intent.**
- **(b) Keep a 6-name global core.** Make `println`, `print`,
  `eprintln`, `panic`, `assert`, `dbg` the only true free functions
  in the language (declared in a `library/std/prelude/` package that
  re-exports `IO.println` etc.). Everything else class-prefixed.
  Matches Rust's `print!` / `panic!` macro core.
- **(c) Auto-imported namespace.** Add a "wildcard prelude import"
  mechanism (`use std.io.IO.*`) that makes `println` resolve to
  `IO.println` without textual qualification. Adds a language
  feature; defers the surface decision.

This plan assumes **(a)** for the work split below. If the user
ratifies (b) or (c) at the scope-confirmation step, Phase 2 grows
by the prelude design or shrinks to "skip the codemod for
println/print".

---

## End-state architecture

Each stdlib package is **self-contained** — the Riven surface
(`src/lib.rvn`) and the C runtime implementation (`runtime/*.c`)
live in the same directory and ship together. The compiler reads
the package's `Riven.toml` for both the Riven dependency graph
and the per-package C link unit. Today the C runtime is split
across `library/runtime/{io,net,core,*.c}` separately from
`library/std/src/*.rvn`; the migration moves each `.c` file into
its owning package's `runtime/` subdirectory.

Mapping of today's `library/runtime/*.c` → new package home:

| Current path                     | Lives in package          |
| -------------------------------- | ------------------------- |
| `library/runtime/runtime.c`      | `std-core/runtime/`       |
| `library/runtime/core/alloc.c`   | `std-core/runtime/`       |
| `library/runtime/core/hash.c`    | `std-hash/runtime/`       |
| `library/runtime/core/string.c`  | `std-string/runtime/`     |
| `library/runtime/core/vec.c`     | `std-array/runtime/`      |
| `library/runtime/io/bufio.c`     | `std-io/runtime/`         |
| `library/runtime/io/file.c`      | `std-io/runtime/`         |
| `library/runtime/io/io_error.c`  | `std-io/runtime/`         |
| `library/runtime/io/rand.c`      | `std-rand/runtime/`       |
| `library/runtime/io/stdio.c`     | `std-io/runtime/`         |
| `library/runtime/net/tcp.c`      | `std-net/runtime/`        |
| `library/runtime/process.c`      | `std-process/runtime/`    |
| `library/runtime/fs.c`           | `std-fs/runtime/`         |
| `library/runtime/env.c`          | `std-env/runtime/`        |
| `library/runtime/time.c`         | `std-time/runtime/`       |
| `library/runtime/signal.c`       | `std-sync/runtime/`       |
| `library/runtime/test_extern.c`  | `std-core/runtime/`       |
| `library/runtime/fmt.c`          | `std-fmt/runtime/`        |

`std-map` and `std-set` may end up sharing `std-hash/runtime/`'s
helpers depending on how the runtime is currently split; resolve
at Phase B time. `library/runtime/` ceases to exist after Phase B
— the directory is fully partitioned across the 19 packages.

`Riven.toml` per package declares a `[runtime]` section listing
its C sources so the compiler driver can emit them as a per-package
link unit:

```toml
name = "std-io"
version = "0.1.0"

[dependencies]
std-core = "= 0.1.0"

[runtime]
# Compiler reads this when building any binary that depends on
# std-io. Each `.c` is compiled once per build and linked into
# the final executable. The order follows source-tree order.
sources = ["runtime/bufio.c", "runtime/file.c", "runtime/io_error.c", "runtime/stdio.c"]
# Optional cc flags scoped to this package's compilation unit.
cflags = ["-Wall", "-Wno-unused-parameter"]
```

```
library/
  std/
    core/              # primitive trait declarations + low-level C runtime
      Riven.toml       # name="std-core", no deps,
                       # [runtime] sources=["runtime/alloc.c", "runtime/hash.c",
                       #   "runtime/runtime.c", "runtime/test_extern.c"]
      src/lib.rvn      # Displayable, Error, Comparable, Hashable*
      runtime/         # alloc.c, hash.c, runtime.c, test_extern.c
    io/
      Riven.toml       # name="std-io", deps={std-core="0.1"},
                       # [runtime] sources=["runtime/bufio.c", "runtime/file.c",
                       #   "runtime/io_error.c", "runtime/stdio.c"]
      src/lib.rvn      # class IO + Stdin/Stdout/Stderr/File/OpenOptions/
                       #   BufReader/BufWriter/SeekFrom/IoError/IoErrorKind
                       # + `lib "riven_runtime"` block inside each class
      runtime/         # bufio.c, file.c, io_error.c, stdio.c
    fs/
      Riven.toml       # deps={std-io="0.1", std-core="0.1"},
                       # [runtime] sources=["runtime/fs.c"]
      src/lib.rvn      # class FS + Metadata
      runtime/         # fs.c
    env/
      Riven.toml       # deps={std-io="0.1"}
      src/lib.rvn      # class Env
    path/
      Riven.toml       # no deps
      src/lib.rvn      # class Path
    process/
      Riven.toml       # deps={std-io="0.1"}
      src/lib.rvn      # class Process + Command + Output + ExitStatus
    time/
      Riven.toml       # no deps (Duration/Instant are leaves)
      src/lib.rvn      # class Time + Duration + Instant
    net/
      Riven.toml       # deps={std-io="0.1"}
      src/lib.rvn      # class Net + TcpListener + TcpStream + Shutdown
    sync/
      Riven.toml       # deps={std-time="0.1", std-core="0.1"}
      src/lib.rvn      # class Thread + Signal + Mutex + MutexGuard +
                       #   SharedSync (canonical) + Arc (alias) + ThreadId +
                       #   Context + Waker + JoinHandle + PoisonError +
                       #   ThreadPanic
    rand/
      Riven.toml       # deps={std-io="0.1"}
      src/lib.rvn      # class Rand
    fmt/
      Riven.toml       # deps={std-core="0.1"}
      src/lib.rvn      # Display + Debug traits + Formatter + FmtError
    hash/
      Riven.toml       # deps={std-core="0.1"}
      src/lib.rvn      # Hashable trait
    iter/
      Riven.toml       # deps={std-core="0.1"}
      src/lib.rvn      # Iterator + FromIterator mixins
    array/
      Riven.toml       # deps={std-iter="0.1"}
      src/lib.rvn      # class Array (Vec alias kept)
    map/
      Riven.toml       # deps={std-hash="0.1"}
      src/lib.rvn      # class Map (HashMap alias)
    set/
      Riven.toml       # deps={std-hash="0.1"}
      src/lib.rvn      # class Set (HashSet alias)
    string/
      Riven.toml       # deps={std-core="0.1"}
      src/lib.rvn      # class String method anchor
    option_result/
      Riven.toml       # deps={std-core="0.1"}
      src/lib.rvn      # Option / Result method anchors

compiler/riven_core/src/
  resolve/
    bootstrap.rs       # 1082-LOC stdlib/mod.rs replaced by ~80 LOC of
                       # sysroot-walk + package-load + topo-sort loader
    stdlib/            # DELETED — primitives move to resolve/builtins.rs
    builtins.rs        # NEW ~150 LOC — primitive types + intrinsics only
  codegen/
    runtime_table/     # DELETED
    lang_intrinsics.rs # NEW ~80 LOC — Fn[..]_call, VecIter, SplitIter,
                       # everything that isn't FFI dispatch
```

Loader sequence per `bootstrap.rs`:

1. Resolve sysroot (`$RIVEN_STDLIB_PATH`, workspace `library/std/`,
   `<exe>/../library/std/`).
2. Walk `library/std/*/Riven.toml` — every direct subdirectory with a
   manifest is a stdlib package.
3. Topologically sort by `[dependencies]` (reuse
   `src/riven_cli/src/resolve_deps.rs`). Cycle = E07XX
   diagnostic.
4. For each package in topo order: parse its `src/lib.rvn` (recurse
   into `src/*.rvn` if the manifest declares them — supports
   multi-file packages later), register exports in scope, advance to
   the next package.
5. Build the `std` module namespace from package names — no Rust-side
   hand-assembly.

Net delta:

| File                                         | Before | After | Δ      |
| -------------------------------------------- | ------ | ----- | ------ |
| `compiler/riven_core/src/resolve/stdlib/`    | 1082   | 0     | −1082  |
| `compiler/riven_core/src/resolve/builtins.rs`| 0      | ~150  | +150   |
| `compiler/riven_core/src/resolve/bootstrap.rs` | ~250 | ~330  | +80    |
| `compiler/riven_core/src/codegen/runtime_table/` | 525 | 0    | −525   |
| `compiler/riven_core/src/codegen/lang_intrinsics.rs` | 0 | ~80 | +80    |
| `library/std/*/Riven.toml` (19 packages)     | 0      | ~190  | +190   |
| `library/std/*/src/lib.rvn` (19 packages)    | 847    | ~2400 | +1553  |
| **Net Rust deletion**                        |        |       | **−1297** |

The +1553 in Riven is because the .rvn files absorb the entire FFI
alias surface that today lives in `runtime_table` arms.

---

## Phasing

Each phase is one or more commits; each lands behind narrow pin tests
(per Rule 41/42); the workspace test runs *only* in the final
integration task per phase.

### Phase A — Package discovery loader (no surface change)

**Goal:** generalise `bootstrap.rs` from "ordered flat list of files
under `library/std/src/`" to "topologically sorted packages discovered
under `library/std/`", *without* moving any content yet. The current
`library/std/src/` becomes `library/std/_legacy/src/` with a wrapper
`library/std/_legacy/Riven.toml` so the loader treats it as a single
package during the transition. All existing pin tests pass unchanged.

Tasks:
1. Add `parse_sysroot_packages(sysroot: &Path) -> Vec<PackageInfo>`
   in `bootstrap.rs`. Reuses `src/riven_cli/src/manifest.rs` parser.
2. Add topological sort. Cycle detection emits new diagnostic E07XX
   ("stdlib package cycle") with the offending edge.
3. Add `walk_package(pkg: &PackageInfo) -> Vec<Program>` that parses
   `src/lib.rvn` (later: `src/*.rvn` if manifest declares).
4. Replace the `BOOTSTRAP_FILES` const + flat-list loop with the
   package walk. Keep the legacy `library/std/_legacy/` package as
   the only entry until Phase B starts.
5. Move existing `.rvn` files: `library/std/src/*.rvn` →
   `library/std/_legacy/src/*.rvn`. Add `library/std/_legacy/Riven.toml`.
6. Pin test: new test under
   `compiler/riven_core/tests/stdlib_package_loader.rs` exercising
   topo-sort, cycle detection, and "loader finds N packages under a
   tempdir" via `RIVEN_STDLIB_PATH`.

Diff size: ~400 LOC compiler + manifest. No content moves except the
single directory rename. Verification: existing
`io_error_tag_stability` + `file_class_layout_stability` +
`shutdown_tag_stability` + `seek_from_tag_stability` pin tests all
pass; the new loader test passes; **only those tests run**, not the
workspace.

### Phase B — Split `_legacy` into 19 packages + colocate C runtime

**Goal:** rehome each `.rvn` file under a real package directory,
declare cross-package deps in `Riven.toml`, **move each C runtime
file from `library/runtime/` into its owning package's
`runtime/` subdirectory**, but DO NOT yet change free-fn →
class-method shape. This isolates the directory churn from the
surface churn.

The C-runtime co-location is the user-ratified end-state shape
(2026-05-19): each stdlib package is self-contained — Riven
surface + C implementation ship together. See the "End-state
architecture" section above for the file-by-file mapping from
today's `library/runtime/{io,net,core,*.c}` to the new
`library/std/<pkg>/runtime/*.c` homes.

The compiler driver needs a small extension to read each
package's `[runtime] sources = [...]` list and compile/link the
listed `.c` files into the final binary; see
`src/riven_cli/src/build.rs`. Phase A's loader laid the
single-`Riven.toml` foundation; Phase B teaches the toml parser
+ build driver to honour `[runtime]`.

Tasks (one commit per package or grouped by dep layer):
1. **Layer 0 (no deps):** `core`, `path`, `time`, `iter`, `hash`.
2. **Layer 1 (depend on core):** `fmt`, `string`, `option_result`,
   `array`, `map`, `set`.
3. **Layer 2 (depend on core + io):** `io` itself first, then `fs`,
   `env`, `net`, `process`, `rand`.
4. **Layer 3:** `sync` (depends on time + core).

Each move:
- Create `library/std/<name>/Riven.toml` with `name`,
  `version="0.1.0"`, `[dependencies]`, and `[runtime] sources = [...]`.
- Move `library/std/_legacy/src/<name>.rvn` → `library/std/<name>/src/lib.rvn`.
- `git mv` each owning `library/runtime/*.c` file into
  `library/std/<name>/runtime/` per the mapping table above.
- Update any cross-file forward references that crossed file boundaries
  in the legacy flat layout (rare — `io.rvn`'s `IoError` is the only
  cross-package reference today).
- Per-package pin test: `tests/stdlib_pkg_<name>_loads.rs` parses the
  package in isolation under a fresh `RIVEN_STDLIB_PATH` tempdir and
  asserts the expected exports are in scope AND the `[runtime]`
  sources resolve to actual files on disk.
- Update `src/riven_cli/src/build.rs` to scan each loaded package's
  `[runtime]` and emit its `.c` files as a compile unit before
  linking the final binary.

After all 19 packages exist, delete `library/std/_legacy/` AND
`library/runtime/`. The package directories own everything.

Diff size: directory moves + 19 small TOMLs + 19 small pin tests.
Verification: each per-package pin test plus the workspace test runs
ONCE at the end of Phase B as the integration task.

### Phase C — Class-ify the 44 free fns

**Goal:** every free `def` in `.rvn` becomes a class static method.
This is the breaking-change phase.

Pre-work — user decision:
- Ratify ergonomics option (a) / (b) / (c) from "Ergonomics of no
  free fns" above. The rest of this phase assumes (a) hard break.

Tasks per package (one commit each):
1. `io`: wrap the 9 free fns in `class IO`. Static methods:
   `IO.puts`, `IO.eputs`, `IO.print`, `IO.println`, `IO.eprintln`,
   `IO.read_line`, `IO.stdin`, `IO.stdout`, `IO.stderr`. C-symbol
   aliases unchanged.
2. `fs`: `class FS` with 17 static methods. `FS.read_to_string`,
   `FS.write`, …
3. `env`: `class Env` with 4 static methods.
4. `path`: `class Path` with 5 static methods. Renames at the surface:
   `path_join` → `Path.join` (drop the `path_` prefix; the class
   prefix subsumes it). C-symbol aliases unchanged.
5. `rand`: `class Rand` with 3 static methods.
6. `process`: fold `exit` into `class Process.exit`.
7. `time`: fold `unix_ns` into `class Time.unix_ns` (a new namespace
   class — Duration and Instant stay separate).
8. Rust-side cleanup (this lands with whichever package absorbs
   each fn): `sleep` → `Thread.sleep`, `signal_install_sigint` →
   `Signal.install_sigint`, `signal_received_sigint` →
   `Signal.received_sigint`. All three move into `library/std/sync/`.
9. Codemod: `riven fix --rule=stdlib-class-prefix` in `src/riven_cli/`.
   Rewrites bare calls in user `.rvn` files. Initial coverage: the
   exhaustive rename table above. Applied to `examples/*` as part of
   the same commit so they keep building.
10. Per-package pin test: assert that the post-migration surface
    resolves (`IO.println("x")` typechecks; `println("x")` errors
    with a new diagnostic E07XX suggesting the class prefix).

Verification: per-package narrow runs in iteration; workspace run +
e2e smoke (`cargo test --test release_e2e_smoke -- --ignored`) once
at end of Phase C.

### Phase D — Move declarations out of Rust into packages

**Goal:** delete `compiler/riven_core/src/resolve/stdlib/mod.rs`
entirely. The 3 traits, the `Arc` alias, the `ThreadId` shim, and
every type-constructor `Variable` move into the appropriate package's
`lib.rvn`.

Tasks:
1. Carve `resolve/stdlib/mod.rs` into a thin
   `resolve/builtins.rs` containing only the primitive type table.
2. Move `Displayable`, `Error`, `Comparable` trait declarations into
   `library/std/core/src/lib.rvn` as `trait Foo` items. The
   bootstrap merge already handles trait registration symmetrically
   with user code (a separate `register_trait` pin test covers this).
3. Move the `Arc → SharedSync` alias into `library/std/sync/src/lib.rvn`
   as an explicit `class Arc[T]` shell that delegates. The pin test
   `arc_is_sharedsync_alias.rs` verifies the type identity.
4. Move the `ThreadId` value-scope shim. This is a one-line
   `DefKind::Variable` registration that needs a Riven equivalent —
   either drop it (`ThreadId` is no longer addressable as a value) or
   add a `const THREAD_ID: ThreadId = ...` declaration.
5. Delete the `std.*` module skeleton registration —
   `bootstrap.rs::assemble_std_module_from_packages` builds it from
   package names. Pin test: `std_io_use_resolves.rs`,
   `std_fs_use_resolves.rs`, etc.
6. Delete the type-constructor `Variable` table.
   `Array.new` / `String.from` / etc. resolve through normal
   class-static-method lookup once each package declares them.

Verification: workspace + e2e smoke at end of Phase D. This is the
phase most likely to surface unexpected dependencies on the
hand-registered shapes; budget for a few iterations.

### Phase E — Kill `runtime_table`

**Goal:** every method's C-symbol mapping comes from a `.rvn` `lib`
block. `codegen/runtime_table/mod.rs` deleted.

Tasks:
1. Audit the 195 `riven_*` references. Categorise each as:
   - **Plain alias** (~140): translates 1:1 to a `def foo as "riven_foo"`
     entry in the appropriate `class … lib "riven_runtime"` block. Move.
   - **Generic-stripping** (~30, mostly Vec/Array/Map/Set): already
     covered by the T#14/T#17 fallback in MIR. Move to the .rvn lib
     block; delete the runtime_table arm.
   - **Static-ctor fast path** (~15, mostly Duration/Instant): needs
     codegen to honour `def self.foo as "riven_foo"` consistently. Spec
     out `static_ctor_alias_resolution.spec.md` and pin test.
   - **Inner-type discriminator** (~6, BufReader/BufWriter):
     **resolved by the module + mixin reshape (Decision #3).** The
     20 `BufReader_*` / `BufWriter_*` entries become straight
     plain-alias entries inside the inner classes
     (`BufReader.FromFile`, `BufReader.FromTcp`, etc.) — each
     constructor names its C symbol directly. The inner-type sniff
     in `mir/lower/expr/method_call.rs:180-200` is deleted in the
     same commit; the typeck E0714 check that rejected non-{File,
     TcpStream} inner types is repurposed to reject mismatched
     `inner` args at each named constructor.
   - **Language intrinsic** (~4): `Fn[..]_call`, `VecIter`,
     `VecIntoIter`, `SplitIter`. Move to
     `codegen/lang_intrinsics.rs` — these are NOT stdlib FFI, they're
     closure-ABI / iterator-desugaring lowering.
2. Per category, one commit. Each one shrinks `runtime_table/mod.rs`
   by the moved arms. Final commit deletes the file.
3. Pin tests per category. The cross-cutting smoke that catches
   anything missed is the existing e2e suite — it'll catch a regressed
   `riven_*` link error immediately.

Verification: workspace + full e2e smoke (`RIVEN_E2E_CASES` unset, all
cases) at end of Phase E. This is the only phase that needs the full
e2e run because the symbol-lookup change touches every stdlib call
site.

### Phase F (deferred, separate prompt) — method bodies in Riven

Out of scope for #06.95 — covered in a successor prompt. Per the
earlier conversation:

- **Permanent FFI** (syscall wall): `io` (stdin/stdout/stderr backends),
  `fs`, `net`, `process`, `env`, `time` (clock_gettime), `sync`
  (pthread_*), `rand` (getrandom).
- **Could-become-Riven** (logic, not syscalls): `array`, `map`, `set`,
  `string`, `option_result`, `iter`, `fmt`, `hash`. Each requires
  language primitives that don't yet exist (raw pointers, stable
  unsafe, allocator FFI). Phase F gates on those primitives.

---

## Ratified decisions

Captured here so the next implementer doesn't have to re-litigate:

1. **Free-fn fate: hard break + codemod.** Every stdlib call becomes
   class-prefixed (`IO.println`, `FS.read_to_string`, `Thread.sleep`,
   `Process.exit`, `Time.unix_ns`, …). Compiler emits a new
   diagnostic E07XX on bare `println(...)` etc. with a class-prefix
   suggestion. `riven fix --rule=stdlib-class-prefix` codemod ships
   with Phase C and rewrites `examples/*` in the same commit.
2. **`Path` class-vs-type coexistence.** Riven's type-vs-value scope
   separation already handles `String`/`Array` shape; `Path` follows
   suit. Pin test `path_class_and_type_coexist.rs`.
3. **BufReader / BufWriter → module + mixin (see §"Module + mixin
   pattern" below).** Drops the `[R]` generic. Inner-type sniff in
   MIR is deleted. Each inner class declares its own constructor and
   `into_inner`; shared instance methods live in a mixin.
4. **Apply module + mixin pattern broadly** to every tagged variant
   family in the stdlib — not just BufReader/BufWriter. Initial
   scope: `Shutdown`, `SeekFrom`, `IoError`, `Result`, `Option`,
   plus any other tag-discriminated runtime spine surfaced during
   the audit. See §"Module + mixin pattern" below.
5. **Package versioning: lockstep at `0.1.0`.** Every stdlib package
   versions at exactly `0.1.0` for v1. Deps use exact-version
   (`= "0.1.0"`). Atomic stdlib updates.
6. **Drop the deprecated `Hash` alias.** `library/std/hash/src/lib.rvn`
   declares `mixin Hashable` only — no `Hash` re-export. The migration
   IS the deprecation window. Compiler emits E07XX on `include Hash`
   with rename suggestion. Codemod from Phase C extends to cover
   this rename in the same pass.
7. **`_bootstrap_smoke.rvn` → test fixtures.** Move to
   `compiler/riven_core/tests/fixtures/riven/bootstrap_smoke.rvn`.
   Production sysroot ships with zero smoke files. The bootstrap
   loader gets coverage from the Phase A pin test
   (`stdlib_package_loader.rs`) which builds a fake sysroot with the
   smoke fixture and asserts the symbol resolves end-to-end.

## No open questions remain.

Every design decision required to start Phase A is captured above.
Pre-flight Check 1 (mixin lib-decl wire-through) is the only
remaining unknown, and it's a one-task spike — not a decision —
that runs before Phase A.

---

## Module + mixin pattern

Adopted as the canonical shape for every stdlib class whose runtime
spine carries a discriminator tag (Decision #4). The pattern:

```riven
module BufReader
  mixin Reader
    lib "riven_runtime"
      def read_line as "riven_bufreader_read_line"(self) -> Result[Option[String], IoError]
      def read     as "riven_bufreader_read"(self, buf: &var Array[Int]) -> Result[Int, IoError]
    end
  end

  class File
    include Reader
    lib "riven_runtime"
      def self.new           as "riven_bufreader_new_file"(inner: ::File) -> BufReader.File
      def self.with_capacity as "riven_bufreader_with_capacity_file"(cap: Int, inner: ::File) -> BufReader.File
      def into_inner         as "riven_bufreader_into_inner_file"(self) -> ::File
    end
  end

  class Tcp
    include Reader
    lib "riven_runtime"
      def self.new           as "riven_bufreader_new_tcp"(inner: TcpStream) -> BufReader.Tcp
      def self.with_capacity as "riven_bufreader_with_capacity_tcp"(cap: Int, inner: TcpStream) -> BufReader.Tcp
      def into_inner         as "riven_bufreader_into_inner_tcp"(self) -> TcpStream
    end
  end
end
```

ABI contract: `BufReader.File` and `BufReader.Tcp` MUST have
byte-compatible spine layouts (same 32-byte struct with `kind=0` or
`kind=1`). Mixin-included instance methods dispatch through the
shared C symbol; the runtime branches on `spine->kind`. Pin test:
extends the existing `file_class_layout_stability` pattern with
a `bufio_module_spine_compat` test asserting layout equality.

### Scope of module + mixin migration

| Today                              | After                                                      | Mixin                  |
| ---------------------------------- | ---------------------------------------------------------- | ---------------------- |
| `class BufReader[R]` over {File, TcpStream}  | `module BufReader { class File, class Tcp }`     | `Reader`               |
| `class BufWriter[W]` over {File, TcpStream}  | `module BufWriter { class File, class Tcp }`     | `Writer`               |
| `enum Shutdown { Read=0, Write=1, Both=2 }`  | `module Shutdown { class Read, class Write, class Both }` | `ShutdownTag` (empty — variant identity only) |
| `enum SeekFrom { Start(o), End(o), Current(o) }` | `module SeekFrom { class Start, class End, class Current }` | `SeekOffset` (the `offset` field accessor) |
| `enum IoError { NotFound, PermissionDenied, ... }` | `module IoError { class NotFound, class PermissionDenied, ... }` | `IoErrorKind` (`message`, `kind` accessors) |
| `enum Result[T,E] { Ok(t), Err(e) }`         | `module Result { class Ok[T,E], class Err[T,E] }` | `Resultlike[T,E]` (`unwrap`, `map`, `and_then`, …) |
| `enum Option[T] { Some(t), None }`           | `module Option { class Some[T], class None[T] }`  | `Optionlike[T]` (`unwrap`, `map`, …) |

**Major caveat on Result/Option:** these are deeply load-bearing
enum types whose variant tags are pinned by name (`Result_Ok=1`,
`Option_Some=1`). Reshaping them into modules+classes is a tagged-enum
widening per [[project_riven_tagged_enum_widening_pattern]] —
4-layer touch (runtime.c → resolve → typeck → codegen) plus every
existing `Result.Ok(x)` / `Some(x)` pattern-match path. **Treat as
a separate prompt (#06.97 or similar) — do NOT bundle into 06.95.**
This plan keeps Result/Option as enums for the initial landing and
adds them to the migration backlog.

The four within-scope migrations for 06.95: `BufReader`, `BufWriter`,
`Shutdown`, `SeekFrom`, `IoError`. Each lands as one commit during
Phase B-tail or Phase D depending on dep layer.

---

## Pre-flight checks (run before Phase A)

Status: **probed; results captured below; one spike still required.**

### Check 1 — Mixin `lib_decls` wire-through to including classes

**Status:** *fixed and pinned* (commit landed alongside this plan
revision). The spike found the gap and the fix shipped as part of
the pre-flight phase. See pin test
`compiler/riven_core/tests/mixin_lib_decl_propagation.rs` +
fixture `tests/fixtures/riven/mixin_lib_decl_propagates.rvn`.

**Original gap (kept here for context):** plumbed at the mixin
DefId level, propagation through `include` was *unverified*.

`compiler/riven_core/src/resolve/ffi_registration.rs:540-564`
(`#06.8 Phase 3b`) already registers mixin-body `lib` FFI decls as
methods attached to the mixin's DefId. The `inner_impls` /
`HirImplItem::Include` machinery exists at
`resolve/items.rs:229-235` for surfacing trait/mixin methods on
the including class.

**Unverified:** whether the include path covers lib-decl methods
specifically, or only `MethodSig` / `DefaultMethod` items. If
lib-decl methods don't flow through `include`, the module+mixin
pattern can't ship — every `BufReader.File.read_line` call would
fail to resolve.

**Spike task (blocking Phase A):**
```riven
# tests/fixtures/riven/mixin_lib_decl_propagates.rvn
mixin Reader
  lib "riven_runtime"
    def hello as "riven_test_extern_add_one"(self, x: Int) -> Int
  end
end

class Thing
  include Reader
end

def main
  let t = Thing.new()
  let r = t.hello(41)
  if r != 42 panic("propagation failed") end
end
```
Add as a pin test under `compiler/riven_core/tests/`. If it
fails, the fix is to extend `register_class_lib_method` (or
parallel) to walk `class.includes` and append mixin lib methods.
Estimated effort: ~80 LOC compiler + 1 pin test.

**Actual outcome (2026-05-19):** test failed exactly as
predicted — typeck accepted `Thing.add_one(41)` but codegen
emitted a call to the unmangled `_Thing_add_one` symbol because
the FFI alias map only carried `Adder_add_one →
riven_test_extern_add_one` (under the *mixin's* name). The
linker errored with `Undefined symbols: _Thing_add_one`.

**Fix landed:**
- `compiler/riven_core/src/resolve/mod.rs`: new field
  `mixin_lib_decls: HashMap<String, Vec<ast::LibDecl>>` on
  `Resolver` + new method `collect_mixin_lib_decls(programs)`
  that walks every top-level item (recursing into modules) and
  snapshots each mixin's lib_decls.
- `compiler/riven_core/src/resolve/bootstrap_merge.rs`: pre-pass
  call `self.collect_mixin_lib_decls(...)` BEFORE Pass 1's
  registration loop. Walks both user program and bootstrap
  programs so include-from-stdlib works in either direction.
- `compiler/riven_core/src/resolve/ffi_registration.rs`: new
  block in the `Class` arm of `register_top_level_type_with_ffi`
  that walks `class.inner_impls` looking for `Include`
  directives, looks up each included mixin's lib_decls in
  `self.mixin_lib_decls`, and calls
  `register_class_lib_method(class_id, &class.name, ffi_fn, …)`
  for each included lib function. This registers a parallel
  `ClassName_method → c-symbol` entry in the FFI alias map
  alongside the original `MixinName_method → c-symbol` entry.

Final cost: ~70 LOC compiler, 1 pin test + 1 fixture, 0 surface
behaviour change beyond the new propagation. Pre-existing
mixin / include / bootstrap pin tests (14 total across
`lib_in_class_body`, `bootstrap_prelude_merge`,
`stdlib_bootstrap`) all green.

### Check 2 — `::Name` root anchor at type position AND module-qualified class resolution

**Status:** the root anchor *and* the whole module-with-inner-classes
shape need real language work. Tracked as **#06.93** (see
`docs/prompts/v1/06_93_module_qualified_class_resolution.md`).

The first sub-probe (`::Name` parser support) failed:

`compiler/riven_core/src/parser/types.rs:305-340`'s
`parse_type_path` requires the first segment to be a
`TypeIdentifier` or `SelfType`. No leading `ColonColon` or `Dot`.
Subsequent segments use `.`, not `::`. The lexer has
`TokenKind::ColonColon` (`token.rs:237`) but it's unused in type
parsing.

**Implication:** inside `module BufReader`, writing
`inner: ::File` to disambiguate from `BufReader.File` is rejected
by the parser.

**Two ways forward; recommend (a):**

**(a) Rename inner classes to avoid collision (no parser change).**
Inside `module BufReader`, name the inner classes after the
*role*, not the *backing type*:
```riven
module BufReader
  class FromFile     # was: File
    lib "riven_runtime"
      def self.new(inner: File) -> BufReader.FromFile
    end
  end
  class FromTcp      # was: Tcp
    lib "riven_runtime"
      def self.new(inner: TcpStream) -> BufReader.FromTcp
    end
  end
end

# Usage:
let br = BufReader.FromFile.new(f)
let br = BufReader.FromTcp.new(s)
```
Same shape for `BufWriter.FromFile` / `BufWriter.FromTcp`. No
collision — `BufReader.FromFile` is unambiguously distinct from
top-level `File`. The slight ugliness at call sites is the
documented cost.

For `Shutdown` / `SeekFrom` / `IoError`: no collision exists
(`Shutdown.Read` doesn't conflict with anything; `SeekFrom.Start`
doesn't either; `IoError.NotFound` doesn't either). So those three
keep clean inner names; only BufReader/BufWriter pay the `From*`
prefix.

**(b) Add `::Name` to the parser (~20 LOC).** Extend
`parse_type_path` to optionally consume a leading `ColonColon`
and mark the resulting `TypePath` as rooted (new field
`pub rooted: bool` on `TypePath`). Resolver then resolves rooted
paths from the global namespace instead of the current scope.
Affects every type-expr resolution site that consumes `TypePath`.

Recommendation: **(a) for the 06.95 plan.** Defer `::Name` to a
separate language-feature prompt where it can be designed
holistically (it also affects expression paths, use paths,
turbofish, etc.).

**Outcome (2026-05-19):** during the pre-flight, a follow-up
probe (`tests/fixtures/riven/module_class_qualified_type.rvn` +
`tests/module_class_qualified_type.rs`, currently `#[ignore]`)
discovered that the module + class shape itself isn't supported
by the resolver — classes inside modules don't get registered
under their qualified name, and call sites like
`Outer.Inner.method(...)` get misresolved as enum variant
patterns. This is much bigger than the (a) renaming workaround
can mask, and it's load-bearing for Decision #3 (BufReader /
BufWriter module + mixin shape) and Decision #4 (broader scope
to Shutdown / SeekFrom / IoError).

**Resolution:** picked option (b) — build the language feature
as its own prompt **#06.93** before 06.95 Phase C. The renaming
sidestep (a) is no longer needed; once 06.93 lands, BufReader
can use `class File` / `class Tcp` with `::File` to refer to the
top-level OS handle. Phase C of 06.95 cannot start until
06.93's success-criterion test passes.

### Check 3 — Turbofish-like type args at call sites

Listed for completeness (was implied by an earlier discussion of
`BufReader[File].new(f)`). With the module+mixin shape adopted,
no longer required for the BufReader migration. Recheck the
parser only if a future prompt revives generic-directed dispatch.

---

## Risks and mitigations

- **R1 — Hidden cross-package coupling.** Today the flat-file loader
  uses `BOOTSTRAP_FILES` ordering to side-step the fact that
  `fs.rvn` references `IoError` from `io.rvn`. With per-package
  manifests, every cross-reference must be a declared dep. **Mitigation:**
  Phase A ships the topo sort + cycle detection before any content
  moves; Phase B lands packages one dep-layer at a time so each layer
  validates against the next.
- **R2 — Codemod misses a call site.** Free-fn → class-method rewrite
  on `examples/*` is easy; on third-party code we don't control it's a
  user-facing break. **Mitigation:** the new compiler diagnostic for
  bare `println(...)` includes the class-prefixed suggestion verbatim;
  the codemod ships with `--check` mode that lists call sites without
  modifying.
- **R3 — runtime_table deletion breaks a niche dispatch.** 525 LOC of
  pattern-matching is hard to mentally audit. **Mitigation:** Phase E
  proceeds category-by-category, each behind its own commit + pin
  test. The full e2e suite catches anything the categorisation
  missed.
- **R4 — Loader becomes a perf cliff.** Walking 19 packages on every
  compiler invocation is more I/O than reading 18 flat files.
  **Mitigation:** keep the parse cache from #06.8 (it caches the
  parsed `Program` AST per file, so 19 packages → 19 cache lookups
  instead of 18). Negligible in practice.
- **R5 — Self-referential dep on `core`.** Every package depends on
  `std-core` (it owns the `Displayable`/`Error` traits used in return
  types). If `core` ends up needing a dep itself, the dep graph
  inverts. **Mitigation:** `core` MUST stay leaf-pure — primitive
  trait declarations only, no FFI, no other class references. Pin
  test `core_has_no_deps.rs`.

---

## Estimated effort

| Phase                          | LOC moved | Files touched | Compiler-side test impact            |
| ------------------------------ | --------- | ------------- | ------------------------------------ |
| A — discovery loader           | ~400      | ~5            | 1 new pin test                       |
| B — split into 19 packages     | ~850      | ~60           | 19 per-package pin tests + 1 e2e     |
| C — class-ify 44 free fns      | ~600      | ~25           | 19 pin tests for new surface + e2e   |
| D — delete `resolve/stdlib/`   | ~1000     | ~10           | trait / module / alias pin tests + e2e |
| E — delete `runtime_table/`    | ~525      | ~15           | 4 per-category pin tests + full e2e  |
| **Total**                      | ~3375     | ~115          | ~50 pin tests + 5 e2e runs           |

Each phase is independently revertable. Phases A and B are pure
infrastructure with zero user-visible change. Phase C is the only
breaking-change phase. Phases D and E remove Rust LOC without
user-visible behaviour change (assuming Phase C surfaces the breaks
upfront).
