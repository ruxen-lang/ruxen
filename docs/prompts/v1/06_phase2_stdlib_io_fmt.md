# 06 — Phase 2 stdlib: `io` + `fmt` + `process` + `env` + `fs`

**Depends on:** prompts 02-05.
**Reads:** `docs/requirements/tier1_01_stdlib.md` §io, §fmt, §process.

## Surface

### `std.io`
- `Stdin`, `Stdout`, `Stderr` already partially wired. Complete:
- `Stdin.read_line -> Result[String, IoError]`,
  `read_to_string -> Result[String, IoError]`,
  `lines -> Iterator[Result[String, IoError]]`.
- `Stdout.write_str(&str)`, `flush -> Result[(), IoError]`,
  `print(&str)`, `println(&str)`.
- `Stderr.write_str`, `flush`, `eprint`, `eprintln`.
- `IoError` enum: `NotFound`, `PermissionDenied`, `Interrupted`,
  `UnexpectedEof`, `Other(String)`. Each carries a message.

### `std.fmt`
- `mixin Display
    def fmt(f: &var Formatter) -> Result[(), fmt.Error]
  end`.
- `mixin Debug` (already partially via implicit include — wire formal mixin).
- `Formatter` carries width, alignment, precision flags.
- String interpolation `"#{x}"` calls `Display.fmt` (currently
  ad-hoc — make it route through the mixin method).
- `Debug` interpolation via `"#{x:?}"` syntax.

### `std.env`
- `args() -> Array[String]` (already exists; verify).
- `var(&str) -> Result[String, VarError]`.
- `vars() -> Map[String, String]`.
- `current_dir -> Result[String, IoError]`.

### `std.fs`
- `read_to_string(&str) -> Result[String, IoError]` (already wired;
  audit).
- `write(&str, &str) -> Result[(), IoError]`.
- `read_dir(&str) -> Result[Array[String], IoError]`.
- `metadata`, `exists`, `is_file`, `is_dir`.

### `std.process`
- `exit(Int) -> !`.
- `Command.new(&str)`, `.arg(&str)`, `.args(I)`, `.env(K,V)`,
  `.spawn -> Child`, `.output -> Result[Output, IoError]`,
  `.status -> Result[ExitStatus, IoError]`.

## TDD

- Unit tests in `crates/riven-core/tests/stdlib_io.rs`,
  `stdlib_fmt.rs`, `stdlib_env.rs`, `stdlib_fs.rs`,
  `stdlib_process.rs`.
- E2E fixtures cover: read stdin (use a piped fixture), write
  stdout, format a struct via Display, env var lookup, fs round-trip.
- Negative tests: nonexistent file → `IoError.NotFound`; non-UTF8
  → meaningful error.

## Implementation

- Most fns already exist as runtime stubs in `runtime.c`; complete
  their semantics.
- `Display` becomes the canonical interpolation mixin. Existing
  ad-hoc `to_string` calls in interpolation lowering must route
  through `Display.fmt`. This is a refactor — keep tests green
  every step.
- `Command.spawn` shells out via `posix_spawn` or `fork+execvp`
  on Unix; Windows skipped per `tier4_04` until Phase 4.
- `IoError` is an enum that survives FFI: define stable repr.

## Definition of done

- [ ] Every listed function has a positive + negative test.

      **SHIPPED:**
      - `std.env`: `args`, `var`, `vars`, `current_dir` (positive tests
        in `crates/riven-core/tests/stdlib_env.rs`).
      - `std.fs`: `read_to_string`, `write`, `read_dir`, `exists`,
        `is_file`, `is_dir` (`stdlib_fs.rs` — incl. negative for
        `read_dir` on missing path).
      - `std.io`: `Stdin.{read_line, read_to_string, lines}`,
        `Stdout.{write_str, flush, print, println}`,
        `Stderr.{write_str, flush, eprint, eprintln}` (12 tests in
        `stdlib_io.rs`, including Stdin.lines edge cases — trailing
        newline, partial final line, empty input).
      - `std.fmt`: `Display` mixin canonical dispatch (Char / Int /
        Float / Bool / String / user-`include Display` types route
        through `Formatter_new` → `{T}_fmt` → `Formatter_buffer`);
        width / align / precision / fill applied at runtime via
        `Formatter_new_with_spec`. `Debug` interpolation `"#{x:?}"`
        on implicit-Debug structs.
      - `std.process`: flat `process_run(cmd, args)` + `process_exit`
        (7 tests in `stdlib_process.rs`).
      - `IoError`: message-only payload (`riven_io_error_message`
        wraps strerror in `Result.Err`).

      **REMAINING v1 work:**
      - `std.process.Command` builder API (`.new/.arg/.args/.env/
        .current_dir/.status/.output`) plus `Output` and `ExitStatus`
        structs. Currently only the flat `process_run` shortcut
        exists.
      - `fs.metadata(path) -> Result[Metadata, IoError]` returning a
        flat `Metadata` struct (size / is_file / is_dir / is_symlink /
        modified). Per the requirements doc the struct must be
        ABI-stable C-side; `stat` is called inside C and packed
        without exposing the libc struct directly.
      - Fill missing negative tests across env/fs/io once the above
        ship (most negative paths already covered by `read_dir` and
        IoError construct tests; sweep for any positive-only
        functions before closing).

      **DEFERRED TO v2:**
      - Tagged `IoError` variant matching (`NotFound` /
        `PermissionDenied` / `Interrupted` / `UnexpectedEof` /
        `Other`) — requires changing FFI repr of
        `Result.Err(IoError)` from `char*` to heap struct
        `{u32 tag; char* msg}` and updating 27 callsites.
      - `Command.spawn -> Child` async-style handle with
        `.wait` / `.kill` / `.try_wait`. v1 ships `.status` /
        `.output` (block-and-wait) only.
- [x] String interpolation routes through `Display.fmt`.
      **Phase D2 (2026-05-13):** `lower_interpolation` emits the
      canonical `Formatter_new` → `{T}_fmt(value, fmt)` →
      `Formatter_buffer` sequence for primitives (Char / Int /
      Float / Bool) and for any user type that does `include Display`
      and provides `def fmt`.
      Synth `Char_fmt` / `Int_fmt` / `Float_fmt` / `Bool_fmt` /
      `String_fmt` MIR fns wrap the existing `riven_*_to_string`
      runtime helpers so observable output is byte-identical to the
      legacy ad-hoc switch. Implicit-`Debug`-only types still fall back
      to `{Name}_to_debug` for the bare `"#{x}"` form until users
      provide their own `include Display`. The `Err(e).message()`
      inference gap remains separately tracked. Commits: S0
      24b84c3 → S1 2683c6d → S2 47250b1 / fd24313 → S3 eb297e2 →
      S4 a1658e9.
      **Phase D4 (2026-05-13):** width / align / precision / fill
      now apply at runtime.  Spec-aware constructor
      `Formatter_new_with_spec(w, p, a, f)` is emitted by
      `emit_display_dispatch` when the lex-captured `FormatSpec` is
      non-default.  Width / align / fill applied at
      `Formatter_buffer` finalize; precision routes through
      `Float_to_string_prec` (snprintf `%.*f`) for floats and
      `String_truncate_chars` (UTF-8 char-count truncate) for
      strings — `Int` / `Bool` / `Char` ignore precision per Rust
      semantics.  Out of scope: width-on-`:?` (debug path bypasses
      Formatter); sign / `#` / `0` / radix flags.  Commit: 4491508.
- [x] `Debug` interpolation `"#{x:?}"` works for any type with the
      implicit `Debug` include.
      **Phase B + C MVP:** format spec captured at lex time
      (FormatSpec.debug = true), threaded through HIR/MIR. Existing
      `_to_debug` synthesis on implicit-`Debug` structs already produces
      the expected output for `"#{x:?}"`; the bare `"#{x}"` form
      currently uses the same path (Phase D will switch bare to
      Display.fmt). Pin tests in `stdlib_fmt.rs::debug_interpolation_spec_typechecks`.
- [ ] CI green.
- [ ] CHANGELOG bullet. **Partial:** env/fs first-batch + std.io
      surface entries shipped + std.fmt foundation A/B/C/D-MVP
      shipped.
