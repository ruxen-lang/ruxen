#include "../../core/runtime/runtime.h"

/* §4.7 deterministic virtual TestClock.
 *
 * A `--test`-style mode where the per-thread reactor swaps its real OS timer
 * for an in-memory virtual clock + ordered timer queue, so time-based async
 * code (timeouts, retries, `Async.sleep`) resolves INSTANTLY and
 * deterministically in a test — no real sleeps.
 *
 * SINGLE SOURCE OF TRUTH: a `_Thread_local` virtual-clock cell lives HERE.
 * Both the clock readers (`time.c`'s `ruxen_time_now_ns` / `ruxen_instant_now`
 * / `ruxen_instant_elapsed`) and the timer machinery (`reactor.c`'s
 * `register_timer` / `park_current` / `check_fired`) consult it. The reactor is
 * per-thread and PERSISTS across `block_on` calls (`executor.c` deliberately
 * does not release it per Context.drop), so there is one stable per-thread
 * object to host the virtual clock; tests install + restore explicitly via the
 * `TestClock` install guard's `def drop`.
 *
 * SCOPE — which time seams are virtualized (spec §4.7 Correction 1, VERIFIED
 * against source): `ruxen_time_now_ns` backs ONLY the ASYNC sleep deadline
 * (read in `time_sleep_future.rx`). Sync `Instant.now` / `Instant.elapsed`
 * sample `clock_gettime(CLOCK_MONOTONIC)` INDEPENDENTLY and do NOT route through
 * `ruxen_time_now_ns`. To make BOTH async-sleep time AND sync-Instant time
 * virtual, all three readers route their real sample through
 * `ruxen_test_clock_now_ns(real)` here — in non-virtual mode it returns `real`
 * unchanged (a single thread-local read + branch; zero behaviour change off
 * test). `ruxen_time_unix_ns` (REALTIME / wall clock) is NOT virtualized — it
 * is not a monotonic-deadline source.
 *
 * HOT-PATH COST (spec §4.7 risk 1): `ruxen_test_clock_now_ns` is a single
 * thread-local flag read + branch; when virtual mode is off it returns its
 * argument unchanged, so the release/non-test path pays one predictable-branch
 * thread-local load — no syscall, no allocation.
 *
 * HYBRID (spec §4.7): ONLY the timer + clock seam is virtualized. Real
 * epoll/kqueue fd readiness is untouched — a test mixing real sockets + virtual
 * time still gets real I/O wakeups (handled in `reactor.c`'s park_current,
 * which services real fd events even in virtual mode) and virtual timer
 * wakeups. Matches tokio's `pause()` affecting only time.
 *
 * SINGLE-THREAD v1 (matches the rest of the executor): the cell is thread-local;
 * a test installs the clock on the thread it drives `block_on` on.
 */

#include <stdint.h>
#include <stdlib.h>

/* Forward decl into reactor.c: fire every pending VIRTUAL timer whose recorded
 * deadline is <= `now_ns`, marking it fired + waking its parked task. Called by
 * `advance` / `advance_to_next` / `set` after bumping virtual-now. */
void ruxen_reactor_fire_due_timers(int64_t now_ns);

/* Forward decl into reactor.c: the earliest not-yet-fired virtual-timer
 * absolute deadline (ns), or -1 if none pending. Backs `advance_to_next`. */
int64_t ruxen_reactor_next_virtual_deadline(void);

/* The per-thread virtual-clock cell. `mode == 0` → real time (the cell is
 * inert); `mode == 1` → virtual time, `now_ns` is the current virtual instant.
 *
 * VIRTUAL EPOCH = 0: `install` anchors virtual-now at 0 (NOT the real monotonic
 * now). This makes the ABSOLUTE `TestClock.set(d)` mean "d ns since install"
 * (intuitive) and `advance`/`advance_to_next` deltas exact. A timer registered
 * after install gets deadline = virtual_now + nanos = (elapsed virtual) + nanos,
 * so `Async.sleep(30s)` then `set(40s)` fires it (40s >= 30s). Anchoring to the
 * real boot-time monotonic value instead would make `set(40s)` a no-op (40s ≪
 * the ~10^15-ns boot offset) — the bug pin 440 caught. */
static _Thread_local int8_t  t_vclock_mode = 0;
static _Thread_local int64_t t_vclock_now_ns = 0;
/* Nesting depth: a nested/repeated `TestClock.install` must NOT re-anchor the
 * active virtual clock, and an INNER guard's `def drop` must NOT uninstall the
 * OUTER guard's clock. Only the OUTERMOST install anchors (now=0, mode=1) and
 * only the OUTERMOST uninstall restores the real clock (Codex medium). */
static _Thread_local int64_t t_vclock_depth = 0;

/* The clock seam every reader routes through. Returns the virtual now in
 * virtual mode, else the caller's real sample unchanged. THE hot path — keep it
 * a single branch. */
int64_t ruxen_test_clock_now_ns(int64_t real_ns) {
    if (t_vclock_mode) {
        return t_vclock_now_ns;
    }
    return real_ns;
}

/* Is virtual mode active on this thread? (reactor.c branches its
 * register_timer / park_current on this.) */
int8_t ruxen_test_clock_is_virtual(void) {
    return t_vclock_mode;
}

/* The current virtual instant (reactor.c reads this to compute a timer's
 * absolute virtual deadline at registration). Only meaningful in virtual mode. */
int64_t ruxen_test_clock_virtual_now(void) {
    return t_vclock_now_ns;
}

/* ---------------------------------------------------------------------
 * Install / uninstall — the guard surface. `install` anchors virtual-now at the
 * current real monotonic instant (so time starts "now") and flips mode on;
 * `uninstall` flips it off, restoring the real clock. Idempotent-ish: a second
 * install re-anchors; uninstall on an already-real clock is a no-op. The
 * `TestClock` guard's `def drop` calls `uninstall`. */
void ruxen_test_clock_install(void) {
    t_vclock_depth++;
    if (t_vclock_depth == 1) {
        /* Outermost install: anchor at the virtual epoch + go virtual. A nested
         * install (depth > 1) leaves the active virtual-now untouched. */
        t_vclock_now_ns = 0;
        t_vclock_mode = 1;
    }
}

void ruxen_test_clock_uninstall(void) {
    if (t_vclock_depth > 0) {
        t_vclock_depth--;
    }
    if (t_vclock_depth == 0) {
        /* Outermost uninstall: restore the real clock. An inner guard's drop
         * (depth still > 0) leaves the outer clock running. */
        t_vclock_mode = 0;
    }
}

/* Advance virtual time by `delta_ns` (clamped to >= 0 — time never goes
 * backwards), then fire every now-due virtual timer. No-op if not in virtual
 * mode (advancing a real clock is meaningless). */
void ruxen_test_clock_advance(int64_t delta_ns) {
    if (!t_vclock_mode) {
        return;
    }
    if (delta_ns < 0) {
        delta_ns = 0;
    }
    t_vclock_now_ns += delta_ns;
    ruxen_reactor_fire_due_timers(t_vclock_now_ns);
}

/* Jump virtual-now to the EARLIEST pending virtual-timer deadline (firing it +
 * any others now due), returning the ns advanced. Returns -1 if no timer is
 * pending (nothing to advance to). No-op (returns -1) off virtual mode. Lets a
 * test step deterministically deadline-by-deadline (the "auto-advance to next
 * deadline" surface). */
int64_t ruxen_test_clock_advance_to_next(void) {
    if (!t_vclock_mode) {
        return -1;
    }
    int64_t next = ruxen_reactor_next_virtual_deadline();
    if (next < 0) {
        return -1;
    }
    int64_t delta = next - t_vclock_now_ns;
    if (delta < 0) {
        /* A deadline already in the past (shouldn't happen — fired timers are
         * skipped — but clamp defensively). */
        delta = 0;
    }
    t_vclock_now_ns = next;
    ruxen_reactor_fire_due_timers(t_vclock_now_ns);
    return delta;
}

/* Set virtual-now to an ABSOLUTE instant (`absolute_ns` since the virtual
 * epoch), firing every timer at/before it. Time never moves backwards: a value
 * below the current virtual-now is clamped to it (no-op fire). No-op off
 * virtual mode. */
void ruxen_test_clock_set(int64_t absolute_ns) {
    if (!t_vclock_mode) {
        return;
    }
    if (absolute_ns < t_vclock_now_ns) {
        absolute_ns = t_vclock_now_ns;
    }
    t_vclock_now_ns = absolute_ns;
    ruxen_reactor_fire_due_timers(t_vclock_now_ns);
}
