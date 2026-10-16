# Spec — `Option[T]` / `Result[T, E]`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.4–5.5](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 2 #02-#03; covered by release-e2e fixtures and
the `option_result` package tests; helper-method surface self-hosted in
`library/std/option_result/src/lib.rx` since #06.8 T#17.

`Option` and `Result` are tagged enums whose payload reaches the
runtime as `int64_t`s (tag in the low half, payload pointer / scalar
in the high half) so they pass cleanly through the C ABI.

---

## B1 — `Option.Some(x)` / `nil` round-trip

Programs can construct, pattern-match, and destructure `Option`
values across function boundaries.  Round-trip:
`construct → match → extract → reuse` produces the original value.

## B2 — `Result.Ok(x)` / `Result.Err(e)` round-trip

Same as B1 for `Result`.

## B3 — `?` operator on `Result`

**Given** a method returning `Result[T, E]` and a sub-call
`x?` where `x: Result[T, E]`
**When** `x = Err(e)`
**Then** the enclosing function returns `Err(e)` immediately; `T` is
bound to the unwrapped value on the `Ok` path.

## B4 — `if let Some(x) = opt` pattern matching

`if let Some(x) = opt … else … end` binds `x` to the payload
on the `Some` arm; the `else` arm has no binding.

## B5 — Generic enum usage with payload

**Given** a user enum `enum Result2[T]` parameterised over `T`
**Then** monomorphisation routes the right payload type per
instantiation.

## B6 — `.expect!(msg)` panics on `nil` / `Err` with `msg`

The `!` marks `expect!` as panicking.  On `Some(x)` / `Ok(x)` it returns `x`; on `nil` /
`Err(_)` it panics with `msg`.

## B7 — `.unwrap_or(default)` falls back

Returns the contained value on `Some(x)` / `Ok(x)`; returns
`default` otherwise.

## B8 — `.map(|x| f(x))` transforms the payload

`Option.map` and `Result.map` apply `f` to the contained value when
present and pass through `nil` / `Err` unchanged.

---

## Pin tests

All B1–B8 are covered by release-e2e fixtures (run by
`release_e2e_smoke`) and the `option_result` package tests:

| Behaviour | Fixture                                    | Test                    |
|-----------|--------------------------------------------|-------------------------|
| B1        | `23_option.rx`                            | release-e2e             |
| B2        | `24_result.rx`                            | release-e2e             |
| B3        | `25_question_op.rx`                       | release-e2e             |
| B4        | `56_if_let_some.rx`                       | release-e2e             |
| B5        | `73_enum_generic.rx`                      | release-e2e             |
| B5        | `19_enums_data.rx` + `18_enums_simple.rx`| release-e2e |
| B6        | `library/std/option_result/tests/result_test.rx` | "unwrap_or_else and expect!" |
| B7        | `library/std/option_result/tests/option_test.rx` | "unwrap_or, unwrap_or_else and expect!" |
| B8        | `library/std/option_result/tests/option_test.rx` | "map chains on Some and skips on nil" |

---

## Out of scope (v2)

- `Result.map_err`, `Result.and_then`, `Result.or_else` — wired
  in the typeck/MIR layer but only `map` has a dedicated pin.
- `Option.and_then` / `Option.or_else` / `Option.flatten`.
- `Try` mixin that user types can implement (the `?` operator is
  hard-wired to `Option` and `Result` in v1).
