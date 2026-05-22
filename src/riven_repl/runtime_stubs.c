/* Weak stubs for symbols that `library/std/future/runtime/scheduler.c`
 * forward-declares and the AOT Cranelift backend would normally emit
 * (`synthesize_dynamic_dispatch_helpers`). The REPL's librivenrt.a
 * link doesn't go through Riven codegen, so the symbol is missing and
 * the link fails with
 *   "_Future_dynamic_poll referenced from _riven_executor_pump_tasks".
 *
 * The stub returns 0 (Poll::Pending tag at the i64 ABI). The REPL
 * doesn't execute the executor's pump path, so the stub never runs in
 * practice — it just needs to exist as a link target. If/when a Riven
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
