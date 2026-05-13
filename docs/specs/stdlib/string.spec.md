# Spec — `String` / `&str`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §5.6](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 2 #03-#04; ownership negatives Phase 2 #02.

Riven's `String` is a heap-owned UTF-8 buffer; `&str` is a borrowed
view.  Both share the same runtime representation at the FFI layer
(`char*`).  Ownership and use-after-move are enforced statically by
the borrow checker.

---

## B1 — `String.from(s)` accepts `&str`

**Given** `s: &str`
**Then** `String.from(s)` returns an owned `String`.

**Given** `String.from(1)` (Int arg, not a string)
**Then** typeck handles it via the int-to-string convenience path
(documented as "currently lenient" — v2 may tighten).

## B2 — Use-after-move on `String` argument is rejected

**Given** a function `f(s: String)` that takes ownership and a caller
`let x = String.from("hi"); f(x); f(x);`
**Then** the second `f(x)` emits a use-after-move diagnostic from the
borrow checker.

## B3 — `into_bytes()` consumes the receiver

**Given** `let s = String.from("hi"); let _ = s.into_bytes(); let _ = s;`
**Then** the final `s` use is rejected: `into_bytes` consumed it.

## B4 — Existing method surface (pre-#06)

The string surface includes:
- `len`, `is_empty`, `chars`, `bytes`, `as_bytes`
- `to_uppercase`, `to_lowercase`, `trim`, `trim_start`, `trim_end`
- `split(sep)`, `split_whitespace`, `lines`
- `starts_with`, `ends_with`, `contains`, `find`, `rfind`
- `replace(from, to)`
- `clone`, `concat` (`+`)
- `parse[T]() -> Result[T, ParseError]` for primitive `T`
- `truncate(n)` (in-place; byte-count)

Pin tests for these live in the E2E fixture set
(`106_string_chars.rvn`, `108_string_split.rvn`,
`112_string_trim.rvn`, `113_string_methods_chain.rvn`,
`05_string_interp.rvn`, `114_string_interp_mixed.rvn`).

---

## Pin tests

| Behaviour | Test fn                                       | File                            |
|-----------|-----------------------------------------------|---------------------------------|
| B1        | `string_from_with_int_arg_is_handled`         | `stdlib_string_negatives.rs`    |
| B2        | `use_after_move_on_string_argument_is_rejected` | `stdlib_string_negatives.rs`  |
| B3        | `use_after_into_bytes_is_rejected`            | `stdlib_string_negatives.rs`    |
| B4        | covered by E2E fixtures                       | `tests/release-e2e/cases/`      |

---

## Out of scope (v2)

- `String` interning / `&'static str` literal table.
- Grapheme clusters (`chars()` returns Unicode scalar values, not
  grapheme clusters).
- Regex (separate `std::regex` module, deferred).
- `Cow[str]` — v1 chooses between `String` and `&str` at the source
  level; no copy-on-write wrapper.
