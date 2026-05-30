/* std.test runtime entry points:
 *
 *   ruxen_test_current_set / ruxen_test_current_get
 *     Process-static slot holding the active Runner handle (an int64
 *     pointer cast to int64_t). Set by Runner.new before the user file
 *     body runs; read by Tester.describe to know which Runner to
 *     attach new root groups to. Single-thread access only —
 *     a test binary's DSL-setup phase is strictly single-threaded.
 *
 *   ruxen_test_runner_addr
 *     Identity function: takes `self` (the Runner heap pointer, passed
 *     as int64_t at the FFI boundary) and returns it as int64_t.
 *     Used to implement `Runner#handle_addr` without __addr_of_self.
 *
 *   ruxen_test_runner_from_addr
 *     Identity function: takes an int64_t address and returns it as
 *     int64_t. Declared in Ruxen as `-> Runner` so the type system
 *     treats the returned integer as a Runner object reference.
 *     Used to implement `Runner.from_handle` without __from_addr[T].
 *
 *   ruxen_test_case_reset / ruxen_test_case_mark_failed / ruxen_test_case_get_failed
 *     Per-case failure flag. Reset before each case body, set by any
 *     failing matcher path. Reads as 0=pass, 1=fail.
 *
 *   ruxen_test_fork_and_wait (Task 5.1)
 *     fork() + child runs a Ruxen closure + exit + parent waitpid.
 *     Lives in this file; Phase 5 fills the body.
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/types.h>
#include <sys/wait.h>

#include "../../core/runtime/runtime.h"

static int64_t ruxen_test_current_runner = 0;

int64_t ruxen_test_current_set(int64_t handle) {
    int64_t prev = ruxen_test_current_runner;
    ruxen_test_current_runner = handle;
    return prev;
}

int64_t ruxen_test_current_get(void) {
    return ruxen_test_current_runner;
}

/* Self-address helpers — identity functions at the C level; the type
 * annotation on the Ruxen lib decl does all the semantic lifting.
 *
 * ruxen_test_runner_addr:   Runner.self -> Int (for handle_addr)
 * ruxen_test_runner_from_addr: Int -> Runner (for from_handle)
 * ruxen_test_tester_addr:   Tester.self -> Int (for tester_handle)
 * ruxen_test_tester_from_addr: Int -> Tester (for Tester.from_int)
 * ruxen_test_testcase_from_addr: Int -> TestCase (for TestCase.from_int)
 */
int64_t ruxen_test_runner_addr(int64_t self) {
    return self;
}

int64_t ruxen_test_runner_from_addr(int64_t addr) {
    return addr;
}

int64_t ruxen_test_tester_addr(int64_t self) {
    return self;
}

int64_t ruxen_test_tester_from_addr(int64_t addr) {
    return addr;
}

int64_t ruxen_test_testcase_addr(int64_t self) {
    return self;
}

int64_t ruxen_test_testcase_from_addr(int64_t addr) {
    return addr;
}

/* Per-case failure flag. */
static int64_t ruxen_test_case_failed = 0;

int64_t ruxen_test_case_reset(void) {
    ruxen_test_case_failed = 0;
    return 0;
}

int64_t ruxen_test_case_mark_failed(void) {
    ruxen_test_case_failed = 1;
    return 0;
}

int64_t ruxen_test_case_get_failed(void) {
    return ruxen_test_case_failed;
}

/* ─── Phase 5: fork-per-test isolation ────────────────────────────────
 *
 * Child-process model: parent (the test binary's main) calls
 * `ruxen_test_fork` before each case's body. Child runs the body and
 * calls `ruxen_test_child_exit(code)` (0 = pass, 1 = fail). Parent
 * calls `ruxen_test_wait(pid)` and decodes the packed status to update
 * Runner counts.
 *
 * Packed status layout (matches Phase 5.1 spec):
 *   bit 0:    1 if WIFEXITED && WEXITSTATUS == 0
 *   bits 1-8: WEXITSTATUS & 0xff (if exited normally)
 *   bits 9-16: WTERMSIG & 0xff (if killed by signal)
 *
 * A child that calls `ruxen_panic` in core/runtime/alloc.c exits with
 * status 101 (not a signal) — handled by parent reading exit_code != 0.
 */
int64_t ruxen_test_fork(void) {
    /* fflush both stdio streams before fork so buffered output is not
     * duplicated by the child's exit. */
    fflush(stdout);
    fflush(stderr);
    pid_t pid = fork();
    if (pid < 0) {
        ruxen_panic("ruxen_test_fork: fork() failed");
    }
    return (int64_t)pid;
}

int64_t ruxen_test_wait(int64_t pid) {
    int status = 0;
    /* Single waitpid — children are short-lived test bodies, and a
     * SIGCHLD-interrupted wait would only occur if the user is killing
     * the suite (in which case returning -1 lets the parent's pass-bit
     * decode treat the case as a failure, which is correct). */
    pid_t result = waitpid((pid_t)pid, &status, 0);
    if (result < 0) {
        return -1;
    }
    int64_t packed = 0;
    if (WIFEXITED(status)) {
        if (WEXITSTATUS(status) == 0) {
            packed |= 1;
        }
        packed |= ((int64_t)WEXITSTATUS(status) & 0xff) << 1;
    }
    if (WIFSIGNALED(status)) {
        packed |= ((int64_t)WTERMSIG(status) & 0xff) << 9;
    }
    return packed;
}

/* Child-side: redirect stderr to a file so the parent can scan for
 * expect_panic substrings after waitpid returns. Kept available for
 * v1.1 substring verification; not used by v1 it_panics. */
int64_t ruxen_test_redirect_stderr(int64_t path_handle, int64_t path_len) {
    const char *path = (const char *)path_handle;
    (void)path_len;
    int fd = open(path, O_CREAT | O_WRONLY | O_TRUNC, 0644);
    if (fd < 0) return -1;
    if (dup2(fd, 2) < 0) {
        close(fd);
        return -1;
    }
    close(fd);
    return 0;
}

/* Child-side: explicit exit. We DO flush stdio before _exit so the
 * child's `puts` output (hook traces, assertion failure messages) is
 * visible to the parent's stdout pipe. Without flush, line-buffered
 * stdout in the child is dropped on _exit.
 *
 * Note: we already fflush(stdout)/fflush(stderr) BEFORE fork in
 * ruxen_test_fork, so the parent's buffer is empty at the fork point —
 * the only thing in the child's buffer when we get here was produced
 * AFTER fork (by the test body), so there is no risk of double-write
 * in the parent.
 */
void ruxen_test_child_exit(int64_t code) {
    fflush(stdout);
    fflush(stderr);
    _exit((int)code);
}

/* Test-fixture entry point: trigger a panic with a fixed message so
 * `.rx` fixtures can simulate user panics without depending on
 * `unwrap!`/`expect!` ergonomics. The message is hard-coded to keep the
 * FFI signature simple (no string-pointer marshalling at the call
 * site — Ruxen strings are heap objects with a separate length).
 *
 * Fixtures call `Runner.panic` (declared on Runner via lib decl); we
 * keep the symbol name `ruxen_test_panic` to make the panic source
 * obvious in stderr traces.
 */
void ruxen_test_panic(void) {
    ruxen_panic("ruxen_test_panic: simulated explosion");
}
