/* std.test runtime entry points:
 *
 *   riven_test_current_set / riven_test_current_get
 *     Process-static slot holding the active Runner handle (an int64
 *     pointer cast to int64_t). Set by Runner.new before the user file
 *     body runs; read by Tester.describe to know which Runner to
 *     attach new root groups to. Single-thread access only —
 *     a test binary's DSL-setup phase is strictly single-threaded.
 *
 *   riven_test_runner_addr
 *     Identity function: takes `self` (the Runner heap pointer, passed
 *     as int64_t at the FFI boundary) and returns it as int64_t.
 *     Used to implement `Runner#handle_addr` without __addr_of_self.
 *
 *   riven_test_runner_from_addr
 *     Identity function: takes an int64_t address and returns it as
 *     int64_t. Declared in Riven as `-> Runner` so the type system
 *     treats the returned integer as a Runner object reference.
 *     Used to implement `Runner.from_handle` without __from_addr[T].
 *
 *   riven_test_case_reset / riven_test_case_mark_failed / riven_test_case_get_failed
 *     Per-case failure flag. Reset before each case body, set by any
 *     failing matcher path. Reads as 0=pass, 1=fail.
 *
 *   riven_test_fork_and_wait (Task 5.1)
 *     fork() + child runs a Riven closure + exit + parent waitpid.
 *     Lives in this file; Phase 5 fills the body.
 */

#include <stdint.h>

static int64_t riven_test_current_runner = 0;

int64_t riven_test_current_set(int64_t handle) {
    int64_t prev = riven_test_current_runner;
    riven_test_current_runner = handle;
    return prev;
}

int64_t riven_test_current_get(void) {
    return riven_test_current_runner;
}

/* Self-address helpers — identity functions at the C level; the type
 * annotation on the Riven lib decl does all the semantic lifting.
 *
 * riven_test_runner_addr:   Runner.self -> Int (for handle_addr)
 * riven_test_runner_from_addr: Int -> Runner (for from_handle)
 * riven_test_tester_addr:   Tester.self -> Int (for tester_handle)
 * riven_test_tester_from_addr: Int -> Tester (for Tester.from_int)
 * riven_test_testcase_from_addr: Int -> TestCase (for TestCase.from_int)
 */
int64_t riven_test_runner_addr(int64_t self) {
    return self;
}

int64_t riven_test_runner_from_addr(int64_t addr) {
    return addr;
}

int64_t riven_test_tester_addr(int64_t self) {
    return self;
}

int64_t riven_test_tester_from_addr(int64_t addr) {
    return addr;
}

int64_t riven_test_testcase_addr(int64_t self) {
    return self;
}

int64_t riven_test_testcase_from_addr(int64_t addr) {
    return addr;
}

/* Per-case failure flag. */
static int64_t riven_test_case_failed = 0;

int64_t riven_test_case_reset(void) {
    riven_test_case_failed = 0;
    return 0;
}

int64_t riven_test_case_mark_failed(void) {
    riven_test_case_failed = 1;
    return 0;
}

int64_t riven_test_case_get_failed(void) {
    return riven_test_case_failed;
}
