# Spec — `std.path`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.9](../../requirements/tier1_01_stdlib.md).

**Status:** shipped Phase 3 (POSIX path operations).

`std.path` provides string-based POSIX path manipulation.  Linux-
style forward-slash separators only — Windows backslash support is a
non-goal for v1.

---

## B1 — `path_join(a, b) -> String` concatenates segments

**Given** `a = "/usr/local"`, `b = "bin/riven.rvn"`
**When** the program calls `path_join(&a, &b)`
**Then** the result is `"/usr/local/bin/riven.rvn"` (one separator
inserted; no double-slash).

## B2 — `path_join` defers to absolute second argument

**Given** `a = "/etc"`, `b = "/usr/bin"`
**When** the program calls `path_join(&a, &b)`
**Then** the result is `"/usr/bin"` (b's leading `/` overrides a).
Matches Rust's `Path::join` semantics.

## B3 — `path_parent(p) -> String` strips the final component

**Given** `p = "/usr/local/bin/riven.rvn"`
**Then** `path_parent(&p)` returns `"/usr/local/bin"`.

## B4 — `path_file_name(p) -> String` returns the final component

**Given** `p = "/usr/local/bin/riven.rvn"`
**Then** `path_file_name(&p)` returns `"riven.rvn"`.

## B5 — `path_extension(p) -> String`

Returns the extension (without the dot) when present.

**Given** `p = "/usr/local/bin/riven.rvn"`
**Then** `path_extension(&p)` returns `"rvn"`.

**Given** `p = "/foo/bar"` (no extension)
**Then** `path_extension(&p)` returns `""` (empty string, not Err).

**Given** `p = "/foo/.hidden"` (dotfile with no real extension)
**Then** `path_extension(&p)` returns `""` (the leading dot is part of
the filename, not an extension).

## B6 — `path_is_absolute(p) -> Bool`

Returns `true` iff `p` starts with `/`.

**Given** `p = "/etc"`
**Then** `path_is_absolute(&p)` returns `true`.

**Given** `p = "foo/bar"`
**Then** `path_is_absolute(&p)` returns `false`.

---

## Pin tests

| Behaviour | Test fn                              | File             |
|-----------|--------------------------------------|------------------|
| B1, B3, B4, B5, B6 | `path_module_basic_operations`  | `stdlib_path.rs` |
| B2        | `path_join_handles_absolute_second`  | `stdlib_path.rs` |
| B5 (empty cases) | `path_extension_empty_when_missing` | `stdlib_path.rs` |
| B6        | `path_is_absolute_detects_root`      | `stdlib_path.rs` |

---

## Out of scope (v2)

- `PathBuf` / `Path` typed values; v1 uses `String` throughout.
- Windows backslash separators and drive-letter handling.
- Normalisation (`..` / `.` resolution beyond what `path_parent`
  does on the trailing segment).
- Canonicalisation that touches the filesystem (would belong in
  `std.fs`).
