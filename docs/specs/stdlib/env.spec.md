# Spec — `std::env`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.6](../../requirements/tier1_01_stdlib.md),
[docs/prompts/v1/06_phase2_stdlib_io_fmt.md](../../prompts/v1/06_phase2_stdlib_io_fmt.md).

**Status:** shipped Phase 2 #06 (first batch, 2026-04).

`std::env` exposes process-level environment access: command-line
arguments, environment variables, and the working directory.

---

## B1 — `args() -> Vec[String]`

Returns the process's command-line argv as a `Vec[String]`.  Element 0
is the program name; subsequent elements are user arguments.  Always
non-empty (at least the program name is present).

## B2 — `var(name: &str) -> Result[String, VarError]`

Looks up a single environment variable.

**Given** an environment with `FOO=bar`
**When** the program calls `env::var("FOO")`
**Then** the result is `Result::Ok("bar")`.

**Given** the variable is unset
**Then** the result is `Result::Err(VarError::NotPresent)`.

## B3 — `vars() -> HashMap[String, String]`

Returns a snapshot of every environment variable as a `HashMap`.

**Given** an environment with at least one variable set
**Then** `env::vars()` returns a non-empty map containing that key →
value pair.

**Snapshot semantics:** modifications to the OS environment after
`vars()` returns are not reflected in the returned map.

## B4 — `current_dir() -> Result[String, IoError]`

Returns the process's current working directory as an absolute path
string.

**Given** the process started in any directory
**When** the program calls `env::current_dir()`
**Then** the result is `Result::Ok(path)` where `path` is a non-empty
absolute path.

`Result::Err` is reserved for the rare case where `getcwd(3)` fails
(directory unlinked while the process is alive, permission errors).

---

## Pin tests

| Behaviour | Test fn                              | File                |
|-----------|--------------------------------------|---------------------|
| B1        | `args_returns_program_name_and_user_args` (existing pre-#06 surface — covered transitively by E2E fixtures that call `args()`) | `tests/release-e2e/cases/` |
| B2        | (existing pre-#06 surface; covered transitively in REPL coverage tests) | |
| B3        | `env_vars_snapshots_process_environment` | `stdlib_env.rs` |
| B3        | `env_vars_is_non_empty_when_one_var_set` | `stdlib_env.rs` |
| B4        | `env_current_dir_returns_ok_path`        | `stdlib_env.rs` |

---

## Gaps (add pin tests when next touched)

- B1 and B2 ship via the pre-#06 runtime surface but have no dedicated
  `stdlib_env.rs` pin tests; coverage comes from E2E fixtures + REPL
  scenarios.  Adding direct pins is a small follow-up.

## Out of scope (v2)

- `set_var` / `remove_var` write-side helpers — read-only in v1.
- `args_os` (OS-native bytes) — `args()` UTF-8-decodes argv and
  rejects invalid sequences.
