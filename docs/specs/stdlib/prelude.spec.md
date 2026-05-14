# Spec — `std.prelude`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.1](../../requirements/tier1_01_stdlib.md).

**Status:** shipped since Phase 1; expands incrementally as new
stdlib types land.

`std.prelude` is the set of names auto-imported into every Riven
program — types and free functions that are reachable without a
`use` statement.

---

## B1 — Auto-imported types

The following type names are in scope at program start without any
`use`:

| Type          | Origin module        |
|---------------|----------------------|
| `Int`, `Int8` `Int16` `Int32` `Int64`, `ISize` | builtin |
| `UInt`, `UInt8` `UInt16` `UInt32` `UInt64`, `USize` | builtin |
| `Float`, `Float32`, `Float64` | builtin |
| `Bool`, `Char`, `String`, `str` (= `&str`) | builtin |
| `Array[T]`      | `std.array`        |
| `Map[K, V]`     | `std.collections`  |
| `Set[T]`        | `std.collections`  |
| `Option[T]`     | `std.option`       |
| `Result[T, E]`  | `std.result`       |
| `IoError`       | `std.io`           |
| `VarError`      | `std.env`          |
| `FmtError`      | `std.fmt`          |
| `Formatter`     | `std.fmt`          |
| `Display`, `Debug`, `PartialEq`, `Eq`, `Hashable`, `Clone`, `Copy`, `Default`, `Ord`, `PartialOrd`, `Drop`, `Iterator`, `FromIterator` | mixins |

## B2 — Auto-imported free functions

| Function      | Effect                            |
|---------------|-----------------------------------|
| `puts(s)`     | print string + newline to stdout  |
| `eputs(s)`    | print string + newline to stderr  |
| `panic!(msg)` | abort with diagnostic + backtrace |

`println(s)` / `print(s)` / `eprintln(s)` / `eprint(s)` are **not**
in the prelude — call them via `Stdout.new().println(...)` etc.,
or `use std.io.{...}`.

## B3 — Auto-imported enum variants

The variants of `Option` and `Result` are exposed at the
type-path level (`Option.Some(...)`, `Result.Ok(...)`) without
needing `use std.option.Option`.  Bare `Some(x)` / `nil` / `Ok(x)` /
`Err(e)` work in pattern contexts even without explicit imports.

---

## Pin tests

| Behaviour | Test fn / fixture                                         | File                       |
|-----------|-----------------------------------------------------------|----------------------------|
| B1, B3    | every release-e2e fixture that uses `Array` / `Option` / `Result` without `use` (hundreds) | `tests/release-e2e/cases/` |
| B2        | `01_hello.rvn` (`puts`) + `e2e_24_result.rvn` (`panic!`)  | `tests/release-e2e/cases/` |

The prelude is *implicitly* pin-tested by every program that
compiles without importing these names.  A regression would
manifest as widespread "name not in scope" failures across the
fixture set.

---

## Out of scope (v2)

- User-defined prelude extension (`use prelude.X` to opt-in extra names).
- Per-package prelude (Rust's `core.prelude` vs `std.prelude`).
- Glob re-exports inside the prelude (`use std.io.*`).
