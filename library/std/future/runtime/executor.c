#include "../../core/runtime/runtime.h"

/* Sub-phase 3 of the async round (docs/specs/stdlib/executor.spec.md):
 * single-threaded block_on executor.
 *
 * Sub-phase 1 shipped panic-stubs for every entry; sub-phase 3 makes
 * Waker.wake / wake_by_ref no-ops (the poll loop spins, so wake is
 * implicit) and `Context.waker` returns a real singleton no-op
 * Waker. The actual `block_on` is a compiler intrinsic — see
 * library/std/executor/src/lib.rvn and the async_lowering pass — so
 * there is NO `riven_executor_block_on` C symbol; the poll loop is
 * inlined at every call site against the future's concrete
 * `<__FooFuture>_poll` symbol (matches how `.await` already
 * dispatches).
 *
 * What the C side owns in sub-phase 3:
 *   1. `riven_executor_make_context` — heap-allocate a Context
 *      whose `.waker` slot points at the singleton no-op Waker.
 *      The block_on intrinsic calls this once per loop entry.
 *   2. `riven_context_waker` — read the waker slot. Returns the
 *      same singleton for test_dummy and executor-made contexts.
 *   3. `riven_context_test_dummy` — unchanged surface for the
 *      hand-driven poll tests in async_lowering (Milestone 2A).
 *      Backfilled so its waker slot is also the singleton, so
 *      `cx.waker()` works on test_dummy values too.
 *   4. `riven_waker_wake` / `riven_waker_wake_by_ref` — no-ops.
 *      Sub-phase 4 replaces with real signal-to-park machinery.
 *
 * ABI:
 *   - Context and Waker are heap pointers passed as i64 (Riven's
 *     class-value ABI). The first 8 bytes of a Context are the
 *     waker pointer (`struct RivenContext { void *waker; }`).
 *   - `&Waker` is the same pointer at the i64 ABI (Riven's `&T`
 *     lowering matches `T` for class types in v1).
 *
 * The C structs are intentionally untyped (`void *` slots) so user
 * code never depends on their layout. The compiler intrinsic is the
 * only consumer; if the layout changes, only the intrinsic and this
 * file change in lockstep.
 */

#include <stdlib.h>

/* RivenContext layout: first slot is the waker pointer. */
typedef struct {
    void *waker;
} RivenContext;

/* Singleton no-op Waker. Allocated lazily on first use. Lives for
 * the lifetime of the process (never freed) — sub-phase 4 introduces
 * a real Waker type with proper lifecycle. */
static void *s_noop_waker = (void *)0;

static void *get_or_init_noop_waker(void) {
    if (!s_noop_waker) {
        /* 8-byte sentinel — content doesn't matter; the waker is
         * opaque to user code and Waker.wake / wake_by_ref are
         * no-ops that don't read the value. */
        void *p = malloc(8);
        if (!p) {
            riven_panic("riven_executor: failed to allocate no-op waker");
            return (void *)0;
        }
        *(long *)p = 0;
        s_noop_waker = p;
    }
    return s_noop_waker;
}

/* Construct a new Context whose waker is the singleton no-op. Used
 * by the block_on intrinsic at the top of every poll loop. The
 * Context is heap-allocated; the block_on lowering is responsible
 * for releasing it via the standard Riven class-drop path when the
 * outer block exits (Drop spec B9). Sub-phase 3 ships without an
 * explicit free helper — Context has no Drop impl in Riven yet and
 * the per-call leak is bounded (one Context per block_on call,
 * 8 bytes each). Sub-phase 4 wires a proper lifecycle. */
void *riven_executor_make_context(void) {
    RivenContext *cx = (RivenContext *)malloc(sizeof(RivenContext));
    if (!cx) {
        riven_panic("riven_executor: failed to allocate Context");
        return (void *)0;
    }
    cx->waker = get_or_init_noop_waker();
    return cx;
}

/* Returns the Context's waker pointer. Sub-phase 1 panicked here;
 * sub-phase 3 honours the slot. Both test_dummy and executor-made
 * Contexts have the singleton stashed, so callers don't need to
 * branch. */
void *riven_context_waker(void *cx) {
    if (!cx) {
        riven_panic("riven_context_waker: null context");
        return (void *)0;
    }
    return ((RivenContext *)cx)->waker;
}

/* Test-only Context constructor (async_lowering.spec.md B6 —
 * Milestone 2A). Sub-phase 1 leaked an 8-byte zero-initialised
 * pointer; sub-phase 3 backfills the waker slot so `cx.waker()`
 * works on test_dummy contexts too. Still intentionally leaked —
 * Context has no Drop impl yet and the test programs are short-
 * lived. */
void *riven_context_test_dummy(void) {
    RivenContext *cx = (RivenContext *)malloc(sizeof(RivenContext));
    if (!cx) {
        return (void *)0;
    }
    cx->waker = get_or_init_noop_waker();
    return cx;
}

/* No-op waker for the test-dummy context. Predates sub-phase 3;
 * kept for any external caller that may still reference the symbol.
 * The singleton waker installed by get_or_init_noop_waker() is what
 * actually flows through user code now. */
void riven_waker_test_noop(void *waker) {
    (void)waker;
}

/* Sub-phase 3: Waker.wake / wake_by_ref are no-ops. The block_on
 * poll loop spins (with Thread.yield_now between iterations) so
 * any "wake" is implicit. Sub-phase 4 replaces these with real
 * pthread_cond_signal / wake-fd-write so the executor can park on
 * epoll/kqueue and resume only when I/O is ready. */
void riven_waker_wake(void *waker) {
    (void)waker;
}

void riven_waker_wake_by_ref(void *waker) {
    (void)waker;
}
