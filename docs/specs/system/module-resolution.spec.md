# Spec — Module resolution + runtime startup

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §6](../../requirements/tier1_01_stdlib.md)
(`Module System Design`).

**Status:** shipped Phase 1 (resolver) + extended through every
stdlib module landing.

This spec covers the `use ...` import surface and the runtime-entry
shim that bootstraps argv before user code runs.  Every numbered
behaviour is pinned by an integration test in
[`crates/ruxen-core/tests/std_use_resolution.rs`](../../../crates/ruxen-core/tests/std_use_resolution.rs).

---

## B1 — `use std.<mod>.<name>` typechecks cleanly

For every shipped stdlib module, a `use` statement importing one or
more symbols typechecks cleanly with **zero** diagnostics:

| Module        | Import form                                            |
|---------------|--------------------------------------------------------|
| `std.io`      | `use std.io.{Stdin, Stdout, Stderr}`                   |
| `std.io`      | `use std.io.{stdin, stdout, stderr}` (lowercase helpers) |
| `std.env`     | `use std.env.{args, var, vars, current_dir}`           |
| `std.fs`      | `use std.fs.{read_to_string, write, exists, …}`        |
| `std.process` | `use std.process.{exit, Command}`                      |
| `std.sync`    | `use std.sync.{Thread, Mutex, SharedSync, JoinHandle, …}` |

## B2 — Group imports

The `use a.b.{x, y, z}` group form parses + resolves correctly.  Each
imported name binds in the current scope.

## B3 — Method dispatch on imported types

After `use std.io.Stdout`, methods on `Stdout` (e.g.
`Stdout.new().println(...)`) resolve to their typeck signatures and
lower to the right runtime symbol.

## B4 — End-to-end round-trip for shipped runtimes

For modules with shipped runtime impls, a complete
`use → call → assert on stdout/stderr` round-trip works:

| Module                  | Round-trip pin                              |
|-------------------------|---------------------------------------------|
| `std.io` println        | `std_io_println_and_eprintln_round_trip`    |
| `std.io` write_str      | `std_io_write_str_result_is_unit_and_round_trips` |
| `std.io` read_line      | `std_io_read_line_and_stdout_round_trip`    |
| `std.io` read_to_string | `std_io_stdin_read_to_string_round_trip`    |
| `std.env` args          | `std_env_args_round_trip`                   |
| `std.env` var           | `std_env_var_round_trip`                    |
| `std.fs`                | `std_fs_round_trip`                         |
| `std.fs` mutations      | `std_fs_mutation_helpers_round_trip`        |
| `std.fs` create_dir_all | `std_fs_create_dir_all_round_trip`          |
| `std.process` exit      | `std_process_exit_round_trip`               |
| `std.sync` thread util  | `std_sync_thread_sleep_and_yield_round_trip`|

## B5 — `main` shim initialises runtime argv

Every Ruxen binary has a generated `main` shim that calls
`ruxen_env_init(argc, argv)` before user code runs.  This lets
`std.env.args()` return the right values from the first line of
`main`.

**Given** a program that calls `args()` from `def main`
**When** the binary runs with `argv = [bin, "a", "b"]`
**Then** `args()` returns an `Array[String]` with at least three
entries (the program name plus the two user args).

---

## Pin tests

All behaviours live in
`crates/ruxen-core/tests/std_use_resolution.rs`:

| Behaviour | Test fn                                                 |
|-----------|---------------------------------------------------------|
| B1 io     | `use_std_io_typechecks_cleanly`                         |
| B1 env    | `use_std_env_typechecks_cleanly`                        |
| B1 fs     | `use_std_fs_typechecks_cleanly`                         |
| B1 fs+    | `use_std_fs_mutation_helpers_typecheck_cleanly`         |
| B1 fs++   | `use_std_fs_create_dir_all_typechecks_cleanly`          |
| B1 proc   | `use_std_process_typechecks_cleanly`                    |
| B1 sync   | `std_sync_concurrency_surface_typechecks_cleanly`       |
| B2        | `std_io_group_imports_and_methods_typecheck_cleanly`    |
| B3        | covered transitively by every B4 round-trip             |
| B4        | (see table above)                                       |
| B5        | `main_shim_initializes_runtime_argv`                    |

---

## Out of scope (v2)

- User-defined modules with `module foo ... end`.  The parser
  accepts them but resolve currently only handles `std.*` paths
  for type lookup.
- External packages (`use otherpackage.mod.X`).  Tracked in tier4_01
  package-manager work.
- Re-exports (`use ...` re-exports).
- Glob imports (`use std.io.*`).
- Submodule lookup (`use std.io.{Stdin, Stdout as Out}` — the
  `as` alias is parsed but the alias rebind isn't yet enforced).
