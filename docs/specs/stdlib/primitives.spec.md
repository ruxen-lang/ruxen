# Spec — Primitive types method surface

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.7](../../requirements/tier1_01_stdlib.md).

**Status:** shipped across Phase 1-2; numeric-literal suffixes since
Phase 1.

Ruxen primitives (`Int`, sized `Int8..64` / `UInt8..64`, `ISize`,
`USize`, `Float`, `Float32`, `Float64`, `Bool`, `Char`) have small,
type-stable method surfaces.  This spec lists the methods that the
typechecker resolves and the runtime helpers that back them.

---

## B1 — `Int.to_string() -> String`

`Int` exposes `to_string()` returning `String`.  Runtime backed by
`ruxen_int_to_string`.

**Given** `let n: Int = 42`
**When** `n.to_string()` is evaluated
**Then** the result is `"42"`.

Same surface holds for `USize` (the type sometimes inferred for array
sizes and `Array.len`).

## B2 — `Float.to_string() -> String`

`Float` (`= Float64`) exposes `to_string()`; runtime `ruxen_float_to_string`
uses `%g` formatting.

**Given** `let f: Float = 3.14`
**Then** `f.to_string()` is `"3.14"`.

The D4 surface (chapter 17) adds `Float_to_string_prec(value, prec)`
for the `:.N` format spec — see [fmt.spec.md](fmt.spec.md) B8.

## B3 — `Bool.to_string() -> String`

Returns `"true"` or `"false"` via `ruxen_bool_to_string`.

## B4 — `Char` interpolation produces a UTF-8 single-codepoint string

`Char` does not currently expose `to_string()` as a typeck method
(unlike `Int` / `Float` / `Bool` / `USize`).  Conversion happens via
string interpolation `"#{c}"` which routes through `Char_fmt` →
`ruxen_char_to_string` and handles all four UTF-8 code-unit widths
(1-4 bytes).

**Given** `let h: Char = '\u{6C34}'`
**Then** `"#{h}"` is the string `"水"` (3 bytes).

A direct `to_string()` method is a small follow-up; the runtime
helper already exists.

## B5 — Numeric literal suffixes type-check

The lexer recognises suffixes `i8 i16 i32 i64 u u8 u16 u32 u64
isize usize f32 f64`; the typechecker propagates the resulting
sized type.

**Given** `let x = 123u8`
**Then** `x` has type `UInt8`.

**Given** `let pi = 3.14f32`
**Then** `pi` has type `Float32`.

## B6 — Numeric literal forms

Integer literals accept three notations:

| Form           | Example       | Notes                          |
|----------------|---------------|--------------------------------|
| Decimal        | `1_234_567`   | Underscores allowed as separators |
| Hexadecimal    | `0x1F`        | Lower- or upper-case `0X` prefix accepted |
| Binary         | `0b101`       | Lower- or upper-case `0B` prefix accepted |

Float literals accept scientific notation: `1.5e3`, `2.0E-4`.

## B7 — Numeric-suffix-less inference

A bare literal `42` has type `Int` (= `Int64`).  A bare float `3.14`
has type `Float` (= `Float64`).  Suffix-less literals participate in
inference for the surrounding context: `let x: UInt8 = 200` types
fine (literal narrows on assignment).

## B8 — Char literal escape sequences

| Escape   | Codepoint        |
|----------|------------------|
| `?\n`   | `0x0A`           |
| `'\r'`   | `0x0D`           |
| `?\t`   | `0x09`           |
| `'\\'`   | `0x5C`           |
| `'\''`   | `0x27`           |
| `'\"'`   | `0x22`           |
| `'\0'`   | `0x00`           |
| `'\u{1F600}'` | Unicode (any codepoint up to U+10FFFF) |

---

## Pin tests

| Behaviour | Test fixture / fn                                          | File                                           |
|-----------|------------------------------------------------------------|------------------------------------------------|
| B1        | `e2e_02_int_arith.rx` + many interpolation fixtures       | `tests/release-e2e/cases/`                     |
| B2        | `e2e_03_float_arith.rx` + `interpolation_float_precision` | `stdlib_fmt_runtime.rs`                        |
| B3        | `e2e_04_bool_logic.rx`                                    | `tests/release-e2e/cases/`                     |
| B4        | `prim_char_unicode_escapes_round_trip`                     | `stdlib_primitives.rs`                         |
| B5        | `prim_numeric_suffix_int_widths_round_trip`                | `stdlib_primitives.rs`                         |
| B6        | `prim_numeric_suffix_int_widths_round_trip` + `prim_float_scientific_notation` | `stdlib_primitives.rs`     |
| B7        | covered transitively by literal-typed assignments throughout the fixture set | |
| B8        | `prim_char_escape_sequences_round_trip` + `prim_char_unicode_escapes_round_trip` | `stdlib_primitives.rs`   |

---

## Gaps

- B4: `Char.to_string()` typeck method is missing (the runtime
  helper exists; spec correction noted above).  Adding it is a
  small typeck wiring follow-up.
- B5/B6: a negative pin asserting `let x: UInt8 = 300` is rejected
  at typeck (overflow) — currently covered via the borrow check
  unit suite but not at the integration boundary.

## Out of scope (v2)

- Mixin-bound numeric primitives — Rust's `Num`, `One`, `Zero` mixins
  don't exist in v1; arithmetic is hardwired.
- 128-bit integers (`Int128`, `UInt128`) and arbitrary-precision
  big-int.
- Float NaN / Infinity literal syntax (`Float.NAN`, `Float.INFINITY`).
- `Char.from_u32` / `Char.to_digit` helpers — only `to_string` ships
  today.
- Bit-manipulation methods (`leading_zeros`, `count_ones`, etc.).
