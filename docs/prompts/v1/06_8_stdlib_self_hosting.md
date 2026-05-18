# 06.8 — stdlib self-hosting: migrate all stdlib classes from Rust registrations to Riven source

**Depends on:** #06.5 (sync I/O completeness must land first — File /
BufReader / TcpStream are part of the migration set, and migrating
them mid-design would force two passes).
**Reads:** `compiler/riven_core/src/resolve/stdlib/mod.rs` (the
2215-line Rust file this prompt mostly deletes); `library/std/src/iter.rvn`
and `library/std/src/net.rvn` (aspirational docs — see "the dirty
secret" below); `docs/specs/stdlib/*.spec.md` (every spec eventually
points at the .rvn module that satisfies it).

## Why this prompt exists

Riven today is a self-hosted language with a non-self-hosted stdlib.
The compiler is Rust; the stdlib's *runtime* (`library/runtime/*.c`)
is C; but the stdlib's *user-facing surface* — `File`, `Command`,
`Duration`, `Instant`, `Stdin/Stdout/Stderr`, `Map`, `Set`, every
string method, every array method, every IoError variant — is
registered in **~2200 lines of Rust** inside the compiler at
`compiler/riven_core/src/resolve/stdlib/mod.rs`. The Rust code
calls `r.symbols.define("File", DefKind::Class { ... })` for every
class, registers each method's signature inline, and wires the
codegen to emit C calls via a hand-maintained table in
`compiler/riven_core/src/codegen/runtime_table/mod.rs`.

This works, but it's a real architectural smell. Every consequence:

1. **Adding a stdlib method requires recompiling the compiler.** A
   one-line change to "what does `String.upcase` return" forces a
   full rebuild of ~26 KLOC of Rust. That's a 30-second pause for
   every stdlib iteration.
2. **LSP can't navigate to stdlib.** A user who Cmd-clicks `File.open`
   has nowhere to land — there is no source file. The single
   strongest learning surface in Rust/Go/Python (read the stdlib
   source) is unavailable in Riven.
3. **Users can't learn idioms from stdlib.** New users in every
   other language learn "this is how a class with a resource handle
   is written" by reading `File` or `io::BufReader`. In Riven they
   land on Rust code that doesn't even use Riven syntax.
4. **The "language vs library" line is invisible.** A reader of
   `compiler/riven_core/src/resolve/stdlib/mod.rs` cannot tell what
   is a compiler intrinsic versus what is a library type. Every
   stdlib class looks like a built-in.
5. **The runtime_table is a parallel registry.** Two files must
   stay in sync — `resolve/stdlib/mod.rs` (Riven-name → method
   signature) and `codegen/runtime_table/mod.rs` (Riven-name →
   C-symbol). Forgetting either one fails at codegen with an
   unhelpful error.
6. **Stdlib changes can't ship as patches.** A `riven-extra-fs`
   package can never expose itself as `std.fs` — only the compiler
   can register names there. This caps the package ecosystem.

The end-state we want is the one Rust, Go, Swift, Python, OCaml,
Haskell, and basically every other production language reached:
*the compiler is in language X, the stdlib is in language Riven,
the runtime layer is the narrow waist between them.*

## The current state, exactly

### The dirty secret: 100% of stdlib is in Rust today

Before estimating scope, audit what's actually loaded. The two
"Riven stdlib" files (`library/std/src/iter.rvn` and
`library/std/src/net.rvn`) carry banners that quietly admit the
truth:

> "This file is declarative documentation for the intended stdlib
> source layout. **v1 still models the executable behavior through
> built-in resolver/typeck/MIR hooks rather than loading `.rvn`
> stdlib modules.**"

The compiler does not parse or load these files at all. They exist
as human-readable specifications of what the resolver-registered
classes should look like when expressed in Riven syntax. The actual
`Iterator` mixin, `TcpListener` class, etc. are defined in Rust:

```rust
// compiler/riven_core/src/resolve/stdlib/mod.rs:81
("Iterator", vec!["next"]),
("FromIterator", vec!["from_iter"]),
```

So the real state of the stdlib today is:

| Module        | Authored in                              | Has aspirational `.rvn`? | Lines (approx) |
|---------------|------------------------------------------|--------------------------|----------------|
| `iterator`    | ❌ `resolve/stdlib/mod.rs` (Rust)        | ✅ `iter.rvn` (docs only)| ~80 Rust       |
| `net`         | ❌ `resolve/stdlib/mod.rs` (Rust)        | ✅ `net.rvn` (docs only) | ~250 Rust      |
| `io`          | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~250 Rust      |
| `fs`          | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~150 Rust      |
| `env`         | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~80 Rust       |
| `process`     | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~180 Rust      |
| `time`        | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~120 Rust      |
| `fmt`         | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~100 Rust      |
| `string`      | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~250 Rust      |
| `array`       | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~280 Rust      |
| `map`         | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~180 Rust      |
| `set`         | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~140 Rust      |
| `hash`        | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~60 Rust       |
| `path`        | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~40 Rust       |
| `option_result` | ❌ partly intrinsic, partly registrations | ❌                      | ~120 Rust      |
| `primitives`  | (compiler intrinsics — STAY in compiler) | n/a                       | n/a            |
| `prelude`     | (auto-import wiring — STAY in compiler)  | n/a                       | n/a            |
| `rand` (T8)   | ❌ `resolve/stdlib/mod.rs` (Rust)        | ❌                        | ~40 Rust       |

**Zero modules** are actually executed from a `.rvn` source today.
The two "Riven stdlib" files are unloaded docs.

### What we actually need to do

Two distinct things, often confused:

1. **Make the compiler LOAD `.rvn` stdlib files** at startup (the
   bootstrap loader described in §2 below). This is the foundation
   — nothing else works without it.
2. **Move each stdlib class FROM Rust registrations INTO a `.rvn`
   file** (the per-class migration described in §3). The `iter.rvn`
   and `net.rvn` files that exist today are a huge head-start for
   this — they are already written, they just aren't loaded. Once
   the loader works, those two are 90%-migrated for free; only
   the `extern "C"` bindings need to be added.

### Target

Every cell in the "Authored in" column that isn't a compiler
intrinsic flips to ✅, AND each `.rvn` file is genuinely loaded by
the compiler (banner removed; the source is the source of truth).
The Rust file shrinks from ~2200 lines to ~300 lines — only the
pieces that genuinely have to live in the compiler (primitive type
registration, prelude auto-import wiring, the FFI declaration
parsing path itself, the small set of compiler-magic types like
`Result` and `Option` whose codegen the compiler intrinsifies for
`?` and `match`).

## The blocker

There is no way today for a `.rvn` file to declare "this Riven
method is implemented by C symbol `riven_file_open`." Every
existing `.rvn` file can only call other Riven code. The Rust
registrations bypass this by registering the method AND its
C-symbol mapping in the same step, server-side, before any
user-facing Riven file is parsed.

We need a Riven-language way to express that binding. That is the
core deliverable of this prompt.

## Surface, in priority order

### 1. `extern "C"` declarations in Riven (the keystone)

```riven
# Declare that this Riven free function is backed by a C symbol.
# Codegen emits a direct call to `riven_time_now_ns`; no Riven
# body required.
extern "riven_time_now_ns"
def time_now_ns_raw() -> Int
end

# Same, for methods on a class.
class File
  extern "riven_file_open"
  def self.open(path: &String) -> Result[File, IoError]
  end

  extern "riven_file_read"
  def read(self, buf: &mut Array[U8]) -> Result[Int, IoError]
  end

  extern "riven_file_close"
  def close(self) -> Result[(), IoError]
  end
end

# Tagged enum whose runtime layout MUST match what the C runtime
# produces. The `#[repr(tagged)]` attribute pins the layout so the
# resolver checks (and the compiler refuses to reorder) variants.
#[repr(tagged)]
enum IoError
  NotFound(String)        # tag = 0
  PermissionDenied(String) # tag = 1
  # … exactly the order C code produces …
end
```

**Surface rules:**
- `extern "<c-symbol>"` attaches to the *next* `def`. No body
  allowed (parser refuses one). The Riven signature is the source
  of truth for typeck; the C symbol must exist at link time or
  linking fails (which is the right failure mode — same as Rust).
- The C symbol name is a verbatim string. No mangling. No
  namespacing. The author is explicitly stating "I am binding this
  exact symbol."
- `extern` is allowed at module top-level (for free functions),
  inside a `class` body (for methods, including `self.` static
  methods), and inside a `mixin` body (for default-implemented
  methods backed by C — rare; deferrable).
- `extern` declarations require an explicit return type and explicit
  parameter types. No inference. The FFI boundary is the worst
  possible place for inference surprises.
- `#[repr(tagged)]` on an enum pins variant order; the resolver
  refuses to compile a file where two `#[repr(tagged)]` enums in
  scope would collide on layout. Variant addition is allowed at
  the END only (matches the existing tag-stability pin-test
  pattern from `project_riven_tagged_enum_widening_pattern` in
  the team's memory).
- `#[repr(flat_heap_struct)]` on a class pins the C layout to a
  flat heap struct (the existing `RivenFile`, `RivenCommand`,
  `RivenTcpStream` pattern). The class body still lists fields in
  Riven; the attribute just says "the C runtime knows about this
  layout and will allocate / free instances of it."

### 2. Bootstrap path — how stdlib `.rvn` files reach the user

Today the prelude auto-imports a fixed set of names. After this
prompt:

- Compiler keeps a small bootstrap list: `iter.rvn`, `io.rvn`,
  `fs.rvn`, `env.rvn`, `process.rvn`, `time.rvn`, `fmt.rvn`,
  `string.rvn`, `array.rvn`, `map.rvn`, `set.rvn`, `hash.rvn`,
  `path.rvn`, `net.rvn`, `result_option.rvn`, `rand.rvn`. These
  paths are relative to `library/std/src/`.
- At compiler start, before user code is parsed, the resolver
  walks the bootstrap list, parses each file, resolves its
  top-level names, and inserts them into the prelude scope.
- Stdlib `.rvn` files are parsed by the SAME parser the user's
  code uses. No second grammar. If the parser can't parse stdlib
  code, the parser is broken — this becomes the most-tested code
  in the project for free.
- Bootstrap failures are fatal compiler errors with file:line — not
  Rust panics. The error message must say "stdlib bootstrap
  failed at `library/std/src/io.rvn:42`" so a contributor fixing
  stdlib never wonders why the test suite is exploding mysteriously.
- Each stdlib `.rvn` file's path is canonicalized relative to
  `RIVEN_STDLIB_PATH` (env var) with a build-time fallback to the
  installed `library/std/src/` next to the `rivenc` binary. This
  matches how Rust's sysroot lookup works and lets contributors run
  against a checkout without setting anything.

### 3. The migration: one class per commit

The actual class-by-class movement. The order matters because some
classes depend on others (Result/Option ⇐ everything; String ⇐
most; Array ⇐ many; IoError ⇐ io/fs/net/process).

**Phase 1 — leaf modules (no internal stdlib deps beyond
String/Array/Result/Option/IoError):**

1. **`iter.rvn` + `net.rvn` first** — these files already exist as
   aspirational docs; the migration is "strip the disclaimer banner,
   add `extern "C"` bindings for each method, delete the Rust
   registrations, run tests." Doing these first validates the
   bootstrap loader against pre-written real-world code instead of
   a toy.
2. `rand.rvn` (smallest greenfield surface; second proof of the
   extern path)
3. `path.rvn` (single function today; trivially small)
4. `hash.rvn` (Hashable mixin + 1-2 free fns)
5. `env.rvn` (args, get, vars, current_dir)
6. `time.rvn` (Duration, Instant, sleep, unix_ns)
7. `fmt.rvn` (Display, Debug, Formatter, the format! macro is
   compiler-side and stays)

**Phase 2 — IoError + I/O cluster (everything tagged-enum-y):**

7. `io.rvn` — IoError enum (the big one; uses #[repr(tagged)]),
   Stdin/Stdout/Stderr, File, BufReader, BufWriter
8. `fs.rvn` — fs.{read_to_string, write, copy, rename, …}, Metadata
9. `process.rvn` — Command, ExitStatus, Output

**Phase 3 — (net already done in Phase 1 step 1):**

10. (no-op — net.rvn handled in Phase 1 step 1 above.)

**Phase 4 — collections (largest surface area):**

11. `string.rvn` — String methods (split, trim, lines, replace,
    contains, …) — about 30 methods
12. `array.rvn` — Array methods (push, pop, slice, sort_by, retain,
    extend, …) — about 40 methods
13. `map.rvn` — HashMap, Entry API
14. `set.rvn` — HashSet
15. `result_option.rvn` — Result/Option helper methods that aren't
    compiler intrinsics (`.map`, `.and_then`, `.unwrap_or`, …)

Each migration is exactly one commit. The commit message format is:
`stdlib(<module>): migrate from Rust registrations to Riven source`.
Each commit deletes ~N lines of Rust from `resolve/stdlib/mod.rs`
and adds a `library/std/src/<module>.rvn`. The pin tests for that
module must stay green before AND after the migration — if a test
breaks during the migration, the commit is wrong.

### 4. Cleanup: shrink `resolve/stdlib/mod.rs`

After all migrations, the file should contain only:

- Primitive type registrations (`Int`, `Bool`, `String`, …) — these
  ARE compiler intrinsics and stay
- The bootstrap-list constant: `&[("iter", include_str!(...)), …]`
  if you choose to embed the stdlib in the binary; otherwise just
  the path list
- Prelude auto-import wiring
- The minimal set of compiler-magic types that `?` / `match` /
  closures need

If `resolve/stdlib/mod.rs` is still over ~400 lines at the end of
this prompt, something didn't get migrated. Audit before declaring
done.

### 5. The runtime_table goes away

`compiler/riven_core/src/codegen/runtime_table/mod.rs` exists today
to map Riven names to C symbols. After this prompt, every C call
site is reachable via `extern "C-symbol"` in a `.rvn` file. The
runtime_table becomes redundant: codegen looks up the
`extern`-binding on the method's `DefKind`, emits a direct call to
the named C symbol. Delete `runtime_table` entirely.

If the build still references `runtime_table` after the migration,
something didn't get migrated.

## TDD

This is a high-stakes migration — the existing test suite is the
safety net.

1. **Existing pin tests are the regression gate.** Every
   `compiler/riven_core/tests/stdlib_*.rs` file already pins
   stdlib behavior. After each per-module migration commit, run
   that module's narrow tests:
   ```
   gtimeout 120 cargo test -p riven_core --test stdlib_<module> -- --test-threads=1 2>&1 | tee tmp/test-cache/p06_8-<module>-vN.log
   ```
   All tests that passed before must still pass. Any new failure =
   migration is wrong; do not paper over.

2. **New pin tests for the extern path itself.** Before migrating
   any stdlib module, write tests in a new file
   `compiler/riven_core/tests/extern_c_binding.rs`:
   - `extern_decl_resolves_to_c_symbol` — parse a .rvn file with one
     `extern "foo"`, confirm the DefKind carries the symbol name.
   - `extern_decl_typeck_uses_declared_signature` — pass wrong-typed
     args, expect typeck error.
   - `extern_decl_with_body_is_rejected` — parser error.
   - `extern_decl_links_to_c_runtime` — full compile-and-run smoke
     test that binds a tiny test C symbol (added to
     `library/runtime/test_extern.c`) and calls it from Riven.
   - `repr_tagged_pins_variant_order` — declare an enum, swap two
     variants, expect compiler refusal (with a tag-stability
     diagnostic).
   - `bootstrap_failure_in_stdlib_file_has_file_line` — corrupt
     `library/std/src/io.rvn` (in test setup), expect the bootstrap
     error message to name the file and line.

3. **`#06.5` final e2e cases are the user-visible regression gate.**
   The `tests/release-e2e/cases/5XX_*.rvn` fixtures from #06.5 (file,
   fs, time, tcp, etc.) MUST keep passing across every migration.
   Run after each Phase ends:
   ```
   gtimeout 600 cargo test --test release_e2e_smoke -- --ignored 2>&1 | tee tmp/test-cache/p06_8-phase<N>-e2e.log
   ```

4. **Compiler self-test: parse the stdlib without panicking.** Add a
   test that loads every `library/std/src/*.rvn` and verifies the
   parser + resolver succeed. This is the bootstrap path's
   regression gate — if the stdlib breaks under a parser change,
   this fails first and loudly.

## Implementation order (suggested)

1. **Land the `extern "C"` parser + resolver path + pin tests.** No
   migration yet. The compiler accepts `extern "foo" def …` but no
   stdlib file uses it. Existing stdlib stays registered in Rust.
   Verify: all `extern_c_binding.rs` tests green, full workspace
   test still green.

2. **Land the bootstrap-loader for `.rvn` stdlib files.** Compiler
   reads `library/std/src/<module>.rvn` for each module in the
   bootstrap list at startup. INITIALLY, every stdlib file is
   empty (or just has a comment). The Rust registrations still do
   all the real work. This isolates the loader from the migration
   risk.

3. **Migrate `rand.rvn` first** — smallest surface, no dependencies.
   This is the proof-of-life. Once `rand` works end-to-end through
   the `.rvn` file, the rest is mechanical.

4. **Migrate the leaf modules in Phase 1 order** — one commit each.
   Run that module's narrow tests after each commit; full workspace
   test after each module.

5. **Migrate IoError + I/O cluster (Phase 2)** — the IoError tagged
   enum is the highest-risk single migration because runtime C code
   in `library/runtime/io/io_error.c` produces values matching the
   resolver-registered layout. Use `#[repr(tagged)]` with an
   explicit tag-stability pin test BEFORE removing the Rust
   registration.

6. **Migrate net (Phase 3)** — small; mostly already done.

7. **Migrate collections (Phase 4)** — String first (most leaf-y),
   then Array, then Map, then Set, then result_option.

8. **Delete the runtime_table and the now-empty parts of
   `resolve/stdlib/mod.rs`.** This is the satisfying commit.

9. **Final workspace pass.** Cache to `tmp/test-cache/p06_8-final.log`.

## Reserved error codes

- E0720 — `extern "C"` declaration has a body
- E0721 — `extern "C"` declaration is missing required type
            annotations (param or return)
- E0722 — `extern "C"` symbol used in two `extern` declarations
            with conflicting Riven signatures
- E0723 — `#[repr(tagged)]` enum has duplicate or out-of-order tags
- E0724 — `#[repr(flat_heap_struct)]` class layout disagrees with
            the registered C struct size at link time (best-effort:
            the runtime exposes a `riven_<class>_layout_check`
            symbol that the linker invokes from a constructor)
- E0725 — stdlib bootstrap failed (file not found / parser error /
            resolve error in a stdlib `.rvn` file). Always cites
            the offending stdlib file + line.

## Definition of done

- [ ] `extern "C-symbol" def …` parses, resolves, typechecks, and
      codegens to a direct C call.
- [ ] `#[repr(tagged)]` on enums pins variant order with a
      compile-time check.
- [ ] `#[repr(flat_heap_struct)]` on classes documents (and ideally
      verifies at link time) the C layout.
- [ ] Stdlib bootstrap loader reads every `library/std/src/*.rvn`
      at compiler startup, with file:line errors on failure.
- [ ] All stdlib modules in the "Authored in" table above flipped
      from ❌ Rust to ✅ Riven (including iter.rvn and net.rvn,
      whose existing "declarative documentation" banners are
      stripped because the files are now genuinely loaded).
- [ ] `compiler/riven_core/src/resolve/stdlib/mod.rs` is under
      ~400 lines (only primitives, prelude wiring, compiler magic).
- [ ] `compiler/riven_core/src/codegen/runtime_table/` is deleted.
- [ ] All pre-existing stdlib pin tests still green (regression
      gate).
- [ ] All pre-existing release-e2e fixtures still green
      (user-visible regression gate).
- [ ] New `extern_c_binding.rs` pin tests green.
- [ ] `cargo test --workspace` green (cache to
      `tmp/test-cache/p06_8-final.log`).
- [ ] CHANGELOG bullet under `## [Unreleased] ### Changed`:
      "stdlib: migrated from Rust registrations to self-hosted
      Riven source. `extern \"C-symbol\"` syntax now binds C
      runtime symbols from `.rvn` files. `library/std/src/` is the
      authoritative source of every stdlib class."
- [ ] `docs/STRATEGY.md` updated to reflect that the stdlib is
      self-hosted.
- [ ] `docs/specs/stdlib/*.spec.md` "shipped in" lines updated to
      point at the `.rvn` file instead of `resolve/stdlib/mod.rs`.

## Anti-goals

- **Rewriting the C runtime.** `library/runtime/*.c` stays exactly
  as it is. Only the *Riven-facing surface* moves; the C
  implementations of file I/O, sockets, hashing, etc. don't change.
- **Self-hosting the compiler.** That's a much bigger prompt (and
  may never make sense given LLVM is Rust-comfortable). This
  prompt only self-hosts the *library*, not the *compiler*.
- **Rewriting primitives.** `Int`, `Bool`, `String` (as a type),
  `Array` (as a type) stay compiler-known. Their *methods* move to
  `.rvn`, but the types themselves remain compiler intrinsics
  because the codegen for `+`, indexing, `len`, etc. is
  intrinsified.
- **Changing the runtime ABI.** The flat-heap-struct layouts that
  `library/runtime/*.c` produces stay byte-identical. If a
  migration would force an ABI change, the migration is wrong.
- **Generics-over-C-bindings.** A monomorphized generic class
  whose methods are `extern "C"` would need per-instantiation C
  symbols. That's a future prompt (probably never needed if
  `BufReader[R]` etc. just dispatch via a small Riven adapter
  layer over the per-inner-type C fns).
- **Letting user packages register names in `std.*`.** Out of scope.
  Future prompt if/when the package ecosystem demands it.

## Why this comes after #06.5 and before / alongside #10 (LSP)

After #06.5: the sync I/O surface is locked. Migrating File mid-
design would force a second pass. Now that File, BufReader,
TcpStream, etc. are stable, moving them from Rust to .rvn is a
mechanical refactor.

Before or alongside #10 (LSP): the LSP gains massive value the
moment stdlib classes have a `.rvn` source — Cmd-click navigation,
hover docs, find-references all light up for stdlib for free.
Slotting #06.8 before #10 means LSP work doesn't need a "stdlib
navigation" follow-up.

## Estimated scope

- Phase 0 (extern path + bootstrap loader): 3-4 days
- Phase 1 (leaf modules: rand → fmt): 5-6 days (one per day)
- Phase 2 (io + fs + process): 4-5 days
- Phase 3 (net extension): 1 day
- Phase 4 (collections): 6-8 days
- Cleanup + final workspace pass + docs: 1-2 days

Total: ~3 weeks of focused work. Can parallelize: Phase 1 modules
are independent of each other; Phase 4 collections are
independent of each other.
