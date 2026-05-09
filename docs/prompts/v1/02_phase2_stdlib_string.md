# 02 — Phase 2 stdlib: full `String` surface

Ship `String` with full method coverage matching `tier1_01_stdlib.md`.
Drop semantics (P0.7) must be done first.

**Depends on:** prompt 01 part A.
**Reads:** `docs/requirements/tier1_01_stdlib.md` §String,
`crates/riven-core/runtime/runtime.c` (existing `riven_string_*`).

## Surface (mandatory)

`String`:
- Constructors: `String.new`, `String.from(&str)`,
  `String.with_capacity(Int)`.
- Inspectors: `len`, `is_empty`, `as_str -> &str`, `chars`, `bytes`,
  `lines`, `split(&str)`, `splitn(Int, &str)`, `contains(&str)`,
  `starts_with(&str)`, `ends_with(&str)`, `find(&str) -> Option[Int]`,
  `to_lower`, `to_upper`, `trim`, `trim_start`, `trim_end`,
  `replace(&str, &str)`, `repeat(Int)`.
- Mutators: `push(Char)`, `push_str(&str)`, `clear`, `truncate(Int)`,
  `insert(Int, Char)`, `insert_str(Int, &str)`,
  `remove(Int) -> Char`.
- Conversions: `parse[Int]`, `parse[Float]`, `to_string`,
  `into_bytes -> Vec[U8]`.
- Operators: `+` (concat owned), `+=` (push_str), `==`, `!=`, `<`,
  ordering, `Hash`.

`&str`:
- All `String` inspectors that don't require ownership.

## TDD

For every method above:

1. Add unit test in `crates/riven-core/tests/stdlib_string.rs` with
   a `.rvn` source string that exercises the method and asserts the
   stdout. Use `compile_and_run` helper (mirror
   `drop_fixtures.rs::compile_and_run_with_tracking` minus the
   tracker).
2. Add e2e fixture pair `tests/release-e2e/cases/3NN_string_<op>.rvn`
   with matching `expected/3NN_string_<op>.out`.
3. For mutating methods, add a `drop_fixtures.rs` test confirming
   `outstanding == 0` afterwards.

## Implementation rules

- Every method has a runtime fn `riven_string_<op>` declared in
  `runtime.c` and dispatched via `codegen/runtime.rs`.
- `&str_*` and `String_*` dispatch tables stay separate (the audit
  already split them; do not collapse).
- Iterators (`chars`, `bytes`, `lines`, `split`) return owned
  `Vec[T]` for v1 — the lazy iterator story lives in prompt 05.
- `parse[T]` returns `Result[T, ParseIntError]` /
  `Result[T, ParseFloatError]`. Define both error types in stdlib.
- No `riven_noop_passthrough` for unimplemented methods; codegen
  must `Err()` for any name not in the dispatch table.

## Negative tests

- `String.from(&non_str)` — type error.
- Borrow-after-move on `String` arguments.
- Slice indices out of range — runtime panic with explicit message.

## Definition of done

- [x] Every method in the surface table has positive + negative tests.
- [x] `tests/release-e2e/cases/3NN_string_*` covers every method.
- [x] No leak in any `drop_fixtures.rs` String test.
- [x] `cargo test --workspace` green.
- [x] Each new error code (E0700+ if needed) registered.
- [x] CHANGELOG bullet listing the new methods grouped by category.
