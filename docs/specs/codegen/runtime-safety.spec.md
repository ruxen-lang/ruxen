# Spec — C runtime safety guarantees

**Source docs:**
[docs/requirements/](../../requirements/) (cross-cutting; see ROADMAP).

**Status:** shipped Phase 1-2 (runtime sanitizer build + ABI pins).

The C runtime (`crates/riven-core/runtime/runtime.c`) is the only
unsafe-by-default part of the Riven compiler.  This spec captures
the structural guarantees the test harness enforces on it.

---

## B1 — Strict-warnings build is clean

The runtime compiles under `-Werror -Wall -Wextra -Wpedantic`
without warnings.  Acts as a regression canary for casual `unsigned`
/ `int64_t` conversions, unused-fn drift, and printf-format errors.

## B2 — Sanitiser build is clean

The runtime compiles with ASan + UBSan and a smoke-test invocation
produces no sanitiser diagnostics.  Catches misalignment, integer
UB, and obvious memory bugs at PR time.

## B3 — `riven_env_init` copies argv and clones reads

`riven_env_init` (called at process start to capture argv) makes a
defensive copy of the OS argv buffer.  Subsequent reads via
`riven_env_args_*` return clones, not pointers into the original.
Catches the class of bugs where argv mutation by `setproctitle` /
LD_PRELOAD would corrupt later reads.

## B4 — `fs`, `env`, and `process` helpers match their declared ABI

Each runtime helper used by `std.fs`, `std.env`, and
`std.process` has a fixed C signature.  The pin test compiles a
minimal C harness that includes the runtime header and calls every
helper with the declared argument types — any signature drift fails
the build.

## B5 — Static assertions enforce 64-bit pointer width

`_Static_assert(sizeof(void *) == 8)` and
`_Static_assert(sizeof(void *) == sizeof(int64_t))` at the top of
`runtime.c` reject 32-bit builds.  Riven assumes 64-bit pointers
throughout the MIR + codegen.

(C tokens `void *` / `int64_t` above are C source — not Riven
surface syntax — and stay as written.)

---

## Pin tests

| Behaviour | Test fn                                                | File                  |
|-----------|--------------------------------------------------------|-----------------------|
| B1        | `runtime_compiles_with_strict_warnings`                | `runtime_safety.rs`   |
| B2        | `runtime_compiles_with_sanitizers`                     | `runtime_safety.rs`   |
| B3        | `runtime_env_init_copies_argv_and_clones_reads`        | `runtime_safety.rs`   |
| B4        | `runtime_fs_env_and_process_helpers_match_expected_abi`| `runtime_safety.rs`   |
| B5        | compile-time `_Static_assert`s in `runtime.c`           | (build-time)          |

---

## Out of scope (v2)

- 32-bit pointer support.
- Windows runtime port (POSIX-only in v1).
- Formal verification of the unsafe parts (sanitisers + ABI pins
  cover the common-bug envelope).
