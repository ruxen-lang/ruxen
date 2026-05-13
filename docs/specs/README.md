# Riven Specs

Riven uses **Spec-Driven Development (SDD)**.  Every feature flows:

```
docs/requirements (or tutorial / prompts)
        ↓
docs/specs/<area>/<feature>.spec.md         ← this directory
        ↓
crates/riven-core/tests/*.rs (pin tests)
        ↓
implementation
```

Specs are the **source of truth**.  Tests are the **enforcement
layer** — every numbered behaviour (B1, B2, …) in a spec has at least
one Rust integration test or release-e2e fixture that pins it.

## Spec file shape

Each `.spec.md` follows the same skeleton:

```markdown
# Spec — <area>::<feature>

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

- [std::fmt](stdlib/fmt.spec.md) — Display, Debug, Formatter,
  interpolation routing (D2), format specs (D4).
- [std::io](stdlib/io.spec.md) — Stdin, Stdout, Stderr, IoError.
- [std::env](stdlib/env.spec.md) — args, var, vars, current_dir.
- [std::fs](stdlib/fs.spec.md) — read_to_string, write, read_dir,
  predicates.
- [std::process](stdlib/process.spec.md) — exit, process_run.
- [std::path](stdlib/path.spec.md) — POSIX path manipulation.
- [std::time](stdlib/time.spec.md) — `now_ns`, `unix_ns`.
- [std::net](stdlib/net.spec.md) — minimal TCP surface.
- [std::iter (Iterator)](stdlib/iterator.spec.md) — pipeline +
  collect surface.
- [HashMap](stdlib/hashmap.spec.md) — separate-chaining hash table.
- [HashSet](stdlib/hashset.spec.md) — `HashMap[T, ()]` alias.
- [Vec](stdlib/vec.spec.md) — growable contiguous array.
- [String / &str](stdlib/string.spec.md) — UTF-8 owned + borrowed
  string surface and ownership negatives.
- [Option / Result](stdlib/option_result.spec.md) — tagged-enum
  surface, `?` operator, `if let Some`, `expect!`, `unwrap_or`,
  `map`.
- [std::sync](stdlib/sync.spec.md) — concurrency surface: Thread,
  Mutex, Arc, JoinHandle, MutexGuard.  v1: typeck contract +
  `Thread.sleep`/`yield_now` runtime; full runtime in Phase 4.

### Traits

- [derive `<Trait>`](traits/derive.spec.md) — Debug, Clone,
  PartialEq, Eq, Hash, Default, Ord, PartialOrd, Copy.
- [Trait system](traits/system.spec.md) — declaration, impl-for,
  default methods, inheritance, assoc types, `impl Trait`, `dyn
  Trait`, multi-bound, `where`, static methods.
- [Variance](traits/variance.spec.md) — invariance for `&mut T` /
  `Vec[T]`; covariance for `Option[T]`.

### Ownership

- [Borrow check](ownership/borrow-check.spec.md) — move / ref /
  mut-ref rejection envelope.
- [Drop](ownership/drop.spec.md) — drop elaboration, user `impl
  Drop`, leak-tracker fixtures.

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
  `lib` / `extern "C"`, `#[repr(C/packed/transparent)]`.

### System

- [Module resolution + runtime startup](system/module-resolution.spec.md) —
  `use std.x.{...}` import surface, group imports, method dispatch
  on imported types, end-to-end round-trips, `main` shim argv init.

### Future (backfill as we touch them)

- std::hash (top-level hashing utilities — separate from HashMap /
  HashSet).
- Error-code registry as its own spec (currently informal —
  `derive.spec.md` B12 lists the relevant codes).
- LSP / formatter / REPL / package manager — not yet spec'd.
- User-defined modules (`module foo ... end`) — parsed but resolve
  only handles `std.*` paths today.

## Cross-references

- Long-form requirements: [docs/requirements/](../requirements/).
- Phase prompts (driver scripts): [docs/prompts/v1/](../prompts/v1/).
- Implementation plans: [docs/superpowers/plans/](../superpowers/plans/).
- Tutorial (user-facing): [docs/tutorial/](../tutorial/).
