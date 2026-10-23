# Spec — `std.fs`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.3](../../requirements/tier1_01_stdlib.md),
[docs/prompts/v1/06_phase2_stdlib_io_fmt.md](../../prompts/v1/06_phase2_stdlib_io_fmt.md).

**Status:** shipped Phase 2 #06 (first batch, 2026-04 → 2026-05).
`metadata` deferred to v2.

`std.fs` provides whole-file read/write helpers and boolean path
predicates.  All errors surface as `Result.Err(IoError)`.

---

## B1 — `read_to_string(path) -> Result[String, IoError]`

Reads the entire file into a fresh `String`.

**Given** a UTF-8 file at `path`
**When** the program calls `fs.read_to_string(path)`
**Then** the result is `Result.Ok(contents)` where `contents` is the
file's bytes interpreted as UTF-8.

**Given** the file does not exist
**Then** the result is `Result.Err(io_error)` whose `.message()`
mentions "not found".

## B2 — `write(path, contents) -> Result[nil, IoError]`

Writes `contents` to `path`, truncating any existing file.

**Given** a writable destination
**When** the program calls `fs.write(path, "hi")`
**Then** the result is `Result.Ok(nil)` and reading the file back
yields `"hi"`.

## B3 — `exists(path) -> Bool`

Returns `true` iff a filesystem entity exists at `path` (regardless
of type).

## B4 — `is_file(path) -> Bool` distinguishes regular files

**Given** a regular file at `path`
**Then** `fs.is_file(path)` returns `true` and `fs.is_dir(path)`
returns `false`.

**Given** a directory at `path`
**Then** `fs.is_file(path)` returns `false`.

**Given** a path that doesn't exist
**Then** `fs.is_file(path)` returns `false` (no error).

## B5 — `is_dir(path) -> Bool` distinguishes directories

Mirror of B4: returns `true` only for directories.

## B6 — `read_dir(path) -> Result[Array[String], IoError]` lists entries

**Given** a directory containing files `a`, `b`, `c`
**When** the program calls `fs.read_dir(path)`
**Then** the result is `Result.Ok(entries)` where `entries` contains exactly
`["a", "b", "c"]` (order unspecified; sort before comparing).

Hidden files (`.dotfile`) are included.  `.` and `..` are filtered.

## B7 — `read_dir(missing)` returns `Result.Err`

**Given** `path` does not exist
**When** the program calls `fs.read_dir(path)`
**Then** the result is `Result.Err(io_error)`.

## B8 — write-side helpers: `create_dir`, `create_dir_all`, `remove_file`, `rename`

These return `Result[nil, IoError]`, pinned in
`library/std/fs/tests/fs_test.rx`.

---

## Pin tests

| Behaviour | Test fn                                       | File             |
|-----------|-----------------------------------------------|------------------|
| B1 ok     | "round-trips write and read_to_string" | `library/std/fs/tests/fs_test.rx` |
| B1 err    | "returns NotFound from read_to_string for a missing file" | `library/std/fs/tests/fs_test.rx` |
| B2        | "round-trips write and read_to_string" | `library/std/fs/tests/fs_test.rx` |
| B3        | "tells files, directories and missing paths apart" | `library/std/fs/tests/fs_test.rx` |
| B4        | "tells files, directories and missing paths apart" | `library/std/fs/tests/fs_test.rx` |
| B5        | "tells files, directories and missing paths apart" | `library/std/fs/tests/fs_test.rx` |
| B6        | "lists every entry but . and .. with read_dir" | `library/std/fs/tests/fs_test.rx` |
| B7        | "returns Err from read_dir for a missing directory" | `library/std/fs/tests/fs_test.rx` |
| B8 mkdir  | "creates a directory with create_dir" | `library/std/fs/tests/fs_test.rx` |
| B8 rmfile | "removes a file with remove_file" | `library/std/fs/tests/fs_test.rx` |
| B8 mkdir-p| `std_fs_create_dir_all_round_trip`            | `std_use_resolution.rs`  |
| B8 rename | `std_fs_mutation_helpers_round_trip`          | `std_use_resolution.rs`  |

---

## Gaps

None at present — every behaviour has a direct pin (B1/B2/B3/B8
direct pins added 2026-05; B8 `create_dir_all` and `rename` were
already covered by `std_use_resolution.rs` round-trips).

## Out of scope (v2)

- `fs.metadata(path) -> Result[Metadata, IoError]` — needs a
  `Metadata` struct surface (size, kind, mtime).  The boolean
  predicates plus `exists` cover the v1 minimum.
- Recursive copy / remove / walk helpers.
- File-handle API (`File.open`, seekable reads/writes).
