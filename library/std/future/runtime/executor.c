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
 * Context.executor (which routes here) calls `ruxen_reactor_acquire`
 * to install it, and Context.drop (also routed here) calls
 * `ruxen_reactor_release` to tear it down. The existing
 * `Thread.yield_now` AST emission in the block_on rewriter calls
 * into reactor.c::ruxen_reactor_park_current via the time.c
 * `ruxen_thread_yield` symbol — no compiler change needed in 4A.
 *
 * What this file owns:
 *   1. `ruxen_executor_make_context` — heap-allocate a Context
 *      with the singleton no-op Waker + install the per-thread
 *      reactor.
 *   2. `ruxen_executor_context_drop` — `def drop` on Context (lib
 *      decl in library/std/future/src/lib.rx). Releases the
 *      per-thread reactor + lets MIR's standard `ruxen_dealloc`
 *      free the Context.
 *   3. `ruxen_context_waker` — read the waker slot.
 *   4. `ruxen_context_test_dummy` — test-only Context (does NOT
 *      install a reactor; tests that exercise reactor-aware
 *      futures use `ruxen_executor_make_context` instead).
 *   5. `ruxen_waker_wake` / `ruxen_waker_wake_by_ref` — mark the
 *      owning task ready, then poke the reactor wake fd.
 *
 * ABI:
 *   - Context is a heap pointer passed as i64 (Ruxen's class-value ABI).
 *   - struct RuxenContext layout is private to this C runtime.
 *   - `&Waker` is the same pointer at the i64 ABI.
 */

#include <stdlib.h>
#include <stdint.h>

/* RuxenContext layout, v1 (real-waker round):
 *
 *   offset 0:  waker_ptr  — points to `inline_waker` inside
 *                            the same heap block. Returned by
 *                            `ruxen_context_waker(self)`.
 *   offset 8:  inline_waker — the Waker's private payload:
 *                            `(reactor_ptr, task_entry_ptr)`.
 *
 * Inlining the waker in the same allocation keeps Context construction
 * at a single malloc. The waker pointer's stability requirement is met
 * because the Context heap block doesn't move.
 *
 * Test contexts allocate the same layout with NULL payload fields, so
 * `wake` becomes a no-op without a separate "is this a test waker?"
 * branch. */
#ifndef RUXEN_WAKER_CELL_DEFINED
#define RUXEN_WAKER_CELL_DEFINED
typedef struct {
    void *reactor;
    void *task;
} RuxenWakerCell;
#endif

typedef struct {
    void *waker;
    RuxenWakerCell inline_waker;
} RuxenContext;

/* Forward decls into reactor.c. */
struct RuxenReactor;
struct RuxenReactor *ruxen_reactor_acquire(void);
void ruxen_reactor_release(void);
void ruxen_reactor_wake(int64_t reactor_handle);
void ruxen_executor_wake_task(void *task_entry);
void *ruxen_executor_make_task_context(void *task_entry);

/* Allocate a Context. The per-thread reactor is NOT eagerly created
 * here — it's lazy-allocated by `ruxen_reactor_acquire` only when
 * a reactor-aware future first registers an event. This keeps the
 * cost of block_on over a trivial (no-I/O) future at the same level
 * it was at sub-phase 3: a single 8-byte Context heap allocation +
 * one no-op acquire/release pair via Context.executor / .drop.
 *
 * Spec B3 says the reactor "is lazily constructed on first
 * block_on" — interpreted here as "lazily constructed on first
 * registration via reactor.c::ruxen_reactor_acquire". The
 * difference is invisible to user code (Context.executor is a
 * conceptual ownership root either way), and the lazy form
 * avoids opening an epoll fd / kqueue fd for every block_on that
 * doesn't touch async I/O. */
void *ruxen_executor_make_context(void) {
    return ruxen_executor_make_task_context((void *)0);
}

void *ruxen_executor_make_task_context(void *task_entry) {
    RuxenContext *cx = (RuxenContext *)malloc(sizeof(RuxenContext));
    if (!cx) {
        ruxen_panic("ruxen_executor: failed to allocate Context");
        return (void *)0;
    }
    /* Acquire (lazy-create) the per-thread reactor so the waker has a
     * stable reactor pointer to signal. The reactor lives for the
     * thread's lifetime; the Context only borrows it. */
    struct RuxenReactor *r = ruxen_reactor_acquire();
    cx->inline_waker.reactor = (void *)r;
    cx->inline_waker.task = task_entry;
    cx->waker = &cx->inline_waker;
    return cx;
}

void *ruxen_context_waker(void *cx) {
    if (!cx) {
        ruxen_panic("ruxen_context_waker: null context");
        return (void *)0;
    }
    return ((RuxenContext *)cx)->waker;
}

void *ruxen_context_clone(void *cx_opaque) {
    if (!cx_opaque) {
        ruxen_panic("ruxen_context_clone: null context");
        return (void *)0;
    }

    RuxenContext *src = (RuxenContext *)cx_opaque;
    RuxenContext *dst = (RuxenContext *)malloc(sizeof(RuxenContext));
    if (!dst) {
        ruxen_panic("ruxen_context_clone: failed to allocate Context");
        return (void *)0;
    }

    dst->inline_waker.reactor = src->inline_waker.reactor;
    dst->inline_waker.task = src->inline_waker.task;
    dst->waker = &dst->inline_waker;
    return dst;
}

void *ruxen_context_test_dummy(void) {
    RuxenContext *cx = (RuxenContext *)malloc(sizeof(RuxenContext));
    if (!cx) {
        return (void *)0;
    }
    /* No reactor install: test_dummy contexts are for the
     * async_lowering pin tests that hand-drive poll without ever
     * touching the reactor. The waker still has the same shape, but
     * its reactor slot is NULL — wake() becomes a no-op without a
     * separate "is_test" branch. */
    cx->inline_waker.reactor = NULL;
    cx->inline_waker.task = NULL;
    cx->waker = &cx->inline_waker;
    return cx;
}

void ruxen_waker_test_noop(void *waker) {
    (void)waker;
}

/* Waker.wake / wake_by_ref — real implementation.
 *
 * The waker pointer points at a `RuxenWakerCell` inlined in the owning
 * Context. For spawned tasks, `task` points at that task's scheduler
 * entry; waking marks only that entry ready. The reactor wake remains
 * wake-fd based so a parked worker returns from epoll_wait / kevent.
 *
 * v1 takes wake-by-value and wake-by-ref to the same path — neither
 * consumes ownership in any visible way (no refcount). Differentiated
 * for ABI futureproofing when wakers become heap-allocated refcounted
 * cells in v2 (per-task wakers + selective ready-queue routing). */
void ruxen_waker_wake(void *waker) {
    if (!waker) {
        return;
    }
    RuxenWakerCell *cell = (RuxenWakerCell *)waker;
    if (cell->task) {
        ruxen_executor_wake_task(cell->task);
    }
    if (cell->reactor) {
        ruxen_reactor_wake((int64_t)(uintptr_t)cell->reactor);
    }
}

void ruxen_waker_wake_by_ref(void *waker) {
    ruxen_waker_wake(waker);
}

/* `def drop` for Context (wired via lib decl in
 * library/std/future/src/lib.rx). Releases the per-thread reactor.
 * The Context heap allocation itself is freed by the standard
 * `ruxen_dealloc` call that MIR drop elaboration emits AFTER this
 * method returns — touching `cx_opaque` past this point is a
 * use-after-free, so we deliberately do not free it here.
 */
void ruxen_executor_context_drop(void *cx_opaque) {
    (void)cx_opaque;
    /* INTENTIONALLY does NOT call ruxen_reactor_release().
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
