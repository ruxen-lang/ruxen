# Spec — `std::fs`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.3](../../requirements/tier1_01_stdlib.md),
[docs/prompts/v1/06_phase2_stdlib_io_fmt.md](../../prompts/v1/06_phase2_stdlib_io_fmt.md).

**Status:** shipped Phase 2 #06 (first batch, 2026-04 → 2026-05).
`metadata` deferred to v2.

`std::fs` provides whole-file read/write helpers and boolean path
predicates.  All errors surface as `Result::Err(IoError)`.

---

## B1 — `read_to_string(path) -> Result[String, IoError]`

Reads the entire file into a fresh `String`.

**Given** a UTF-8 file at `path`
**When** the program calls `fs::read_to_string(path)`
**Then** the result is `Result::Ok(contents)` where `contents` is the
file's bytes interpreted as UTF-8.

**Given** the file does not exist
**Then** the result is `Result::Err(io_error)` whose `.message()`
mentions "not found".

## B2 — `write(path, contents) -> Result[(), IoError]`

Writes `contents` to `path`, truncating any existing file.

**Given** a writable destination
**When** the program calls `fs::write(path, "hi")`
**Then** the result is `Result::Ok(())` and reading the file back
yields `"hi"`.

## B3 — `exists(path) -> Bool`

Returns `true` iff a filesystem entity exists at `path` (regardless
of type).

## B4 — `is_file(path) -> Bool` distinguishes regular files

**Given** a regular file at `path`
**Then** `fs::is_file(path)` returns `true` and `fs::is_dir(path)`
returns `false`.

**Given** a directory at `path`
**Then** `fs::is_file(path)` returns `false`.

**Given** a path that doesn't exist
**Then** `fs::is_file(path)` returns `false` (no error).

## B5 — `is_dir(path) -> Bool` distinguishes directories

Mirror of B4: returns `true` only for directories.

## B6 — `read_dir(path) -> Result[Vec[String], IoError]` lists entries

**Given** a directory containing files `a`, `b`, `c`
**When** the program calls `fs::read_dir(path)`
**Then** the result is `Result::Ok(vec)` where `vec` contains exactly
`["a", "b", "c"]` (order unspecified; sort before comparing).

Hidden files (`.dotfile`) are included.  `.` and `..` are filtered.

## B7 — `read_dir(missing)` returns `Result::Err`

**Given** `path` does not exist
**When** the program calls `fs::read_dir(path)`
**Then** the result is `Result::Err(io_error)`.

## B8 — write-side helpers: `create_dir`, `create_dir_all`, `remove_file`, `rename`

These return `Result[(), IoError]`.  Pin tests exist transitively
through the E2E pipeline but are not yet covered by dedicated
`stdlib_fs.rs` tests — flagged as a gap.

---

## Pin tests

| Behaviour | Test fn                                       | File             |
|-----------|-----------------------------------------------|------------------|
| B1 ok     | `fs_write_then_read_to_string_round_trips`    | `stdlib_fs.rs`   |
| B1 err    | `fs_read_to_string_missing_returns_err`       | `stdlib_fs.rs`   |
| B2        | `fs_write_then_read_to_string_round_trips`    | `stdlib_fs.rs`   |
| B3        | `fs_exists_distinguishes_existing_and_missing`| `stdlib_fs.rs`   |
| B4        | `fs_is_file_distinguishes_regular_files`      | `stdlib_fs.rs`   |
| B5        | `fs_is_dir_distinguishes_directories`         | `stdlib_fs.rs`   |
| B6        | `fs_read_dir_lists_all_entries`               | `stdlib_fs.rs`   |
| B7        | `fs_read_dir_missing_path_returns_err`        | `stdlib_fs.rs`   |
| B8 mkdir  | `fs_create_dir_then_is_dir`                   | `stdlib_fs.rs`   |
| B8 rmfile | `fs_remove_file_then_exists_false`            | `stdlib_fs.rs`   |

---

## Gaps

- B8 still needs dedicated pin tests for `create_dir_all` (recursive
  mkdir) and `rename`.  `create_dir` and `remove_file` are pinned
  as of 2026-05.

## Out of scope (v2)

- `fs::metadata(path) -> Result[Metadata, IoError]` — needs a
  `Metadata` struct surface (size, kind, mtime).  The boolean
  predicates plus `exists` cover the v1 minimum.
- Recursive copy / remove / walk helpers.
- File-handle API (`File::open`, seekable reads/writes).
