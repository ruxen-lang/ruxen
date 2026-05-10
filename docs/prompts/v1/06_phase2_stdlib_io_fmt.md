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
      path. Still pending: full `std::io` (`Stdin`/`Stdout`/`Stderr`
      surface + `IoError` enum), `std::fmt` (`Display` trait +
      `Formatter`), `std::process::Command` builder. `fs::metadata`
      deferred — needs a struct surface to expose size / kind / mtime;
      the boolean helpers (`is_file`, `is_dir`) plus the existing
      `exists` cover the v1 minimum.
- [ ] String interpolation routes through `Display::fmt`.
- [ ] `Debug` interpolation `"#{x:?}"` works for any `derive Debug`
      type.
- [ ] CI green.
- [ ] CHANGELOG bullet. **Partial:** env/fs first-batch entry shipped.
