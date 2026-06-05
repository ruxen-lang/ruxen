# Ruxen Specs

Ruxen uses **Spec-Druxen Development (SDD)**.  Every feature flows:

```
docs/requirements (or tutorial / prompts)
        ↓
docs/specs/<area>/<feature>.spec.md         ← this directory
        ↓
crates/ruxen-core/tests/*.rs (pin tests)
        ↓
implementation
```

Specs are the **source of truth**.  Tests are the **enforcement
layer** — every numbered behaviour (B1, B2, …) in a spec has at least
one Rust integration test or release-e2e fixture that pins it.

## Spec file shape

Each `.spec.md` follows the same skeleton:

```markdown
# Spec — <area>.<feature>

**Source docs:** links to docs/requirements + docs/prompts
**Status:** when it shipped + phase tag

(short intro)

## B1 — <one-line behaviour title>

Given / When / Then in prose.

## B2 — …

…

## Pin tests

| Behaviour | Test fn | File |
|-----------|---------|------|
| B1        | …       | …    |

## Out of scope (v2)
- deferred items, with reasons
```

The "Pin tests" table is the contract — every `B<n>` must appear in
at least one row.  If a behaviour ships without a pin test, list it
in a **Gaps** section so the missing coverage is visible.

## Adding a new feature

1. **Identify the source doc.**  Usually `docs/requirements/tier*` or
   `docs/prompts/v1/`.  If none exists, write the doc first.
2. **Write the spec.**  Create `docs/specs/<area>/<feature>.spec.md`
   with numbered behaviours.  Submit for review before opening any
   source file.
3. **Implement TDD.**  For each `B<n>`, write the pin test first,
   watch it fail, then implement until it passes.  Add the test to
   the spec's Pin tests table.
4. **Iterate.**  `cargo test --workspace` must be green at every
   commit.  If a behaviour changes, update the spec first.

## Backfilling

Specs are being backfilled for already-shipped features as we touch
them.  Pin tests usually already exist as Rust integration tests —
cross-link rather than duplicate.

## Index

### stdlib (Phase 2 #04-#06)

- [std.fmt](stdlib/fmt.spec.md) — Display, Debug, Formatter,
  interpolation routing (D2), format specs (D4).
- [std.io](stdlib/io.spec.md) — Stdin, Stdout, Stderr, IoError.
- [std.env](stdlib/env.spec.md) — args, var, vars, current_dir.
- [std.fs](stdlib/fs.spec.md) — read_to_string, write, read_dir,
  predicates.
- [std.process](stdlib/process.spec.md) — exit, Command builder.
- [std.path](stdlib/path.spec.md) — POSIX path manipulation.
- [std.time](stdlib/time.spec.md) — `unix_ns`, Instant, Duration.
- [std.net](stdlib/net.spec.md) — minimal TCP surface.
- [std.iter (Iterator)](stdlib/iterator.spec.md) — pipeline +
  collect surface.
- [Hash](stdlib/map.spec.md) — separate-chaining hash table.
- [Set](stdlib/set.spec.md) — `Hash[T, nil]` alias.
- [Array](stdlib/array.spec.md) — growable contiguous array.
- [String / &str](stdlib/string.spec.md) — UTF-8 owned + borrowed
  string surface and ownership negatives.
- [Option / Result](stdlib/option_result.spec.md) — tagged-enum
  surface, `?` operator, `if let Some`, `expect!`, `unwrap_or`,
  `map`.
- [std.sync](stdlib/sync.spec.md) — concurrency surface: Thread,
  Mutex, SharedSync, JoinHandle, MutexGuard.  v1: typeck contract +
  `Thread.sleep`/`yield_now` runtime; full runtime in Phase 4.
- [std.future / async](stdlib/async.spec.md) — Future mixin, Poll
  enum, Waker, Context, `async def` / `async { }` / `.await` syntax.
  v1: parser + typeck only.  Executor in Phase 4.
- [Primitives](stdlib/primitives.spec.md) — Int / sized integers /
  Float / Bool / Char method surfaces, numeric literal suffixes
  (`123u8`, `1.5f32`), escape sequences (`?\n`, `?\u{1F600}`).
- [std.prelude](stdlib/prelude.spec.md) — auto-imported names
  available without a `use` statement.

### Mixins

- [Implicit includes](mixins/implicit_includes.spec.md) — Debug, Clone,
  PartialEq, Eq, Hashable, Default, Ord, PartialOrd, Copy.
- [Mixin system](mixins/system.spec.md) — declaration, `include`
  directive, default methods, inheritance, assoc types, `some Mixin`,
  `any Mixin`, multi-bound, `where`, class-level methods.
- [Variance](mixins/variance.spec.md) — invariance for `&var T` /
  `Array[T]`; covariance for `Option[T]`.

### Ownership

- [Borrow check](ownership/borrow-check.spec.md) — move / ref /
  var-ref rejection envelope.
- [Drop](ownership/drop.spec.md) — drop elaboration, user
  `include Drop`, leak-tracker fixtures.

### Type system (in flight)

- [Const generics](types/const-generics.spec.md) — `const N: Type`
  generic-param surface, `Array[T, N]`, etc.  Stage 1 (parser-only)
  shipped 2026-05-13 (commit b8a371c); S2-S9 pending.

### Codegen

- [Backends](codegen/backends.spec.md) — Cranelift (default) + LLVM
  18 (feature-gated); byte-identical stdout invariant.
- [Runtime safety](codegen/runtime-safety.spec.md) — strict warnings,
  sanitisers, ABI pins, 64-bit pointer asserts.
- [FFI](codegen/ffi.spec.md) — Phase 7 unsafe blocks, raw pointers,
  `lib "..."` blocks, `layout c` / `layout packed` / `layout transparent`.

### System

- [Module resolution + runtime startup](system/module-resolution.spec.md) —
  `use std.x.{...}` import surface, group imports, method dispatch
  on imported types, end-to-end round-trips, `main` shim argv init.
- [User-defined modules](system/user-modules.spec.md) — `module foo
  ... end` parser surface (shipped) + resolver path lookup
  (pending).

### Future (backfill as we touch them)

- std.hash (top-level hashing utilities — separate from `Hash` /
  `Set`).
- Error-code registry as its own spec (currently informal —
  `implicit_includes.spec.md` B12 lists the relevant codes).
- LSP / formatter / REPL / package manager — not yet spec'd.
- User-defined modules (`module foo ... end`) — parsed but resolve
  only handles `std.*` paths today.

## Cross-references

- Long-form requirements: [docs/requirements/](../requirements/).
- Phase prompts (driver scripts): [docs/prompts/v1/](../prompts/v1/).
- Implementation plans: [docs/superpowers/plans/](../superpowers/plans/).
- Tutorial (user-facing): [docs/tutorial/](../tutorial/).
