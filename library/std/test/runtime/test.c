/* std.test runtime — three entry points only:
 *
 *   riven_test_current_set / riven_test_current_get
 *     Process-static slot holding the active Runner handle (an int64
 *     pointer cast to int64_t). Set by Runner.new before the user file
 *     body runs; read by Tester.describe to know which Runner to
 *     attach new root groups to. Single-thread access only —
 *     a test binary's DSL-setup phase is strictly single-threaded.
 *
 *   riven_test_fork_and_wait (Task 5.1)
 *     fork() + child runs a Riven closure + exit + parent waitpid.
 *     Lives in this file; Phase 5 fills the body.
 */

#include <stdint.h>

#include "../../core/runtime/runtime.h"

static int64_t riven_test_current_runner = 0;

int64_t riven_test_current_set(int64_t handle) {
    int64_t prev = riven_test_current_runner;
    riven_test_current_runner = handle;
    return prev;
}

int64_t riven_test_current_get(void) {
    return riven_test_current_runner;
}
