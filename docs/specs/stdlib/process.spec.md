# Spec — `std::process`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.7](../../requirements/tier1_01_stdlib.md),
[docs/prompts/v1/06_phase2_stdlib_io_fmt.md](../../prompts/v1/06_phase2_stdlib_io_fmt.md).

**Status:** `exit` + `process_run` helper shipped Phase 2 #06.  Full
`Command` builder deferred to v2.

`std::process` provides primitive operations on the current process
(exit) and a minimal child-process spawner (`process_run`) that
covers the v1 "run an external tool synchronously" use case.

---

## B1 — `process::exit(code: Int) -> !`

Terminates the current process with the given exit code.  Returns the
never-type `!` — control never returns to the caller.

`code` is widened to the OS-native exit code (8 bits on POSIX).
Caller-side encoding (e.g. `exit(-1)` → `255`) follows libc semantics.

## B2 — `process_run(cmd: &str, args: Vec[String]) -> Int`

Fork+execvp a child, inherit stdio, wait for completion, return the
child's exit code.

**Given** `cmd = "/usr/bin/true"`, `args = []`
**When** the program calls `process_run(cmd, args)`
**Then** the returned code is `0`.

**Given** `cmd = "/usr/bin/false"`, `args = []`
**Then** the returned code is `1`.

## B3 — `process_run` propagates argv

**Given** `cmd = "/bin/echo"`, `args = ["hello"]`
**When** the program calls `process_run(cmd, args)`
**Then** the child runs `/bin/echo hello`, the parent's stdout
captures `"hello\n"`, and the call returns `0`.

## B4 — `process_run` failure-mode encoding

Documented behaviour of the runtime helper:

| Outcome                          | Returned code      |
|----------------------------------|--------------------|
| Normal exit with code `n`        | `n`                |
| Killed by signal `s`             | `128 + s`          |
| `fork(2)` failure                | `127`              |
| `execvp(3)` failure              | `127`              |

Stdio is inherited — the child writes to the parent's stdout/stderr
directly.  Capturing output requires the full `Command` builder
(deferred).

---

## Pin tests

| Behaviour | Test fn                                       | File                       |
|-----------|-----------------------------------------------|----------------------------|
| B1 zero   | `process_exit_zero_returns_zero`              | `stdlib_process.rs`        |
| B1 one    | `process_exit_one_returns_one`                | `stdlib_process.rs`        |
| B1 42     | `process_exit_forty_two_returns_forty_two`    | `stdlib_process.rs`        |
| B1 23     | `std_process_exit_round_trip`                 | `std_use_resolution.rs`    |
| B2        | `process_run_true_returns_zero`               | `stdlib_process.rs`        |
| B2        | `process_run_false_returns_one`               | `stdlib_process.rs`        |
| B3        | `process_run_echo_with_args_returns_zero`     | `stdlib_process.rs`        |
| B4 exec   | `process_run_nonexistent_binary_returns_127`  | `stdlib_process.rs`        |
| B4 signal | gap — see below                               |                            |

---

## Gaps

- B4 signal-termination pin (kill the child with SIGTERM, expect
  `128 + 15 = 143`) is not yet shipped — needs careful platform
  handling to avoid CI flakes.  Tracked.

## Out of scope (v2)

- `Command::new(...).arg(...).env(K, V).spawn() -> Child` builder.
- `Command::output() -> Result[Output, IoError]` (capture stdout +
  stderr + exit code as a struct).
- `Command::status() -> Result[ExitStatus, IoError]`.
- Windows support — POSIX-only in v1 (tier4_04 carve-out).
