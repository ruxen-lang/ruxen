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

/* Test-only Context constructor (async_lowering.spec.md B6 —
 * Milestone 2A). The state-machine pin tests need a Context value
 * they can pass into `(&var fut).poll(&var ctx)` without
 * panicking. The real executor pairs Context with the wake queue +
 * task table; for the test dummy we just hand back a malloced
 * sentinel pointer — the no-await Milestone 2A poll never inspects
 * the cx, so the sentinel's contents are irrelevant. Sub-phase 3
 * will replace this with a real executor-owned Context.
 *
 * The pointer is intentionally leaked (no matching free): Riven
 * doesn't yet have a Drop impl on Context, and the test programs
 * are short-lived so the OS reclaims at exit. Sub-phase 3 wires
 * the lifecycle correctly. */
#include <stdlib.h>

void *riven_context_test_dummy(void) {
    void *p = malloc(8);
    /* Zero-fill so any (mis-)deref by buggy poll code lands at a
     * stable NULL-page-ish value rather than uninitialised garbage. */
    if (p) {
        *(long *)p = 0;
    }
    return p;
}

/* No-op waker for the test-dummy context. NOT wired up to
 * `riven_context_waker` yet — that still panics. Sub-phase 3
 * replaces both. */
void riven_waker_test_noop(void *waker) {
    (void)waker;
}

void riven_waker_wake(void *waker) {
    (void)waker;
    riven_panic("executor not yet implemented; lands in async sub-phase 3");
}

void riven_waker_wake_by_ref(void *waker) {
    (void)waker;
    riven_panic("executor not yet implemented; lands in async sub-phase 3");
}
