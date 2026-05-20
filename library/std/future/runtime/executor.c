#include "../../core/runtime/runtime.h"

/* Sub-phase 1 stub runtime for std::future.
 *
 * Each function below corresponds to a Riven-level method on Context
 * or Waker (see library/std/future/src/lib.rvn). The real
 * implementations land in sub-phase 3 alongside `block_on` and the
 * single-threaded cooperative executor. For now every entry point
 * calls `riven_panic` with a stable message so any program that
 * exercises the surface fails loudly with a clear pointer to the
 * unimplemented sub-phase.
 *
 * The C ABI shapes match the Riven-side lib decls:
 *   - Context / Waker are opaque pointer-shaped values at the i64
 *     ABI (same convention as Mutex, SharedSync, etc. — see
 *     library/std/sync/runtime/mutex.c).
 *   - `&Waker` is also a pointer; Riven's `&T` lowers to the same
 *     ABI as `T` for class types in v1 (see typed-FFI returns spec).
 * Sub-phase 3 may change the wire layout once the executor data is
 * defined; user code is shielded from that by the class abstraction.
 */

void *riven_context_waker(void *cx) {
    (void)cx;
    riven_panic("executor not yet implemented; lands in async sub-phase 3");
    return (void *)0; /* unreachable; quiets the compiler */
}

void riven_waker_wake(void *waker) {
    (void)waker;
    riven_panic("executor not yet implemented; lands in async sub-phase 3");
}

void riven_waker_wake_by_ref(void *waker) {
    (void)waker;
    riven_panic("executor not yet implemented; lands in async sub-phase 3");
}
