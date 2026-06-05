# Spec — `std.fmt`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.8](../../requirements/tier1_01_stdlib.md),
[docs/prompts/v1/06_phase2_stdlib_io_fmt.md](../../prompts/v1/06_phase2_stdlib_io_fmt.md).

**Status:** shipped Phase A/B/C/D MVP (2026-05-10) → Phase D2 (2026-05-13) → Phase D4 (2026-05-13).

This spec describes the **observable behaviour** of `std.fmt` — the
`Display` and `Debug` mixins, the `Formatter` class, the string-
interpolation lowering, and the format-spec surface (`width`, `align`,
`fill`, `precision`, `:?`).  Each behaviour is numbered and pinned by
one or more Rust integration tests in `crates/ruxen-core/tests/`.

---

## B1 — `Display` mixin surface

`std.fmt.Display` is resolvable at the type level with method
signature `def fmt(f: &var Formatter) -> Result[nil, FmtError]`.

**Given** a user type `T` with `include Display` in its body
**When** `T` is used in an interpolation `"#{t}"`
**Then** the lowerer emits a call to `T_fmt(value, fmt)` and uses the
formatter's accumulated buffer as the interpolated text.

## B2 — `Debug` mixin surface

`std.fmt.Debug` is resolvable with the same signature as `Display`.
The implicit `include Debug` on a struct or enum synthesises a
`T_to_debug(self) -> String` MIR function whose output shape matches
Rust's derived Debug (`Name { field1: <fmt(field1)>, ... }` for
structs; `Variant(<payload>)` / `Variant { .. }` for enums).

**Given** `struct Point` with fields `x: Int`, `y: Int` (implicit `Debug`)
**When** `"#{p:?}"` is evaluated for `p = Point { x: 1, y: 2 }`
**Then** the output is `Point { x: 1, y: 2 }`.

## B3 — `Formatter` runtime surface

`Formatter` is a built-in class with a heap-owned grow-able byte buffer.

| Method                                          | Returns                  |
|-------------------------------------------------|--------------------------|
| `Formatter.new()`                               | `Formatter`              |
| `f.write_str(s: &str)`                          | `Result[nil, FmtError]`   |
| `f.write_char(c: Char)`                         | `Result[nil, FmtError]`   |
| `f.buffer()`                                    | `String` (consumes `f`)  |
| `f.size()`                                      | `Int`                    |
| `f.width()` / `precision()` / `align()` / `fill()` | accessors (read-only)  |

**Invariant:** `f.buffer()` transfers ownership of the accumulated bytes
to the returned String and frees the Formatter struct in the same call.
Codegen must not emit a follow-up `Formatter_free` on `f` after
`buffer()`.

## B4 — Bare interpolation routes through `Display.fmt`

For every interpolated expression `"#{x}"` whose type is `Char`, `Int`,
`Float`, `Bool`, `String`, or any type `T` with `include Display` in its body,
`lower_interpolation` emits exactly:

```text
fmt = Formatter_new()              # or _new_with_spec(...)  see B6
{T}_fmt(value, fmt)
buf = Formatter_buffer(fmt)
```

The legacy ad-hoc `ruxen_<prim>_to_string` direct call at the
interp site is gone (it still lives inside the synth `_fmt` body — a
different MIR function).

## B5 — Implicit-Debug fallback for bare `"#{x}"`

When `x: T` where `T` has implicit `Debug` but does **not** have
`include Display`, bare `"#{x}"` lowers to a direct call to
`{T}_to_debug(x)` (no Formatter involved).  This is a fallback that
will be removed once every implicit-Debug type also gets a generated
`Display` include (deferred to v2).

## B6 — Format spec syntax + lex-time capture

`"#{x:<spec>}"` accepts an optional spec.  The lexer captures:

| Position    | Form         | Example          | Field                |
|-------------|--------------|------------------|----------------------|
| `fill+align`| `<c>[<>^]`   | `*<`             | `fill`, `align`      |
| `align`     | `[<>^]`      | `>`              | `align`              |
| `width`     | digits       | `10`             | `width`              |
| `.precision`| `.` + digits | `.2`             | `precision`          |
| `?` debug   | literal `?`  | `?`              | `debug = true`       |

Trailing whitespace inside `{ ... }` is tolerated.  Any other character
emits diagnostic `E0007 malformed format spec`.

## B7 — Width / align / fill applied at runtime

When `FormatSpec.width > 0`, the formatter pads the accumulated buffer
to exactly `width` bytes using `fill` as the pad character (default
space) and `align` as the side:

| align     | Pad side                              |
|-----------|---------------------------------------|
| `<` left  | pad on the right                      |
| `>` right | pad on the left (also the default)    |
| `^` center| split; extra char goes on the right   |

**Given** `n: Int = 42`
**When** evaluating `"[#{n:>5}]"`
**Then** the result is `"[   42]"`.

**Given** `n: Int = 7`
**When** evaluating `"[#{n:*<5}]"`
**Then** the result is `"[7****]"`.

Non-ASCII fill codepoints fall back to space (v1 simplification).

## B8 — `precision` applied at runtime

Precision is type-specific:

| Type     | Effect                                                   |
|----------|----------------------------------------------------------|
| `Float`  | decimal digits via `snprintf("%.*f", prec, value)`       |
| `String` | UTF-8 char-count truncate (boundary-safe)                |
| `Int`    | ignored (matches Rust)                                   |
| `Bool`   | ignored                                                  |
| `Char`   | ignored                                                  |

**Given** `pi: Float = 3.14159`
**When** evaluating `"#{pi:.2}"`
**Then** the result is `"3.14"`.

**Given** `s: String = "hello world"`
**When** evaluating `"[#{s:.5}]"`
**Then** the result is `"[hello]"`.

## B9 — Width + precision compose

Width and precision compose: precision is applied first (shortening the
value's string form), then width pads the result.

**Given** `pi: Float = 3.14159`
**When** evaluating `"[#{pi:>8.2}]"`
**Then** the result is `"[    3.14]"`.

## B10 — Non-default spec on `String` does not short-circuit

When a `String` value carries a non-default spec, the legacy
"string-like pass-through" is skipped and the value is routed through
`String_fmt` so width / precision / align / fill all apply.

## B11 — `Formatter.buffer()` ownership transfer

After `Formatter.buffer()`, the `Formatter` is consumed.  Codegen does
not emit a follow-up `Formatter_free`; the runtime frees the struct
inside `_buffer` itself.

---

## Pin tests

| Behaviour | Test fn                                                          | File                                  |
|-----------|------------------------------------------------------------------|---------------------------------------|
| B1        | `display_trait_and_formatter_are_resolvable`                     | `stdlib_fmt.rs`                       |
| B1        | `user_impl_display_lowers_t_fmt_function`                        | `stdlib_fmt_display_dispatch.rs`      |
| B1        | `interpolation_user_impl_display_money_round_trips`              | `stdlib_fmt_runtime.rs`               |
| B2        | `debug_trait_is_resolvable_with_fmt_method`                      | `stdlib_fmt.rs`                       |
| B2        | `debug_interpolation_spec_typechecks`                            | `stdlib_fmt.rs`                       |
| B2        | implicit-Debug E2E fixtures `85_implicit_debug.rx` etc.         | `tests/release-e2e/cases/`            |
| B3        | `formatter_write_str_then_buffer_round_trips`                    | `stdlib_fmt_runtime.rs`               |
| B3        | `formatter_write_char_ascii_round_trips`                         | `stdlib_fmt_runtime.rs`               |
| B3        | `formatter_len_after_write_str`                                  | `stdlib_fmt_runtime.rs`               |
| B3        | `formatter_write_str_returns_result_unit_fmt_error`              | `stdlib_fmt.rs`                       |
| B4        | `synth_primitive_fmt_functions_emitted`                          | `stdlib_fmt_display_dispatch.rs`      |
| B4        | `interpolation_primitive_goes_through_display`                   | `stdlib_fmt_display_dispatch.rs`      |
| B5        | implicit-Debug fixtures + existing interp E2E in `05_string_interp.rx` | `tests/release-e2e/cases/`      |
| B6        | `lex_format_spec_*` (8 tests)                                    | `crates/ruxen-core/src/lexer/tests.rs`|
| B6        | `width_and_precision_specs_typecheck`                            | `stdlib_fmt.rs`                       |
| B7        | `interpolation_width_right_align_pads_int`                       | `stdlib_fmt_runtime.rs`               |
| B7        | `interpolation_width_left_align_pads_int`                        | `stdlib_fmt_runtime.rs`               |
| B7        | `interpolation_width_center_align_pads_int`                      | `stdlib_fmt_runtime.rs`               |
| B7        | `interpolation_fill_char_left_align`                             | `stdlib_fmt_runtime.rs`               |
| B8        | `interpolation_float_precision`                                  | `stdlib_fmt_runtime.rs`               |
| B8        | `interpolation_string_precision_truncates`                       | `stdlib_fmt_runtime.rs`               |
| B9        | `interpolation_width_and_precision_compose`                      | `stdlib_fmt_runtime.rs`               |
| B10       | covered transitively by B7 / B8 / B9                             |                                       |
| B11       | covered transitively by B3 + every interp test                   |                                       |

E2E coverage: `tests/release-e2e/cases/070_interp_display_dispatch.rx`
(B1 + B4) and `071_interp_format_specs.rx` (B7 + B8 + B9).

---

## Out of scope (v2)

- Width on `"#{x:?}"` debug spec — the Debug path bypasses the
  Formatter and goes through `{T}_to_debug` directly, so width is lost.
- Sign / `#` alternate / `0` zero-pad / radix flags (`x`, `X`, `b`,
  `o`, `e`) — not yet parsed in `lex_format_spec`.
- Blanket `Display` includes for `Array` / `Hash` / `Set` / `Option` /
  `Result` / tuples / arrays — only primitives + user types covered.
- The `Err(e).message()` inference gap inside `include Display` bodies
  on Result-returning expressions (separately tracked).
