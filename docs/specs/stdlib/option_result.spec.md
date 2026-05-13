# Spec — `Option[T]` / `Result[T, E]`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.4–5.5](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 2 #02-#03; integration via `option_result_runtime.rs`.

Riven's `Option` and `Result` mirror Rust's modulo a few v1
simplifications.  Both are tagged enums whose payload reaches the
runtime as `int64_t`s (tag in the low half, payload pointer / scalar
in the high half) so they pass cleanly through the C ABI.

---

## B1 — `Option::Some(x)` / `Option::None` round-trip

Programs can construct, pattern-match, and destructure `Option`
values across function boundaries.  Round-trip:
`construct → match → extract → reuse` produces the original value.

## B2 — `Result::Ok(x)` / `Result::Err(e)` round-trip

Same as B1 for `Result`.

## B3 — `?` operator on `Result`

**Given** a fn returning `Result[T, E]` and a sub-call
`x?` where `x: Result[T, E]`
**When** `x = Err(e)`
**Then** the enclosing function returns `Err(e)` immediately; `T` is
bound to the unwrapped value on the `Ok` path.

## B4 — `if let Some(x) = opt` pattern matching

`if let Some(x) = opt { ... } else { ... }` binds `x` to the payload
on the `Some` arm; the `else` arm has no binding.

## B5 — Generic enum usage with payload

**Given** a user enum `enum Result2[T]` parameterised over `T`
**Then** monomorphisation routes the right payload type per
instantiation.

## B6 — `.expect!(msg)` panics on `None` / `Err` with `msg`

`expect!` is the v1 name for Rust's `expect` (the `!` marks it as
panicking).  On `Some(x)` / `Ok(x)` it returns `x`; on `None` /
`Err(_)` it panics with `msg`.

## B7 — `.unwrap_or(default)` falls back

Returns the contained value on `Some(x)` / `Ok(x)`; returns
`default` otherwise.

## B8 — `.map(|x| f(x))` transforms the payload

`Option::map` and `Result::map` apply `f` to the contained value when
present and pass through `None` / `Err` unchanged.

---

## Pin tests

All B1–B8 are covered by the integration test
`crates/riven-core/tests/option_result_runtime.rs` which compiles +
runs E2E fixtures:

| Behaviour | E2E fixture                                | Pin fn                  |
|-----------|--------------------------------------------|-------------------------|
| B1        | `23_option.rvn`                            | `e2e_23_option`         |
| B2        | `24_result.rvn`                            | `e2e_24_result`         |
| B3        | `25_question_op.rvn`                       | `e2e_25_question_op`    |
| B4        | `56_if_let_some.rvn`                       | `e2e_56_if_let_some`    |
| B5        | `73_enum_generic.rvn`                      | `e2e_73_enum_generic`   |
| B5        | `19_enums_data.rvn` + `18_enums_simple.rvn`| `e2e_19_enums_data` + `e2e_18_enums_simple` |
| B6        | `97_expect_ok.rvn`                         | `e2e_97_expect_ok`      |
| B7        | `98_unwrap_or.rvn`                         | `e2e_98_unwrap_or`      |
| B8        | `99_map_option.rvn`                        | `e2e_99_map_option`     |

---

## Out of scope (v2)

- `Result::map_err`, `Result::and_then`, `Result::or_else` — wired
  in the typeck/MIR layer but only `map` has a dedicated pin.
- `Option::and_then` / `Option::or_else` / `Option::flatten`.
- `Try` trait that user types can implement (the `?` operator is
  hard-wired to `Option` and `Result` in v1).
- `Result[T, !]` (`Infallible` payload type).
