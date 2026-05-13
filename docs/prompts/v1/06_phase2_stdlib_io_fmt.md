# 06 — Phase 2 stdlib: `io` + `fmt` + `process` + `env` + `fs`

**Depends on:** prompts 02-05.
**Reads:** `docs/requirements/tier1_01_stdlib.md` §io, §fmt, §process.

## Surface

### `std::io`
- `Stdin`, `Stdout`, `Stderr` already partially wired. Complete:
- `Stdin::read_line -> Result[String, IoError]`,
  `read_to_string -> Result[String, IoError]`,
  `lines -> Iterator[Result[String, IoError]]`.
- `Stdout::write_str(&str)`, `flush -> Result[(), IoError]`,
  `print(&str)`, `println(&str)`.
- `Stderr::write_str`, `flush`, `eprint`, `eprintln`.
- `IoError` enum: `NotFound`, `PermissionDenied`, `Interrupted`,
  `UnexpectedEof`, `Other(String)`. Each carries a message.

### `std::fmt`
- `trait Display { def fmt(self: &Self, f: &mut Formatter) -> Result[(), fmt::Error] }`.
- `trait Debug` (already partially via derive — wire formal trait).
- `Formatter` carries width, alignment, precision flags.
- String interpolation `"#{x}"` calls `Display::fmt` (currently
  ad-hoc — make it route through trait).
- `Debug` interpolation via `"#{x:?}"` syntax.

### `std::env`
- `args() -> Vec[String]` (already exists; verify).
- `var(&str) -> Result[String, VarError]`.
- `vars() -> HashMap[String, String]`.
- `current_dir -> Result[String, IoError]`.

### `std::fs`
- `read_to_string(&str) -> Result[String, IoError]` (already wired;
  audit).
- `write(&str, &str) -> Result[(), IoError]`.
- `read_dir(&str) -> Result[Vec[String], IoError]`.
- `metadata`, `exists`, `is_file`, `is_dir`.

### `std::process`
- `exit(Int) -> !`.
- `Command::new(&str)`, `.arg(&str)`, `.args(I)`, `.env(K,V)`,
  `.spawn -> Child`, `.output -> Result[Output, IoError]`,
  `.status -> Result[ExitStatus, IoError]`.

## TDD

- Unit tests in `crates/riven-core/tests/stdlib_io.rs`,
  `stdlib_fmt.rs`, `stdlib_env.rs`, `stdlib_fs.rs`,
  `stdlib_process.rs`.
- E2E fixtures cover: read stdin (use a piped fixture), write
  stdout, format a struct via Display, env var lookup, fs round-trip.
- Negative tests: nonexistent file → `IoError::NotFound`; non-UTF8
  → meaningful error.

## Implementation

- Most fns already exist as runtime stubs in `runtime.c`; complete
  their semantics.
- `Display` becomes the canonical interpolation trait. Existing
  ad-hoc `to_string` calls in interpolation lowering must route
  through `Display::fmt`. This is a refactor — keep tests green
  every step.
- `Command::spawn` shells out via `posix_spawn` or `fork+execvp`
  on Unix; Windows skipped per `tier4_04` until Phase 4.
- `IoError` is an enum that survives FFI: define stable repr.

## Definition of done

- [ ] Every listed function has a positive + negative test.
      **Partial:** `std::env` (`vars`, `current_dir`) and `std::fs`
      (`is_file`, `is_dir`, `read_dir`) shipped as a first batch with
      positive integration tests in `crates/riven-core/tests/stdlib_env.rs`
      + `stdlib_fs.rs` and a negative test for `read_dir` on a missing
      path. **`std::io` surface shipped (#06.2):** `Stdin.read_line` /
      `read_to_string` / `lines` (Vec[Result[String, IoError]] —
      v1 simplification of Rust's BufRead iterator); `Stdout.write_str`
      / `flush` / `print` / `println`; `Stderr.write_str` / `flush` /
      `eprint` / `eprintln`. Tests in `crates/riven-core/tests/stdlib_io.rs`
      cover positive paths for every method + Stdin.lines edge cases
      (trailing newline, partial final line, empty input). Still
      pending: `std::fmt` (`Display` trait + `Formatter`),
      `std::process::Command` builder. `fs::metadata` deferred — needs
      a struct surface to expose size / kind / mtime; the boolean
      helpers (`is_file`, `is_dir`) plus the existing `exists` cover
      the v1 minimum. **`IoError` shipped as message-only**
      (`riven_io_error_message` wraps strerror in `Result::Err`);
      tagged-variant matching (`NotFound` / `PermissionDenied` /
      `Interrupted` / `UnexpectedEof` / `Other`) deferred to v2 —
      requires changing the FFI repr of `Result::Err(IoError)` from
      `char*` to a heap struct `{u32 tag; char* msg}` and updating
      27 callsites.
- [x] String interpolation routes through `Display::fmt`.
      **Phase D2 (2026-05-13):** `lower_interpolation` emits the
      canonical `Formatter_new` → `{T}_fmt(value, fmt)` →
      `Formatter_buffer` sequence for primitives (Char / Int /
      Float / Bool) and for any user type with `impl Display for T`.
      Synth `Char_fmt` / `Int_fmt` / `Float_fmt` / `Bool_fmt` /
      `String_fmt` MIR fns wrap the existing `riven_*_to_string`
      runtime helpers so observable output is byte-identical to the
      legacy ad-hoc switch. Derive-Debug-only types still fall back
      to `{Name}_to_debug` for the bare `"#{x}"` form until users
      provide their own `impl Display`. Width / align / precision /
      fill runtime application (D4) and the `Err(e).message()`
      inference gap remain separately tracked. Commits: S0
      24b84c3 → S1 2683c6d → S2 47250b1 / fd24313 → S3 eb297e2 →
      S4 a1658e9.
- [x] `Debug` interpolation `"#{x:?}"` works for any `derive Debug`
      type. **Phase B + C MVP:** format spec captured at lex time
      (FormatSpec.debug = true), threaded through HIR/MIR. Existing
      `_to_debug` synthesis on derive-Debug structs already produces
      the expected output for `"#{x:?}"`; the bare `"#{x}"` form
      currently uses the same path (Phase D will switch bare to
      Display::fmt). Pin tests in `stdlib_fmt.rs::debug_interpolation_spec_typechecks`.
- [ ] CI green.
- [ ] CHANGELOG bullet. **Partial:** env/fs first-batch + std::io
      surface entries shipped + std::fmt foundation A/B/C/D-MVP
      shipped.
