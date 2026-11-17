/* Weak link targets for symbols that the future runtime
 * (`library/std/future/runtime/scheduler.c`, `schedule.c`, `stream.c`)
 * forward-declares and the AOT backend synthesises per program
 * (`synthesize_dynamic_dispatch_helpers`). The REPL's libruxenrt.a link
 * does not go through Ruxen codegen, so without these the link fails with
 *   "_Future_dynamic_poll referenced from _ruxen_executor_pump_tasks".
 *
 * The REPL never drives the executor, so these must never run. If one
 * does, it aborts with a message instead of returning a made-up value
 * (no silent stubs). A real program's synthesised symbol overrides these
 * through `weak`.
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

static void ruxen_repl_missing_dispatch(const char *name) {
    fprintf(stderr,
            "ruxen repl: %s is not available in the REPL "
            "(dynamic dispatch helpers are synthesised only by AOT builds)\n",
            name);
    abort();
}

__attribute__((weak)) int64_t Future_dynamic_poll(int64_t self, int64_t ctx) {
    (void)self;
    (void)ctx;
    ruxen_repl_missing_dispatch("Future_dynamic_poll");
    return 0; /* unreachable */
}

__attribute__((weak)) void Drop_dynamic_drop(int64_t self) {
    (void)self;
    ruxen_repl_missing_dispatch("Drop_dynamic_drop");
}
