# Spec — `std.io`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.2](../../requirements/tier1_01_stdlib.md),
[docs/prompts/v1/06_phase2_stdlib_io_fmt.md](../../prompts/v1/06_phase2_stdlib_io_fmt.md).

**Status:** shipped Phase 2 #06.1 (2026-05-09) + #06.5 IoError promotion (2026-05-13).

`std.io` provides line-oriented stdin reading and stdout/stderr
writing.  Errors are surfaced via the `IoError` tagged enum.

---

## B1 — `Stdout.println(s)` writes text plus `\n`

**Given** a program calling `Stdout.new().println("hello")`
**When** the program runs
**Then** stdout contains `"hello\n"` exactly (no buffering surprises;
the runtime flushes line-buffered streams at process exit).

## B2 — `Stdout.print(s)` writes text without newline

**Given** `Stdout.new().print("ab")` followed by `print("cd")`
**Then** stdout contains `"abcd"` with no intervening newline.

## B3 — `Stderr.eprintln(s)` routes to stderr, not stdout

**Given** a program calling `Stderr.new().eprintln("warn")`
**Then** stdout is empty; stderr ends with `"warn\n"`.

## B4 — `Stderr.eprint(s)` writes without newline to stderr

Same as B3 but no trailing newline.

## B5 — `Stdout.write_str(s)` emits exact bytes

**Given** `Stdout.new().write_str("raw")`
**Then** stdout contains exactly `"raw"` (no newline).  Returns
`Result[(), IoError]`.

## B6 — `Stderr.write_str` routes to stderr

Mirror of B5 for stderr.

## B7 — `Stdout.flush()` and `Stderr.flush()` return `Ok(())`

`flush` is callable and returns `Result[(), IoError]`.  On the v1
runtime it never fails (no buffered writer) — always `Ok(())`.

## B8 — `Stdin.lines()` yields each line

**Given** stdin contains `"a\nb\nc\n"`
**When** the program iterates `Stdin.new().lines()`
**Then** the iterator yields three `Result.Ok` items with payload
`"a"`, `"b"`, `"c"` (line terminators stripped).

**Simplification (v1):** `lines()` returns `Array[Result[String, IoError]]`
rather than Rust's `BufRead` iterator — the file is read fully into
memory first.  Documented in prompt-06.

## B9 — `Stdin.lines()` handles trailing newline + partial final line

**Given** stdin `"a\nb"` (no trailing newline)
**Then** `lines()` yields `["a", "b"]` (the partial final line is
emitted, not dropped).

**Given** stdin `"a\nb\n"` (trailing newline)
**Then** `lines()` yields `["a", "b"]` (no trailing empty item).

## B10 — `Stdin.lines()` on empty input yields empty Array

**Given** stdin is empty
**Then** `lines()` returns an empty `Array`, not an error.

## B11 — `IoError` is constructible and `.message()` dispatches per variant

`IoError` is a tagged enum with variants `NotFound`, `PermissionDenied`,
`Interrupted`, `UnexpectedEof`, and `Other(String)`.  Each variant
exposes `.message() -> String`.

**Given** `let e = IoError.NotFound`
**When** evaluating `e.message()`
**Then** the result is a stable human-readable string (currently
`"entity not found"`).

**Given** `let e = IoError.Other(String.from("disk full"))`
**Then** `e.message()` is `"disk full"`.

## B12 — `IoError.message()` is reachable through `Result.Err` payloads

Programs can pattern-match `Result.Err(io_err)` returned by
`Stdin.read_to_string`, `Stdout.write_str`, etc., and call
`.message()` on the bound name.  Inference for `let Err(e) = ... ;
e.message()` chains is currently incomplete (deferred).

---

## Pin tests

| Behaviour | Test fn                                                | File             |
|-----------|--------------------------------------------------------|------------------|
| B1        | `stdout_println_emits_text_plus_newline`               | `stdlib_io.rs`   |
| B2        | `stdout_print_emits_text_without_newline`              | `stdlib_io.rs`   |
| B3        | `stderr_eprintln_routes_to_stderr_only`                | `stdlib_io.rs`   |
| B4        | `stderr_eprint_no_newline`                             | `stdlib_io.rs`   |
| B5        | `stdout_write_str_emits_exact_bytes`                   | `stdlib_io.rs`   |
| B6        | `stderr_write_str_routes_to_stderr`                    | `stdlib_io.rs`   |
| B7        | `stdout_flush_returns_ok`                              | `stdlib_io.rs`   |
| B8        | `stdin_lines_yields_each_line`                         | `stdlib_io.rs`   |
| B9        | `stdin_lines_no_trailing_empty_and_partial_final_line` | `stdlib_io.rs`   |
| B10       | `stdin_lines_empty_input_yields_empty_vec`             | `stdlib_io.rs`   |
| B11       | `io_error_variants_are_constructible_and_message_dispatches` | `stdlib_io.rs` |
| B11/B12   | `io_error_message_dispatches_per_variant_on_empty_stdin` | `stdlib_io.rs` |

---

## Out of scope (v2)

- `BufRead` iterator (true streaming line reader); v1 buffers the
  whole input.
- The `Err(e).message()` inference chain inside `if let Err(e) = ...`
  arms (separately tracked).

> **Note:** `Stdin.read_to_string()` is pinned by
> `std_io_stdin_read_to_string_round_trip` in
> `crates/riven-core/tests/std_use_resolution.rs` — the round-trip
> test was previously hidden behind a sibling-file naming convention.
