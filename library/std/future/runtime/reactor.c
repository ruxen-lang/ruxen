#include "../../core/runtime/runtime.h"

/* Sub-phase 4A of the async round (docs/specs/stdlib/async_io.spec.md):
 * OS event reactor backing the single-threaded executor.
 *
 * Architecture:
 *
 *   - One reactor per Riven thread, held in a `__thread`-storage
 *     pointer. `block_on(...)` (via `Context.executor`) calls
 *     `riven_reactor_acquire` which lazily creates the reactor and
 *     installs it as the current-thread pointer. `Context.drop`
 *     calls `riven_reactor_release` which closes the reactor fd and
 *     clears the thread pointer.
 *
 *   - The thread-local approach removes the need for futures to
 *     thread the reactor pointer through every `poll(cx)` call;
 *     reactor-aware futures (TimeSleepFuture, AsyncFile,
 *     AsyncTcpStream) read the current reactor via
 *     `riven_reactor_current_handle` from within poll. This also
 *     means `Thread.yield_now`'s C body (riven_thread_yield in
 *     time.c) can transparently switch from `sched_yield` to a
 *     reactor wait when registrations are pending — the AST-level
 *     block_on rewriter's existing emission unchanged.
 *
 * Public Riven-callable surface (declared in
 * library/std/time/src/lib.rvn as free fns; future 4B/4C may add
 * file/socket variants):
 *
 *   riven_reactor_register_timer(reactor, nanos)  -> handle
 *   riven_reactor_check_fired(reactor, handle)    -> 0 / 1
 *   riven_reactor_deregister(reactor, handle)     -> ()
 *
 * Internal Riven-callable surface (declared in
 * library/std/future/src/lib.rvn lib decls on Context):
 *
 *   riven_executor_make_context()                 -> ctx*
 *   riven_executor_context_drop(ctx)              -> ()
 *
 * Cross-TU C-only surface (called by the time package's
 * runtime/time.c::riven_thread_yield, and by futures resolved via
 * the reactor.c free-fn lib decls in time/src/lib.rvn):
 *
 *   riven_reactor_current_handle()                -> i64
 *   riven_reactor_park_current()                  -> ()  (yield-or-wait)
 *
 * Single-threaded v1: no locking. The reactor is invoked from exactly
 * one Riven thread per block_on per the executor's contract. Each
 * Riven thread that calls block_on gets its own reactor.
 *
 * Platform support: macOS (kqueue) + Linux (epoll). Windows IOCP is
 * v2 (see spec B-Y).
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#if defined(__APPLE__)
#  include <sys/event.h>
#  include <sys/types.h>
#  include <sys/time.h>
#elif defined(__linux__)
#  include <sys/epoll.h>
#  include <sys/timerfd.h>
#else
#  error "riven reactor: unsupported platform (only Linux + macOS in v1; see async_io.spec.md B-Y)"
#endif

/* ---------------------------------------------------------------------
 * Reactor layout
 * ---------------------------------------------------------------------
 *
 * Linux: one epoll fd. Each timer registration owns its own timerfd
 * which is added to the epoll set with EPOLLIN. The handle returned to
 * Riven IS the timerfd, so deregister/check_fired can look up the fd
 * directly.
 *
 * macOS: one kqueue fd. Each timer is a EVFILT_TIMER filter with a
 * synthesized 64-bit ident (we use a monotonically increasing counter).
 * Because kevent doesn't keep a "did it fire?" bit for us, we maintain
 * a small dynamic array of (ident, fired) pairs that the wait + check
 * loops update.
 */

typedef struct RivenReactor {
    int fd;
    int registered_count;
#if defined(__APPLE__)
    int64_t next_ident;
    int slots_len;
    int slots_cap;
    struct {
        int64_t ident; /* 0 = free slot */
        int fired;
    } *slots;
#endif
} RivenReactor;

/* Per-thread current reactor pointer. Set by riven_executor_make_context
 * (via riven_reactor_acquire), cleared by riven_executor_context_drop
 * (via riven_reactor_release). Read by riven_thread_yield in time.c
 * and by the TimeSleepFuture.poll method via
 * riven_reactor_current_handle. */
static _Thread_local RivenReactor *t_current_reactor = (RivenReactor *)0;

static RivenReactor *riven_reactor_alloc_struct(void) {
    RivenReactor *r = (RivenReactor *)malloc(sizeof(RivenReactor));
    if (!r) {
        riven_panic("riven_reactor: malloc(RivenReactor) failed");
        return NULL;
    }
    memset(r, 0, sizeof(*r));

#if defined(__APPLE__)
    r->fd = kqueue();
    if (r->fd < 0) {
        free(r);
        riven_panic("riven_reactor: kqueue() failed");
        return NULL;
    }
    r->next_ident = 1;
#elif defined(__linux__)
    r->fd = epoll_create1(EPOLL_CLOEXEC);
    if (r->fd < 0) {
        free(r);
        riven_panic("riven_reactor: epoll_create1() failed");
        return NULL;
    }
#endif
    return r;
}

static void riven_reactor_free_struct(RivenReactor *r) {
    if (!r) {
        return;
    }
    if (r->fd >= 0) {
        close(r->fd);
        r->fd = -1;
    }
#if defined(__APPLE__)
    if (r->slots) {
        free(r->slots);
        r->slots = NULL;
    }
#endif
    free(r);
}

/* Acquire (or lazily create) the current-thread reactor. Returns the
 * reactor pointer. Called by `Context.executor` to install a fresh
 * reactor for this block_on entry. Nested block_on within the same
 * thread reuses the existing reactor — matches spec B3 "reactor lives
 * per thread; one per block_on call gets the same reactor". */
RivenReactor *riven_reactor_acquire(void) {
    if (!t_current_reactor) {
        t_current_reactor = riven_reactor_alloc_struct();
    }
    return t_current_reactor;
}

/* Release the current-thread reactor. Called by `Context.drop` at
 * scope exit. */
void riven_reactor_release(void) {
    if (t_current_reactor) {
        riven_reactor_free_struct(t_current_reactor);
        t_current_reactor = NULL;
    }
}

/* Return the current-thread reactor as an i64 handle for Riven side.
 * Returns 0 if no reactor is installed (e.g. test_dummy contexts or
 * code outside any block_on call). The reactor-aware future uses
 * this in poll() rather than threading a pointer through cx. */
int64_t riven_reactor_current_handle(void) {
    return (int64_t)(uintptr_t)t_current_reactor;
}

#if defined(__APPLE__)
static int riven_reactor_alloc_slot(RivenReactor *r) {
    for (int i = 0; i < r->slots_len; i++) {
        if (r->slots[i].ident == 0) {
            return i;
        }
    }
    if (r->slots_len == r->slots_cap) {
        int new_cap = r->slots_cap == 0 ? 8 : r->slots_cap * 2;
        void *new_slots = realloc(r->slots, (size_t)new_cap * sizeof(*r->slots));
        if (!new_slots) {
            riven_panic("riven_reactor: realloc(slots) failed");
            return -1;
        }
        r->slots = new_slots;
        memset(&r->slots[r->slots_cap], 0,
               (size_t)(new_cap - r->slots_cap) * sizeof(*r->slots));
        r->slots_cap = new_cap;
    }
    return r->slots_len++;
}

static int riven_reactor_find_slot(RivenReactor *r, int64_t ident) {
    if (ident == 0) {
        return -1;
    }
    for (int i = 0; i < r->slots_len; i++) {
        if (r->slots[i].ident == ident) {
            return i;
        }
    }
    return -1;
}
#endif

/* Register a one-shot timer. Returns an opaque i64 handle that
 * `check_fired` and `deregister` accept. `reactor` is the i64-cast
 * pointer returned by `riven_reactor_current_handle`; if 0 is
 * passed, the function lazy-acquires the current-thread reactor
 * (matches the v1 spec — futures don't need to thread the handle
 * themselves; passing 0 means "use whatever reactor this thread
 * has, allocating if needed"). */
int64_t riven_reactor_register_timer(int64_t reactor_handle, int64_t nanos) {
    RivenReactor *r;
    if (reactor_handle == 0) {
        r = riven_reactor_acquire();
    } else {
        r = (RivenReactor *)(uintptr_t)reactor_handle;
    }
    if (nanos < 0) {
        nanos = 0;
    }

#if defined(__APPLE__)
    int slot = riven_reactor_alloc_slot(r);
    if (slot < 0) {
        return 0;
    }
    int64_t ident = r->next_ident++;
    r->slots[slot].ident = ident;
    r->slots[slot].fired = 0;

    struct kevent ev;
    EV_SET(&ev, (uintptr_t)ident, EVFILT_TIMER,
           EV_ADD | EV_ENABLE | EV_ONESHOT,
           NOTE_NSECONDS, (intptr_t)nanos, NULL);
    if (kevent(r->fd, &ev, 1, NULL, 0, NULL) < 0) {
        r->slots[slot].ident = 0;
        riven_panic("riven_reactor_register_timer: kevent EV_ADD failed");
        return 0;
    }
    r->registered_count++;
    return ident;
#elif defined(__linux__)
    int tfd = timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC | TFD_NONBLOCK);
    if (tfd < 0) {
        riven_panic("riven_reactor_register_timer: timerfd_create failed");
        return 0;
    }
    struct itimerspec spec;
    memset(&spec, 0, sizeof(spec));
    spec.it_value.tv_sec = (time_t)(nanos / 1000000000LL);
    spec.it_value.tv_nsec = (long)(nanos % 1000000000LL);
    if (spec.it_value.tv_sec == 0 && spec.it_value.tv_nsec == 0) {
        spec.it_value.tv_nsec = 1;
    }
    if (timerfd_settime(tfd, 0, &spec, NULL) < 0) {
        close(tfd);
        riven_panic("riven_reactor_register_timer: timerfd_settime failed");
        return 0;
    }
    struct epoll_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.events = EPOLLIN;
    ev.data.fd = tfd;
    if (epoll_ctl(r->fd, EPOLL_CTL_ADD, tfd, &ev) < 0) {
        close(tfd);
        riven_panic("riven_reactor_register_timer: epoll_ctl ADD failed");
        return 0;
    }
    r->registered_count++;
    return (int64_t)tfd;
#endif
}

int64_t riven_reactor_check_fired(int64_t reactor_handle, int64_t handle) {
    if (reactor_handle == 0 || handle == 0) {
        return 0;
    }
    RivenReactor *r = (RivenReactor *)(uintptr_t)reactor_handle;

#if defined(__APPLE__)
    int slot = riven_reactor_find_slot(r, handle);
    if (slot < 0) {
        return 0;
    }
    return r->slots[slot].fired ? 1 : 0;
#elif defined(__linux__)
    int tfd = (int)handle;
    uint64_t expirations = 0;
    ssize_t n = read(tfd, &expirations, sizeof(expirations));
    if (n == (ssize_t)sizeof(expirations) && expirations > 0) {
        return 1;
    }
    return 0;
#endif
}

void riven_reactor_deregister(int64_t reactor_handle, int64_t handle) {
    if (reactor_handle == 0 || handle == 0) {
        return;
    }
    RivenReactor *r = (RivenReactor *)(uintptr_t)reactor_handle;

#if defined(__APPLE__)
    int slot = riven_reactor_find_slot(r, handle);
    if (slot < 0) {
        return;
    }
    struct kevent ev;
    EV_SET(&ev, (uintptr_t)handle, EVFILT_TIMER, EV_DELETE, 0, 0, NULL);
    (void)kevent(r->fd, &ev, 1, NULL, 0, NULL);
    r->slots[slot].ident = 0;
    r->slots[slot].fired = 0;
    if (r->registered_count > 0) {
        r->registered_count--;
    }
#elif defined(__linux__)
    int tfd = (int)handle;
    (void)epoll_ctl(r->fd, EPOLL_CTL_DEL, tfd, NULL);
    close(tfd);
    if (r->registered_count > 0) {
        r->registered_count--;
    }
#endif
}

/* Park the current thread on the current-thread reactor's wait point.
 * Called by `riven_thread_yield` in time.c — the existing
 * `Thread.yield_now` AST emission in the block_on rewriter routes
 * through here transparently when there's a reactor + registrations.
 *
 * Behaviour:
 *   - No reactor installed on this thread → sched_yield (the
 *     pre-sub-phase-4A behaviour).
 *   - Reactor installed but no live registrations → sched_yield
 *     (chained-await futures that yield Pending between states
 *     without I/O — fixtures 723 / 724 hit this path).
 *   - Reactor installed and at least one registration → block on
 *     epoll_wait / kevent until at least one event fires.
 *
 * Marks fired registrations on macOS (kqueue doesn't keep a
 * "did it fire" bit, so we maintain that bookkeeping ourselves).
 * On Linux fired-ness is observed at check_fired time via read()
 * on the timerfd. */
void riven_reactor_park_current(void) {
    if (!t_current_reactor || t_current_reactor->registered_count <= 0) {
        sched_yield();
        return;
    }
    RivenReactor *r = t_current_reactor;

#if defined(__APPLE__)
    struct kevent events[16];
    int n = kevent(r->fd, NULL, 0, events, 16, NULL);
    if (n < 0) {
        return;
    }
    for (int i = 0; i < n; i++) {
        int64_t ident = (int64_t)events[i].ident;
        int slot = riven_reactor_find_slot(r, ident);
        if (slot >= 0) {
            r->slots[slot].fired = 1;
        }
        if (r->registered_count > 0) {
            r->registered_count--;
        }
    }
#elif defined(__linux__)
    struct epoll_event events[16];
    int n = epoll_wait(r->fd, events, 16, -1);
    if (n < 0) {
        return;
    }
    /* Linux fired-ness is observed via the timerfd's read() in
     * check_fired; nothing to update here. */
    (void)events;
#endif
}
