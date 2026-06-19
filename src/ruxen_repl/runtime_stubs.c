/* Weak stubs for symbols that `library/std/future/runtime/scheduler.c`
 * forward-declares and the AOT Cranelift backend would normally emit
 * (`synthesize_dynamic_dispatch_helpers`). The REPL's libruxenrt.a
 * link doesn't go through Ruxen codegen, so the symbol is missing and
 * the link fails with
 *   "_Future_dynamic_poll referenced from _ruxen_executor_pump_tasks".
 *
 * The stub returns 0 (Poll::Pending tag at the i64 ABI). The REPL
 * doesn't execute the executor's pump path, so the stub never runs in
 * practice — it just needs to exist as a link target. If/when a Ruxen
 * program *does* define the symbol (e.g. via a more capable JIT
 * lowering of class includes Future dispatch runtime), the weak
 * attribute lets it override this default.
 */
#include <stdint.h>

__attribute__((weak)) int64_t Future_dynamic_poll(int64_t self, int64_t ctx) {
    (void)self;
    (void)ctx;
    return 0;
}

/* Likewise `Drop_dynamic_drop` — the per-class drop dispatcher the AOT
 * backend synthesises for the narrow Drop vtable (concurrency §4.1). The
 * future runtime now calls it: scheduler.c (drain/cancel), schedule.c
 * (retry child teardown), stream.c (adapter-chain inner drop). The REPL
 * link doesn't go through Ruxen codegen, so it needs a weak no-op target;
 * a real Ruxen program's synthesised symbol overrides it via `weak`. */
__attribute__((weak)) void Drop_dynamic_drop(int64_t self) {
    (void)self;
}
