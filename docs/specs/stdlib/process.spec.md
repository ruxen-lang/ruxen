# Spec — `std.process`

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §4.7](../../requirements/tier1_01_stdlib.md),
[docs/prompts/v1/06_phase2_stdlib_io_fmt.md](../../prompts/v1/06_phase2_stdlib_io_fmt.md),
[docs/prompts/v1/06_5_phase2_sync_io_completeness.md](../../prompts/v1/06_5_phase2_sync_io_completeness.md).

**Status:** `exit` shipped Phase 2 #06.  `Command` builder shipped
Phase 2 #06 (status / output terminals).  The flat `process_run`
free-fn previously offered as a shortcut was removed in #06.5 T5.5
once `Command.{status, output}` covered every use case.

`std.process` provides primitive operations on the current process
(`exit`) and a typed `Command` builder for spawning child processes.

---

## B1 — `process.exit(code: Int) -> !`

Terminates the current process with the given exit code.  Returns the
never-type `!` — control never returns to the caller.

`code` is widened to the OS-native exit code (8 bits on POSIX).
Caller-side encoding (e.g. `exit(-1)` → `255`) follows libc semantics.

## B2 — `Command.new(program: &str) -> Command`

Constructs a child-process builder.  No process is spawned until one
of the terminal methods (`.status`, `.output`) is called.  The
returned `Command` owns the argv / env / cwd it accumulates; dropping
it without calling a terminal is a no-op.

## B3 — Builder methods append argv / env / cwd

| Method | Effect |
|--------|--------|
| `.arg(s: &str) -> Command`             | Append one argv slot. |
| `.args(xs: Array[String]) -> Command`  | Append every slot in order. |
| `.env(k: &str, v: &str) -> Command`    | Add / overwrite an env entry. |
| `.current_dir(p: &str) -> Command`     | Set the child's cwd. |

All builder methods return `Command` so the chain composes
left-to-right.  Implementations must be order-preserving (argv slots
appear in the child in the order they were added).

## B4 — `Command.status -> Result[ExitStatus, IoError]`

Forks + execs the child, inherits stdio, waits for completion,
returns the exit status.  Replaces the v0 `process_run(cmd, args)`
free-fn — every behaviour previously expressed as
`process_run(...) == 0` is now expressed as
`match cmd.status { Ok(s) -> s.code == 0; Err(_) -> false }`.

**Given** `Command.new("/usr/bin/true").status`
**Then** the result is `Ok(s)` with `s.code == 0`.

**Given** `Command.new("/usr/bin/false").status`
**Then** the result is `Ok(s)` with `s.code == 1`.

**Given** `Command.new("/bin/echo").arg("hello").status`
**Then** the child runs `/bin/echo hello`, the parent's stdout
captures `"hello\n"`, and the result is `Ok(s)` with `s.code == 0`.

**Given** `Command.new("/no/such/binary").status`
**Then** the result is `Err(IoError.NotFound(_))` (a pre-flight
`access(F_OK)` check turns a typo'd binary into a structured error
rather than an exit code that aliases `127`).

### B4 failure-mode encoding (when the child does fork+exec OK)

| Outcome                          | `ExitStatus.code` |
|----------------------------------|-------------------|
| Normal exit with code `n`        | `n`               |
| Killed by signal `s`             | `128 + s`         |

Stdio is inherited — the child writes to the parent's stdout/stderr
directly.  Use `.output` to capture instead.

## B5 — `Command.output -> Result[Output, IoError]`

Same fork+exec contract as `.status`, but the child's stdout and
stderr are captured into the returned `Output`.  `Output.stdout` /
`Output.stderr` return `Array[UInt8]`; `Output.status` returns the
same `ExitStatus` shape as `.status`.

---

## Pin tests

| Behaviour | Test fn                                       | File                       |
|-----------|-----------------------------------------------|----------------------------|
| B1 zero   | `process_exit_zero_returns_zero`              | `stdlib_process.rs`        |
| B1 one    | `process_exit_one_returns_one`                | `stdlib_process.rs`        |
| B1 42     | `process_exit_forty_two_returns_forty_two`    | `stdlib_process.rs`        |
| B1 23     | `std_process_exit_round_trip`                 | `std_use_resolution.rs`    |
| B4 zero   | `command_status_true_returns_zero`            | `stdlib_process.rs`        |
| B4 one    | `command_status_false_returns_one`            | `stdlib_process.rs`        |
| B3 arg    | `command_arg_passes_through`                  | `stdlib_process.rs`        |
| B3 args   | `command_args_bulk`                           | `stdlib_process.rs`        |
| B3 env    | `command_env_visible_to_child`                | `stdlib_process.rs`        |
| B3 cwd    | `command_current_dir_changes_cwd`             | `stdlib_process.rs`        |
| B4 enoent | `command_nonexistent_binary_returns_err`      | `stdlib_process.rs`        |
| B5 stdout | `command_output_captures_stdout`              | `stdlib_process.rs`        |
| B5 stderr | `command_output_captures_stderr`              | `stdlib_process.rs`        |
| B4 signal | gap — see below                               |                            |

---

## Gaps

- B4 signal-termination pin (kill the child with SIGTERM, expect
  `ExitStatus.code == 128 + 15 = 143`) is not yet shipped — needs
  careful platform handling to avoid CI flakes.  Tracked.

## Removed in #06.5 T5.5

- `process_run(cmd, args) -> Int` free-fn — superseded by
  `Command.new(cmd).args(args).status`.  The C symbol
  `riven_process_run` is still linked (it is the implementation used
  internally by `riven_command_status`) but is no longer reachable
  from Riven user code.

## Out of scope (v2)

- `Command.spawn() -> Child` (async / non-blocking spawn).
- Stdin piping (`.stdin(Stdio::piped()).input(...)`).
- Windows support — POSIX-only in v1 (tier4_04 carve-out).
