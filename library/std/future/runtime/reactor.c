#include "../../core/runtime/runtime.h"

/* Sub-phase 4A of the async round (docs/specs/stdlib/async_io.spec.md):
 * OS event reactor backing the single-threaded executor.
 *
 * Architecture:
 *
 *   - One reactor per Ruxen thread, held in a `__thread`-storage
 *     pointer. `block_on(...)` (via `Context.executor`) calls
 *     `ruxen_reactor_acquire` which lazily creates the reactor and
 *     installs it as the current-thread pointer. `Context.drop`
 *     calls `ruxen_reactor_release` which closes the reactor fd and
 *     clears the thread pointer.
 *
 *   - The thread-local approach removes the need for futures to
 *     thread the reactor pointer through every `poll(cx)` call;
 *     reactor-aware futures (TimeSleepFuture, AsyncFile,
 *     AsyncTcpStream) read the current reactor via
 *     `ruxen_reactor_current_handle` from within poll. This also
 *     means `Thread.yield_now`'s C body (ruxen_thread_yield in
 *     time.c) can transparently switch from `sched_yield` to a
 *     reactor wait when registrations are pending — the AST-level
 *     block_on rewriter's existing emission unchanged.
 *
 * Public Ruxen-callable surface (declared in
 * library/std/time/src/lib.rx as free fns; future 4B/4C may add
 * file/socket variants):
 *
 *   ruxen_reactor_register_timer(reactor, nanos)  -> handle
 *   ruxen_reactor_check_fired(reactor, handle)    -> 0 / 1
 *   ruxen_reactor_deregister(reactor, handle)     -> ()
 *
 * Internal Ruxen-callable surface (declared in
 * library/std/future/src/lib.rx lib decls on Context):
 *
 *   ruxen_executor_make_context()                 -> ctx*
 *   ruxen_executor_context_drop(ctx)              -> ()
 *
 * Cross-TU C-only surface (called by the time package's
 * runtime/time.c::ruxen_thread_yield, and by futures resolved via
 * the reactor.c free-fn lib decls in time/src/lib.rx):
 *
 *   ruxen_reactor_current_handle()                -> i64
 *   ruxen_reactor_park_current()                  -> ()  (yield-or-wait)
 *
 * Single-threaded v1: no locking. The reactor is invoked from exactly
 * one Ruxen thread per block_on per the executor's contract. Each
 * Ruxen thread that calls block_on gets its own reactor.
 *
 * Platform support: macOS (kqueue) + Linux (epoll). Windows IOCP is
 * v2 (see spec B-Y).
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

void ruxen_executor_wake_task(void *task_entry);

#if defined(__APPLE__)
#  include <sys/event.h>
#  include <sys/types.h>
#  include <sys/time.h>
#elif defined(__linux__)
#  include <sys/epoll.h>
#  include <sys/timerfd.h>
#  include <sys/eventfd.h>
#else
#  error "ruxen reactor: unsupported platform (only Linux + macOS in v1; see async_io.spec.md B-Y)"
#endif

/* ---------------------------------------------------------------------
 * Reactor layout
 * ---------------------------------------------------------------------
 *
 * Linux: one epoll fd. Each timer registration owns its own timerfd
 * which is added to the epoll set with EPOLLIN. The handle returned to
 * Ruxen IS the timerfd, so deregister/check_fired can look up the fd
 * directly.
 *
 * macOS: one kqueue fd. Each timer is a EVFILT_TIMER filter with a
 * synthesized 64-bit ident (we use a monotonically increasing counter).
 * Because kevent doesn't keep a "did it fire?" bit for us, we maintain
 * a small dynamic array of (ident, fired) pairs that the wait + check
 * loops update.
 */

/* Fd-readiness slot — used by Milestone 4B AsyncFile futures and (when
 * 4C lands) AsyncTcpStream futures. A slot ties together (user-fd,
 * read-or-write, fired-bit) so deregister can EPOLL_CTL_DEL / EV_DELETE
 * the right filter WITHOUT closing the user's fd (the future itself
 * owns the fd lifecycle).
 *
 * Handle encoding: handles for fd registrations are `-(slot_index + 1)`
 * so they live in the negative half of int64 — disjoint from timer
 * handles which are either positive timerfds (Linux) or positive
 * ident counters (macOS). `check_fired` / `deregister` route on sign. */
typedef struct RuxenReactorFdSlot {
    int fd;        /* 0 = free slot; -1 also free (we use >0 to mean live) */
    int mode;      /* 0 = read, 1 = write */
    int fired;     /* set by park_current when the OS reports readiness */
    void *task;    /* scheduler task to mark ready when this fd fires */
} RuxenReactorFdSlot;

typedef struct RuxenReactorTimerSlot {
    int64_t ident; /* macOS: synthetic ident; Linux: timerfd; 0 = free */
    int fired;
    void *task;
} RuxenReactorTimerSlot;

#ifndef RUXEN_WAKER_CELL_DEFINED
#define RUXEN_WAKER_CELL_DEFINED
typedef struct RuxenWakerCell {
    void *reactor;
    void *task;
} RuxenWakerCell;
#endif

typedef struct RuxenReactor {
    int fd;
    int registered_count;
    int fd_slots_len;
    int fd_slots_cap;
    RuxenReactorFdSlot **fd_slots;
    /* Wake fd: a single registration used by Waker.wake to unpark the
     * thread when no I/O event is in flight. Created at reactor alloc
     * time, persists for the reactor's lifetime. Linux: eventfd. macOS:
     * EVFILT_USER ident = (uintptr_t)&wake_fd_sentinel.
     *
     * Deliberately NOT counted in `registered_count`. The
     * sched_yield-fast-path in park_current still triggers when the
     * only "registration" is the wake fd — block_on of a trivial
     * non-I/O future (e.g. CountdownFuture from fixture 728) keeps the
     * sched_yield behaviour rather than parking on epoll/kevent. */
    int wake_fd;
    int wake_fd_sentinel; /* address used as udata to identify wake events */
    int64_t next_ident;
    int slots_len;
    int slots_cap;
    RuxenReactorTimerSlot *slots;
} RuxenReactor;

/* Per-thread current reactor pointer. Set by ruxen_executor_make_context
 * (via ruxen_reactor_acquire), cleared by ruxen_executor_context_drop
 * (via ruxen_reactor_release). Read by ruxen_thread_yield in time.c
 * and by the TimeSleepFuture.poll method via
 * ruxen_reactor_current_handle. */
static _Thread_local RuxenReactor *t_current_reactor = (RuxenReactor *)0;

static RuxenReactor *ruxen_reactor_alloc_struct(void) {
    RuxenReactor *r = (RuxenReactor *)malloc(sizeof(RuxenReactor));
    if (!r) {
        ruxen_panic("ruxen_reactor: malloc(RuxenReactor) failed");
        return NULL;
    }
    memset(r, 0, sizeof(*r));
    r->wake_fd = -1;

#if defined(__APPLE__)
    r->fd = kqueue();
    if (r->fd < 0) {
        free(r);
        ruxen_panic("ruxen_reactor: kqueue() failed");
        return NULL;
    }
    r->next_ident = 1;
    /* Wake fd: EVFILT_USER, ident = (uintptr_t)&r->wake_fd_sentinel.
     * EV_CLEAR auto-resets the trigger on each delivery, so park_current
     * needs no drain syscall. We use the sentinel address as a stable,
     * reactor-unique ident — distinct from timer idents (counter-based,
     * starting at 1) and fd idents (raw fd numbers, all > 0 and small).
     *
     * NOTE: we do NOT bump registered_count. The wake fd is "background
     * infrastructure" — park_current's sched_yield-when-no-real-
     * registrations fast path must still fire for block_on of a
     * non-I/O future (cf. fixture 728). */
    {
        struct kevent ev;
        EV_SET(&ev, (uintptr_t)&r->wake_fd_sentinel, EVFILT_USER,
               EV_ADD | EV_ENABLE | EV_CLEAR,
               0, 0, &r->wake_fd_sentinel);
        if (kevent(r->fd, &ev, 1, NULL, 0, NULL) < 0) {
            close(r->fd);
            free(r);
            ruxen_panic("ruxen_reactor: kevent EVFILT_USER ADD failed");
            return NULL;
        }
        /* wake_fd is unused on macOS (no fd backing for EVFILT_USER), but
         * leaving it at -1 is the sentinel "no fd to close" for the
         * free-struct path. */
    }
#elif defined(__linux__)
    r->fd = epoll_create1(EPOLL_CLOEXEC);
    if (r->fd < 0) {
        free(r);
        ruxen_panic("ruxen_reactor: epoll_create1() failed");
        return NULL;
    }
    /* Wake fd: eventfd registered with EPOLLIN, level-triggered. We use
     * the sentinel address as data.ptr so park_current can distinguish
     * wake events from fd-readiness events and timer events. Reading
     * the eventfd in park_current drains the counter to 0 — subsequent
     * wakes re-arm by writing 1. Eventfd_write is async-signal-safe
     * (single 8-byte counter add), so cross-thread wake from a future
     * stored on another thread is sound. */
    r->wake_fd = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);
    if (r->wake_fd < 0) {
        close(r->fd);
        free(r);
        ruxen_panic("ruxen_reactor: eventfd() failed");
        return NULL;
    }
    {
        struct epoll_event ev;
        memset(&ev, 0, sizeof(ev));
        ev.events = EPOLLIN;
        ev.data.ptr = &r->wake_fd_sentinel;
        if (epoll_ctl(r->fd, EPOLL_CTL_ADD, r->wake_fd, &ev) < 0) {
            close(r->wake_fd);
            close(r->fd);
            free(r);
            ruxen_panic("ruxen_reactor: epoll_ctl(wake_fd) failed");
            return NULL;
        }
    }
#endif
    return r;
}

static void ruxen_reactor_free_struct(RuxenReactor *r) {
    if (!r) {
        return;
    }
    if (r->wake_fd >= 0) {
        close(r->wake_fd);
        r->wake_fd = -1;
    }
    if (r->fd >= 0) {
        close(r->fd);
        r->fd = -1;
    }
    if (r->fd_slots) {
        for (int i = 0; i < r->fd_slots_len; i++) {
            free(r->fd_slots[i]);
        }
        free(r->fd_slots);
        r->fd_slots = NULL;
    }
    if (r->slots) {
        free(r->slots);
        r->slots = NULL;
    }
    free(r);
}

/* Acquire (or lazily create) the current-thread reactor. Returns the
 * reactor pointer. Called by `Context.executor` to install a fresh
 * reactor for this block_on entry. Nested block_on within the same
 * thread reuses the existing reactor — matches spec B3 "reactor lives
 * per thread; one per block_on call gets the same reactor". */
RuxenReactor *ruxen_reactor_acquire(void) {
    if (!t_current_reactor) {
        t_current_reactor = ruxen_reactor_alloc_struct();
    }
    return t_current_reactor;
}

/* Release the current-thread reactor. Called by `Context.drop` at
 * scope exit. */
void ruxen_reactor_release(void) {
    if (t_current_reactor) {
        ruxen_reactor_free_struct(t_current_reactor);
        t_current_reactor = NULL;
    }
}

/* Return the current-thread reactor as an i64 handle for Ruxen side.
 * Returns 0 if no reactor is installed (e.g. test_dummy contexts or
 * code outside any block_on call). The reactor-aware future uses
 * this in poll() rather than threading a pointer through cx. */
int64_t ruxen_reactor_current_handle(void) {
    return (int64_t)(uintptr_t)t_current_reactor;
}

static int ruxen_reactor_alloc_slot(RuxenReactor *r) {
    for (int i = 0; i < r->slots_len; i++) {
        if (r->slots[i].ident == 0) {
            return i;
        }
    }
    if (r->slots_len == r->slots_cap) {
        int new_cap = r->slots_cap == 0 ? 8 : r->slots_cap * 2;
        void *new_slots = realloc(r->slots, (size_t)new_cap * sizeof(*r->slots));
        if (!new_slots) {
            ruxen_panic("ruxen_reactor: realloc(slots) failed");
            return -1;
        }
        r->slots = new_slots;
        memset(&r->slots[r->slots_cap], 0,
               (size_t)(new_cap - r->slots_cap) * sizeof(*r->slots));
        r->slots_cap = new_cap;
    }
    return r->slots_len++;
}

static int ruxen_reactor_find_slot(RuxenReactor *r, int64_t ident) {
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

/* ── Fd-readiness slot allocator (Milestone 4B) ──────────────────────
 *
 * Allocates a slot for fd-readiness tracking. Returns the slot index
 * (>= 0) or -1 on allocation failure (which never happens in practice —
 * malloc-failure here would have panicked already). Same growth shape
 * as the macOS timer slots array. */
static int ruxen_reactor_alloc_fd_slot(RuxenReactor *r) {
    for (int i = 0; i < r->fd_slots_len; i++) {
        if (!r->fd_slots[i] || r->fd_slots[i]->fd == 0) {
            if (!r->fd_slots[i]) {
                r->fd_slots[i] = (RuxenReactorFdSlot *)calloc(1, sizeof(*r->fd_slots[i]));
                if (!r->fd_slots[i]) {
                    ruxen_panic("ruxen_reactor: calloc(fd_slot) failed");
                    return -1;
                }
            }
            return i;
        }
    }
    if (r->fd_slots_len == r->fd_slots_cap) {
        int new_cap = r->fd_slots_cap == 0 ? 8 : r->fd_slots_cap * 2;
        void *next = realloc(r->fd_slots,
                             (size_t)new_cap * sizeof(*r->fd_slots));
        if (!next) {
            ruxen_panic("ruxen_reactor: realloc(fd_slots) failed");
            return -1;
        }
        r->fd_slots = (RuxenReactorFdSlot **)next;
        memset(&r->fd_slots[r->fd_slots_cap], 0,
               (size_t)(new_cap - r->fd_slots_cap) * sizeof(*r->fd_slots));
        r->fd_slots_cap = new_cap;
    }
    r->fd_slots[r->fd_slots_len] = (RuxenReactorFdSlot *)calloc(1, sizeof(*r->fd_slots[r->fd_slots_len]));
    if (!r->fd_slots[r->fd_slots_len]) {
        ruxen_panic("ruxen_reactor: calloc(fd_slot) failed");
        return -1;
    }
    return r->fd_slots_len++;
}

#if defined(__linux__)
static int ruxen_reactor_owns_fd_slot(RuxenReactor *r, void *p) {
    if (!p || !r->fd_slots) {
        return 0;
    }
    for (int i = 0; i < r->fd_slots_len; i++) {
        if ((void *)r->fd_slots[i] == p) {
            return 1;
        }
    }
    return 0;
}
#endif

/* Map a handle (negative-encoded slot index) back to a slot pointer.
 * Returns NULL if the handle is non-negative (= timer handle, not ours)
 * or out of range. */
static RuxenReactorFdSlot *ruxen_reactor_find_fd_slot(RuxenReactor *r,
                                                     int64_t handle) {
    if (handle >= 0) {
        return NULL;
    }
    int idx = (int)(-(handle + 1));
    if (idx < 0 || idx >= r->fd_slots_len) {
        return NULL;
    }
    if (!r->fd_slots[idx] || r->fd_slots[idx]->fd <= 0) {
        return NULL;
    }
    return r->fd_slots[idx];
}

void ruxen_reactor_set_waker(int64_t reactor_handle, int64_t handle, void *waker) {
    if (reactor_handle == 0 || handle == 0 || !waker) {
        return;
    }
    RuxenReactor *r = (RuxenReactor *)(uintptr_t)reactor_handle;
    void *task = ((RuxenWakerCell *)waker)->task;
    if (!task) {
        return;
    }
    if (handle < 0) {
        RuxenReactorFdSlot *slot = ruxen_reactor_find_fd_slot(r, handle);
        if (slot) {
            slot->task = task;
            if (slot->fired) {
                slot->fired = 0;
                slot->task = NULL;
                ruxen_executor_wake_task(task);
            }
        }
        return;
    }
    int slot = ruxen_reactor_find_slot(r, handle);
    if (slot >= 0) {
        r->slots[slot].task = task;
        if (r->slots[slot].fired) {
            r->slots[slot].task = NULL;
            ruxen_executor_wake_task(task);
        }
    }
}

/* Register an fd for read- or write-readiness. `mode`: 0 = read,
 * 1 = write. Returns a negative-encoded slot handle (always < 0) that
 * `check_fired` / `deregister` accept. The user's fd is NOT closed by
 * deregister — only the OS-level registration is torn down.
 *
 * Linux: EPOLL_CTL_ADD with EPOLLIN or EPOLLOUT, level-triggered
 * (default). `data.ptr` points at the slot so park_current can flip
 * `fired` directly.
 *
 * macOS: EVFILT_READ or EVFILT_WRITE with EV_ADD, no EV_CLEAR
 * (level-triggered analog). `udata` points at the slot. */
static int64_t ruxen_reactor_register_fd_internal(int64_t reactor_handle,
                                                  int64_t fd_in,
                                                  int mode) {
    RuxenReactor *r;
    if (reactor_handle == 0) {
        r = ruxen_reactor_acquire();
    } else {
        r = (RuxenReactor *)(uintptr_t)reactor_handle;
    }
    int user_fd = (int)fd_in;
    if (user_fd < 0) {
        return 0;
    }

    int slot = ruxen_reactor_alloc_fd_slot(r);
    if (slot < 0) {
        return 0;
    }
    RuxenReactorFdSlot *fd_slot = r->fd_slots[slot];
    fd_slot->fd = user_fd;
    fd_slot->mode = mode;
    fd_slot->fired = 0;
    fd_slot->task = NULL;

#if defined(__APPLE__)
    struct kevent ev;
    short filter = mode == 1 ? EVFILT_WRITE : EVFILT_READ;
    EV_SET(&ev, (uintptr_t)user_fd, filter,
           EV_ADD | EV_ENABLE,
           0, 0, fd_slot);
    if (kevent(r->fd, &ev, 1, NULL, 0, NULL) < 0) {
        fd_slot->fd = 0;
        return 0;
    }
#elif defined(__linux__)
    struct epoll_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.events = mode == 1 ? EPOLLOUT : EPOLLIN;
    ev.data.ptr = fd_slot;
    if (epoll_ctl(r->fd, EPOLL_CTL_ADD, user_fd, &ev) < 0) {
        fd_slot->fd = 0;
        return 0;
    }
#endif

    r->registered_count++;
    return (int64_t)(-(slot + 1));
}

int64_t ruxen_reactor_register_fd_read(int64_t reactor_handle, int64_t fd) {
    return ruxen_reactor_register_fd_internal(reactor_handle, fd, 0);
}

int64_t ruxen_reactor_register_fd_write(int64_t reactor_handle, int64_t fd) {
    return ruxen_reactor_register_fd_internal(reactor_handle, fd, 1);
}

/* ── Persistent fd registration ────────────────────────────────────────
 *
 * Variant of register_fd_{read,write} that keeps the OS registration
 * live for the fd's lifetime. The intended caller registers ONCE at fd
 * construction (AsyncTcpStream / AsyncTcpListener `_from_fd` / `_bind`)
 * and keeps the handle alive until the owning stream drops, so the
 * per-poll register + deregister pair the futures used to emit becomes
 * zero syscalls on the hot path (-6 syscalls per HTTP request on the
 * rondo bench).
 *
 * Level-triggering is intentional here. Per-task waker routing means a
 * future attaches its task only after it actually returns Pending; if an
 * edge arrives before that attachment, edge-triggering can consume the
 * event and strand the task. Level-triggering lets the reactor observe
 * still-readable/still-writable fds after the task has registered its
 * waker. */
static int64_t ruxen_reactor_register_fd_persistent_internal(int64_t reactor_handle,
                                                             int64_t fd_in,
                                                             int mode) {
    RuxenReactor *r;
    if (reactor_handle == 0) {
        r = ruxen_reactor_acquire();
    } else {
        r = (RuxenReactor *)(uintptr_t)reactor_handle;
    }
    int user_fd = (int)fd_in;
    if (user_fd < 0) {
        return 0;
    }

    int slot = ruxen_reactor_alloc_fd_slot(r);
    if (slot < 0) {
        return 0;
    }
    RuxenReactorFdSlot *fd_slot = r->fd_slots[slot];
    fd_slot->fd = user_fd;
    fd_slot->mode = mode;
    fd_slot->fired = 0;
    fd_slot->task = NULL;

#if defined(__APPLE__)
    struct kevent ev;
    short filter = mode == 1 ? EVFILT_WRITE : EVFILT_READ;
    EV_SET(&ev, (uintptr_t)user_fd, filter,
           EV_ADD | EV_ENABLE,
           0, 0, fd_slot);
    if (kevent(r->fd, &ev, 1, NULL, 0, NULL) < 0) {
        fd_slot->fd = 0;
        return 0;
    }
#elif defined(__linux__)
    struct epoll_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.events = mode == 1 ? EPOLLOUT : EPOLLIN;
    ev.data.ptr = fd_slot;
    if (epoll_ctl(r->fd, EPOLL_CTL_ADD, user_fd, &ev) < 0) {
        fd_slot->fd = 0;
        return 0;
    }
#endif

    r->registered_count++;
    return (int64_t)(-(slot + 1));
}

int64_t ruxen_reactor_register_fd_read_persistent(int64_t reactor_handle, int64_t fd) {
    return ruxen_reactor_register_fd_persistent_internal(reactor_handle, fd, 0);
}

int64_t ruxen_reactor_register_fd_write_persistent(int64_t reactor_handle, int64_t fd) {
    return ruxen_reactor_register_fd_persistent_internal(reactor_handle, fd, 1);
}

/* Register a one-shot timer. Returns an opaque i64 handle that
 * `check_fired` and `deregister` accept. `reactor` is the i64-cast
 * pointer returned by `ruxen_reactor_current_handle`; if 0 is
 * passed, the function lazy-acquires the current-thread reactor
 * (matches the v1 spec — futures don't need to thread the handle
 * themselves; passing 0 means "use whatever reactor this thread
 * has, allocating if needed"). */
int64_t ruxen_reactor_register_timer(int64_t reactor_handle, int64_t nanos) {
    RuxenReactor *r;
    if (reactor_handle == 0) {
        r = ruxen_reactor_acquire();
    } else {
        r = (RuxenReactor *)(uintptr_t)reactor_handle;
    }
    if (nanos < 0) {
        nanos = 0;
    }

#if defined(__APPLE__)
    int slot = ruxen_reactor_alloc_slot(r);
    if (slot < 0) {
        return 0;
    }
    int64_t ident = r->next_ident++;
    r->slots[slot].ident = ident;
    r->slots[slot].fired = 0;
    r->slots[slot].task = NULL;

    struct kevent ev;
    EV_SET(&ev, (uintptr_t)ident, EVFILT_TIMER,
           EV_ADD | EV_ENABLE | EV_ONESHOT,
           NOTE_NSECONDS, (intptr_t)nanos, NULL);
    if (kevent(r->fd, &ev, 1, NULL, 0, NULL) < 0) {
        r->slots[slot].ident = 0;
        ruxen_panic("ruxen_reactor_register_timer: kevent EV_ADD failed");
        return 0;
    }
    r->registered_count++;
    return ident;
#elif defined(__linux__)
    int tfd = timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC | TFD_NONBLOCK);
    if (tfd < 0) {
        ruxen_panic("ruxen_reactor_register_timer: timerfd_create failed");
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
        ruxen_panic("ruxen_reactor_register_timer: timerfd_settime failed");
        return 0;
    }
    struct epoll_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.events = EPOLLIN;
    ev.data.fd = tfd;
    if (epoll_ctl(r->fd, EPOLL_CTL_ADD, tfd, &ev) < 0) {
        close(tfd);
        ruxen_panic("ruxen_reactor_register_timer: epoll_ctl ADD failed");
        return 0;
    }
    {
        int slot = ruxen_reactor_alloc_slot(r);
        if (slot >= 0) {
            r->slots[slot].ident = (int64_t)tfd;
            r->slots[slot].fired = 0;
            r->slots[slot].task = NULL;
        }
    }
    r->registered_count++;
    return (int64_t)tfd;
#endif
}

int64_t ruxen_reactor_check_fired(int64_t reactor_handle, int64_t handle) {
    if (reactor_handle == 0 || handle == 0) {
        return 0;
    }
    RuxenReactor *r = (RuxenReactor *)(uintptr_t)reactor_handle;

    /* Fd-readiness slots use negative-encoded handles; route on sign so
     * 4B's AsyncFile / 4C's AsyncTcpStream futures share `check_fired`
     * with TimeSleepFuture without an explicit kind argument. */
    if (handle < 0) {
        RuxenReactorFdSlot *slot = ruxen_reactor_find_fd_slot(r, handle);
        if (!slot) {
            return 0;
        }
        if (slot->fired) {
            /* Re-arm: clear the fired bit so the next poll cycle that
             * re-encounters EAGAIN can park again. The OS-level
             * registration remains in place (level-triggered). */
            slot->fired = 0;
            return 1;
        }
        return 0;
    }

#if defined(__APPLE__)
    int slot = ruxen_reactor_find_slot(r, handle);
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

void ruxen_reactor_deregister(int64_t reactor_handle, int64_t handle) {
    if (reactor_handle == 0 || handle == 0) {
        return;
    }
    RuxenReactor *r = (RuxenReactor *)(uintptr_t)reactor_handle;

    /* Fd-readiness slots: negative-encoded handle. Tear down the OS
     * registration but do NOT close the user fd — the future owns the
     * fd lifecycle and closes it in its own drop hook. */
    if (handle < 0) {
        RuxenReactorFdSlot *slot = ruxen_reactor_find_fd_slot(r, handle);
        if (!slot) {
            return;
        }
        int user_fd = slot->fd;
#if defined(__APPLE__)
        struct kevent ev;
        short filter = slot->mode == 1 ? EVFILT_WRITE : EVFILT_READ;
        EV_SET(&ev, (uintptr_t)user_fd, filter, EV_DELETE, 0, 0, NULL);
        (void)kevent(r->fd, &ev, 1, NULL, 0, NULL);
#elif defined(__linux__)
        (void)epoll_ctl(r->fd, EPOLL_CTL_DEL, user_fd, NULL);
#endif
        slot->fd = 0;
        slot->mode = 0;
        slot->fired = 0;
        slot->task = NULL;
        if (r->registered_count > 0) {
            r->registered_count--;
        }
        return;
    }

#if defined(__APPLE__)
    int slot = ruxen_reactor_find_slot(r, handle);
    if (slot < 0) {
        return;
    }
    struct kevent ev;
    EV_SET(&ev, (uintptr_t)handle, EVFILT_TIMER, EV_DELETE, 0, 0, NULL);
    (void)kevent(r->fd, &ev, 1, NULL, 0, NULL);
    r->slots[slot].ident = 0;
    r->slots[slot].fired = 0;
    r->slots[slot].task = NULL;
    if (r->registered_count > 0) {
        r->registered_count--;
    }
#elif defined(__linux__)
    int tfd = (int)handle;
    (void)epoll_ctl(r->fd, EPOLL_CTL_DEL, tfd, NULL);
    close(tfd);
    {
        int slot = ruxen_reactor_find_slot(r, handle);
        if (slot >= 0) {
            r->slots[slot].ident = 0;
            r->slots[slot].fired = 0;
            r->slots[slot].task = NULL;
        }
    }
    if (r->registered_count > 0) {
        r->registered_count--;
    }
#endif
}

/* Wake the reactor identified by `reactor_handle` (the same i64 the
 * waker holds). Safe to call from any thread — eventfd_write /
 * kevent NOTE_TRIGGER are atomic kernel-side operations.
 *
 * This is the bottom-half of the real-waker pipeline: when a Future
 * stashes its `cx.waker` somewhere reachable from another thread (a
 * channel, a signal handler, an OS callback), that consumer calls
 * `Waker.wake()`, which routes here. The parked block_on thread
 * returns from epoll_wait/kevent on the wake fd. Explicit Waker.wake
 * calls mark their owning task ready before reaching this function;
 * fd/timer readiness events below mark all tasks ready as a fallback
 * for older futures whose reactor registrations do not store wakers.
 *
 * Idempotent: writing 1 to an eventfd accumulates the counter (we
 * drain in park_current); EVFILT_USER NOTE_TRIGGER coalesces to a
 * single delivery between waits. Either way "multiple wakes between
 * parks → one unpark" is the correct semantic. */
void ruxen_reactor_wake(int64_t reactor_handle) {
    if (reactor_handle == 0) {
        return;
    }
    RuxenReactor *r = (RuxenReactor *)(uintptr_t)reactor_handle;
#if defined(__APPLE__)
    struct kevent ev;
    EV_SET(&ev, (uintptr_t)&r->wake_fd_sentinel, EVFILT_USER,
           0, NOTE_TRIGGER, 0, &r->wake_fd_sentinel);
    (void)kevent(r->fd, &ev, 1, NULL, 0, NULL);
#elif defined(__linux__)
    if (r->wake_fd < 0) {
        return;
    }
    uint64_t one = 1;
    /* write(2) on an eventfd is async-signal-safe and never short-writes
     * — either 8 bytes or -1. EAGAIN is not possible on the writer side
     * unless the counter is at UINT64_MAX-1, which would require
     * 2^64 unanswered wakes. Ignore the result; on failure the parked
     * thread will eventually time out via the next real I/O event.
     *
     * Assign-then-`(void)` rather than a bare `(void)write(...)`: glibc
     * tags write(2) `warn_unused_result`, and a plain cast does NOT
     * silence that on stricter gcc (`-Wall -Wextra -Werror` in the
     * runtime_safety test), only the assigned-local form does. */
    ssize_t wake_wr = write(r->wake_fd, &one, sizeof(one));
    (void)wake_wr;
#endif
}

/* Park the current thread on the current-thread reactor's wait point.
 * Called by `ruxen_thread_yield` in time.c — the existing
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
void ruxen_reactor_park_current(void) {
    if (!t_current_reactor || t_current_reactor->registered_count <= 0) {
        sched_yield();
        return;
    }
    RuxenReactor *r = t_current_reactor;

#if defined(__APPLE__)
    struct kevent events[16];
    int n = kevent(r->fd, NULL, 0, events, 16, NULL);
    if (n < 0) {
        return;
    }
    for (int i = 0; i < n; i++) {
        /* Wake fd? Identified by udata == &r->wake_fd_sentinel. EV_CLEAR
         * already reset the trigger; nothing further to do. The unpark
         * itself IS the side-effect — the pump in the block_on loop
         * will re-poll every task on return. */
        if (events[i].udata == (void *)&r->wake_fd_sentinel) {
            continue;
        }
        /* Sub-phase 4B: fd-readiness events carry the fd_slot pointer
         * in udata (see register_fd_internal). Timer events have a
         * NULL udata and use ident-lookup. Branch on udata. */
        if (events[i].udata != NULL) {
            RuxenReactorFdSlot *fd_slot = (RuxenReactorFdSlot *)events[i].udata;
            fd_slot->fired = 1;
            if (fd_slot->task) {
                ruxen_executor_wake_task(fd_slot->task);
                fd_slot->task = NULL;
            }
            /* Don't decrement registered_count for fd events — the
             * registration persists across multiple wake/poll cycles
             * until the future calls deregister. */
            continue;
        }
        int64_t ident = (int64_t)events[i].ident;
        int slot = ruxen_reactor_find_slot(r, ident);
        if (slot >= 0) {
            r->slots[slot].fired = 1;
            if (r->slots[slot].task) {
                ruxen_executor_wake_task(r->slots[slot].task);
                r->slots[slot].task = NULL;
            }
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
    /* Sub-phase 4B: fd-readiness events carry the fd_slot pointer in
     * data.ptr (set by register_fd_internal). Timer events use
     * data.fd = timerfd and are observed via the timerfd's read() in
     * check_fired — no slot to mark here. We distinguish by checking
     * whether the pointer lies inside this reactor's fd_slots array. */
    for (int i = 0; i < n; i++) {
        void *p = events[i].data.ptr;
        if (!p) {
            continue;
        }
        /* Wake fd? Identified by data.ptr == &r->wake_fd_sentinel.
         * Drain the eventfd counter so we don't re-fire on the next
         * park if no further wakes arrive. */
        if (p == (void *)&r->wake_fd_sentinel) {
            uint64_t drain;
            /* Assign-then-`(void)`: read(2) is `warn_unused_result` in
             * glibc and a bare cast doesn't silence it under
             * `-Werror`. Draining is best-effort — the counter reset is
             * advisory, so the value is intentionally discarded. */
            ssize_t drain_rd = read(r->wake_fd, &drain, sizeof(drain));
            (void)drain_rd;
            continue;
        }
        if (!r->fd_slots) {
            continue;
        }
        if (ruxen_reactor_owns_fd_slot(r, p)) {
            RuxenReactorFdSlot *fd_slot = (RuxenReactorFdSlot *)p;
            fd_slot->fired = 1;
            if (fd_slot->task) {
                ruxen_executor_wake_task(fd_slot->task);
                fd_slot->task = NULL;
            }
        } else {
            int timer_slot = ruxen_reactor_find_slot(r, (int64_t)events[i].data.fd);
            if (timer_slot >= 0) {
                r->slots[timer_slot].fired = 1;
                if (r->slots[timer_slot].task) {
                    ruxen_executor_wake_task(r->slots[timer_slot].task);
                    r->slots[timer_slot].task = NULL;
                }
            }
        }
    }
#endif
}
