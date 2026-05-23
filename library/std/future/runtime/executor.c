#include "../../core/runtime/runtime.h"

/* Sub-phase 3 + 4A of the async round (docs/specs/stdlib/executor.spec.md,
 * docs/specs/stdlib/async_io.spec.md):
 * single-threaded block_on executor + OS event reactor wiring.
 *
 * Sub-phase 1 shipped panic-stubs; sub-phase 3 made the Waker /
 * Context surface real (no-op signals + a singleton waker). Sub-phase
 * 4A wires the per-thread reactor into Context construction +
 * destruction. The reactor itself lives in
 * `library/std/future/runtime/reactor.c` as a thread-local pointer;
 * Context.executor (which routes here) calls `riven_reactor_acquire`
 * to install it, and Context.drop (also routed here) calls
 * `riven_reactor_release` to tear it down. The existing
 * `Thread.yield_now` AST emission in the block_on rewriter calls
 * into reactor.c::riven_reactor_park_current via the time.c
 * `riven_thread_yield` symbol — no compiler change needed in 4A.
 *
 * What this file owns:
 *   1. `riven_executor_make_context` — heap-allocate a Context
 *      with the singleton no-op Waker + install the per-thread
 *      reactor.
 *   2. `riven_executor_context_drop` — `def drop` on Context (lib
 *      decl in library/std/future/src/lib.rvn). Releases the
 *      per-thread reactor + lets MIR's standard `riven_dealloc`
 *      free the Context.
 *   3. `riven_context_waker` — read the waker slot.
 *   4. `riven_context_test_dummy` — test-only Context (does NOT
 *      install a reactor; tests that exercise reactor-aware
 *      futures use `riven_executor_make_context` instead).
 *   5. `riven_waker_wake` / `riven_waker_wake_by_ref` — no-ops in v1.
 *
 * ABI:
 *   - Context is a heap pointer passed as i64 (Riven's class-value ABI).
 *   - struct RivenContext layout: { void *waker; }  (unchanged from
 *     sub-phase 3 — the reactor is thread-local, not Context-local).
 *   - `&Waker` is the same pointer at the i64 ABI.
 */

#include <stdlib.h>
#include <stdint.h>

/* RivenContext layout, v1 (real-waker round):
 *
 *   offset 0:  waker_ptr  — points to (&inline_waker_reactor) inside
 *                            the same heap block. Returned by
 *                            `riven_context_waker(self)`.
 *   offset 8:  inline_waker_reactor — the Waker's payload: the i64
 *                            cast of the reactor pointer to wake.
 *                            Riven's Waker.wake calls
 *                            `riven_waker_wake(self_waker_ptr)` which
 *                            dereferences this slot.
 *
 * Inlining the waker in the same allocation keeps Context construction
 * at a single malloc (was: 1 malloc for the Context + 1 lookup for the
 * singleton no-op waker; now: 1 malloc for the combined 16-byte block).
 * The waker pointer's stability requirement is met because the Context
 * heap block doesn't move — we only ever return `&cx->inline_...` for
 * the lifetime of the Context.
 *
 * The waker's interpretation as "a pointer into the inline slot" is
 * symmetric across `riven_waker_wake(waker_ptr)` — `*(void **)waker_ptr`
 * yields the reactor pointer to signal. Test contexts allocate the
 * same layout with `inline_waker_reactor = NULL`, so `wake` becomes
 * a no-op without a separate "is this a real waker?" branch. */
typedef struct {
    void *waker;
    void *inline_waker_reactor;
} RivenContext;

/* Forward decls into reactor.c. */
struct RivenReactor;
struct RivenReactor *riven_reactor_acquire(void);
void riven_reactor_release(void);
void riven_reactor_wake(int64_t reactor_handle);

/* Allocate a Context. The per-thread reactor is NOT eagerly created
 * here — it's lazy-allocated by `riven_reactor_acquire` only when
 * a reactor-aware future first registers an event. This keeps the
 * cost of block_on over a trivial (no-I/O) future at the same level
 * it was at sub-phase 3: a single 8-byte Context heap allocation +
 * one no-op acquire/release pair via Context.executor / .drop.
 *
 * Spec B3 says the reactor "is lazily constructed on first
 * block_on" — interpreted here as "lazily constructed on first
 * registration via reactor.c::riven_reactor_acquire". The
 * difference is invisible to user code (Context.executor is a
 * conceptual ownership root either way), and the lazy form
 * avoids opening an epoll fd / kqueue fd for every block_on that
 * doesn't touch async I/O. */
void *riven_executor_make_context(void) {
    RivenContext *cx = (RivenContext *)malloc(sizeof(RivenContext));
    if (!cx) {
        riven_panic("riven_executor: failed to allocate Context");
        return (void *)0;
    }
    /* Acquire (lazy-create) the per-thread reactor so the waker has a
     * stable reactor pointer to signal. The reactor lives for the
     * thread's lifetime; the Context only borrows it. */
    struct RivenReactor *r = riven_reactor_acquire();
    cx->inline_waker_reactor = (void *)r;
    cx->waker = &cx->inline_waker_reactor;
    return cx;
}

void *riven_context_waker(void *cx) {
    if (!cx) {
        riven_panic("riven_context_waker: null context");
        return (void *)0;
    }
    return ((RivenContext *)cx)->waker;
}

void *riven_context_test_dummy(void) {
    RivenContext *cx = (RivenContext *)malloc(sizeof(RivenContext));
    if (!cx) {
        return (void *)0;
    }
    /* No reactor install: test_dummy contexts are for the
     * async_lowering pin tests that hand-drive poll without ever
     * touching the reactor. The waker still has the same shape, but
     * its reactor slot is NULL — wake() becomes a no-op without a
     * separate "is_test" branch. */
    cx->inline_waker_reactor = NULL;
    cx->waker = &cx->inline_waker_reactor;
    return cx;
}

void riven_waker_test_noop(void *waker) {
    (void)waker;
}

/* Waker.wake / wake_by_ref — real implementation.
 *
 * The waker pointer points at a `void *reactor` slot (inlined in the
 * owning Context for executor wakers, or NULL for test_dummy wakers).
 * Dereference and route to the reactor. Cross-thread safe: the underlying
 * eventfd_write / kevent NOTE_TRIGGER are kernel-side atomic.
 *
 * v1 takes wake-by-value and wake-by-ref to the same path — neither
 * consumes ownership in any visible way (no refcount). Differentiated
 * for ABI futureproofing when wakers become heap-allocated refcounted
 * cells in v2 (per-task wakers + selective ready-queue routing). */
void riven_waker_wake(void *waker) {
    if (!waker) {
        return;
    }
    void *reactor = *(void **)waker;
    if (!reactor) {
        return;
    }
    riven_reactor_wake((int64_t)(uintptr_t)reactor);
}

void riven_waker_wake_by_ref(void *waker) {
    if (!waker) {
        return;
    }
    void *reactor = *(void **)waker;
    if (!reactor) {
        return;
    }
    riven_reactor_wake((int64_t)(uintptr_t)reactor);
}

/* `def drop` for Context (wired via lib decl in
 * library/std/future/src/lib.rvn). Releases the per-thread reactor.
 * The Context heap allocation itself is freed by the standard
 * `riven_dealloc` call that MIR drop elaboration emits AFTER this
 * method returns — touching `cx_opaque` past this point is a
 * use-after-free, so we deliberately do not free it here.
 */
void riven_executor_context_drop(void *cx_opaque) {
    (void)cx_opaque;
    /* INTENTIONALLY does NOT call riven_reactor_release().
     *
     * The reactor is a per-thread resource — its kqueue/epoll fd is
     * meant to live for the lifetime of the worker thread, not for
     * the lifetime of one Context. Releasing on every Context.drop
     * caused a kqueue() open+close pair per `block_on` call. Under
     * load that was ~3 block_on per HTTP request (read/write/close)
     * × N RPS per worker, e.g. ~18,000 kqueue create+destroy per
     * second per worker at 6k RPS. On macOS that fd churn eventually
     * trips `kqueue()` into returning -1 (transient EMFILE-style
     * exhaustion as the kernel reaps closed kqueues lazily), which
     * panicked rondo's async-multi server mid-bench.
     *
     * Cost of NOT releasing: any thread that calls `block_on(...)`
     * once and never again leaks one kqueue (or epoll) fd until
     * thread exit. For server workers that loop forever this is a
     * no-op; for a script doing one block_on at startup it's a
     * single-fd leak for the program's lifetime. Net benefit
     * dwarfs net cost. Proper thread-exit cleanup would use
     * pthread_key_create + a destructor; deferred until we hit
     * a workload that actually needs it (long-lived programs
     * spawning many short-lived threads each calling block_on
     * once). */
}
