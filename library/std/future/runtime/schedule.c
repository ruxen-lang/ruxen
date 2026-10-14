#include "../../core/runtime/runtime.h"

/* §4.6 Retry / Schedule — C helper for RetryFuture child teardown.
 *
 * v1 Schedule/retry is mostly pure `.rx` (the immutable `Schedule` builder + the
 * sync `run` driver + the hand-written `RetryFuture` poll machine in
 * `library/std/future/src/schedule.rx`), so this file is intentionally tiny. It
 * exists only so `RetryFuture.drop` can run its live child future's own
 * `def drop` — the in-flight backoff `TimeSleepFuture` (deregister its reactor
 * timer) or the current attempt `any Future` — via the compiler-synthesised
 * `Drop_dynamic_drop`, then free the child's heap block. This is the §4.6
 * drop-correctness story: a cancelled retry deterministically tears down its
 * parked backoff timer / attempt. Mirrors `ruxen_stream_drop_inner`.
 */

#include <stdint.h>
#include <stdlib.h>

/* The compiler-synthesised single-helper that routes a future's `def drop`
 * through the Drop vtable slot in its class_info (§4.1 narrow path; emitted for
 * every Future class). Declared `__weak` so a program with no future at all
 * still links — then the call is a guarded no-op. */
__attribute__((weak)) void Drop_dynamic_drop(int64_t self);

/* The compiler-synthesised dynamic poll dispatch helper (mixin_vtables §B5),
 * emitted for any program with a `Future`-including class. Takes the future's
 * heap ptr + the Context heap ptr (both i64), returns the `Poll[T]` heap ptr.
 * SAME symbol the scheduler / Race / Queue use — this is the established
 * dyn-poll mechanism, NOT new dispatch machinery. */
int64_t Future_dynamic_poll(int64_t self, int64_t ctx);

/* §4.6 RetryFuture inner-poll step (mirrors `ruxen_race_step`). The `RetryFuture`
 * holds its current attempt as `any Future` whose `poll -> Poll[Self.Output]`
 * has an INDETERMINATE associated Output for a dyn type, so it cannot be polled
 * via the `.rx`-level `.poll` (typeck: "any Future does not satisfy Future").
 * This wraps the C-only `Future_dynamic_poll` exactly as Race does: poll the
 * inner, decode the outer `Poll` AND the inner `Result[Int, Int]` it carries.
 *
 * The attempt future's `Output` is `Result[Int, Int]` (v1 Int channel). Both
 * `Poll[T]` and `Result[Int,Int]` are the canonical 16-byte tagged-enum block
 * `{ i32 tag; i32 pad; i64 payload }` (Poll Ready=0/Pending=1; Result Ok=0/Err=1)
 * — so we can read the inner Result's tag + Int directly, returning a single
 * status the `.rx` machine branches on WITHOUT rehydrating a `Result` value
 * (which the raw-i64 channel can't do). Returns:
 *   0 = inner Pending (park)
 *   1 = inner Ready Ok   — `ruxen_retry_inner_value` holds the Ok Int
 *   2 = inner Ready Err  — `ruxen_retry_inner_value` holds the Err Int
 * `cx` is the raw `RuxenContext*` at the i64 ABI, forwarded to
 * `Future_dynamic_poll`. NO new dispatch/vtable machinery — one thin wrapper
 * over the existing helper. The 16-byte Poll box returned by `Future_dynamic_poll`
 * is freed here once decoded (this C caller is its only owner, as in the
 * scheduler's `ruxen_poll_is_ready`); the Ready payload, the `Result[Int,Int]`,
 * is not freed. */
#define RUXEN_POLL_READY_TAG    0
#define RUXEN_RESULT_OK_TAG     0
static _Thread_local int64_t t_retry_inner_value = 0;

int64_t ruxen_retry_poll_inner(int64_t inner_ptr, int64_t cx) {
    if (inner_ptr == 0) {
        return 0; /* no inner → treat as Pending (defensive) */
    }
    int64_t poll_val = Future_dynamic_poll(inner_ptr, cx);
    if (poll_val == 0) {
        return 0;
    }
    char *poll = (char *)(uintptr_t)poll_val;
    int32_t poll_tag = *(int32_t *)(poll + 0);
    if (poll_tag != RUXEN_POLL_READY_TAG) {
        ruxen_dealloc(poll);
        return 0; /* Pending */
    }
    /* Ready: payload@8 is the `Result[Int,Int]` heap ptr. Decode its tag + Int. */
    int64_t result_ptr = *(int64_t *)(poll + 8);
    ruxen_dealloc(poll);
    if (result_ptr == 0) {
        /* Defensive: a Ready with a null payload — treat as a zero Err. */
        t_retry_inner_value = 0;
        return 2;
    }
    char *res = (char *)(uintptr_t)result_ptr;
    int32_t res_tag = *(int32_t *)(res + 0);
    t_retry_inner_value = *(int64_t *)(res + 8);
    if (res_tag == RUXEN_RESULT_OK_TAG) {
        return 1; /* Ready Ok */
    }
    return 2; /* Ready Err */
}

/* Read the Int value (Ok value or Err value) from the most recent
 * `ruxen_retry_poll_inner` that returned Ready (1 or 2). Single in-flight inner
 * per RetryFuture (the poll machine drives one attempt at a time), so the
 * thread-local scratch is safe — mirrors `ruxen_race_winner_payload`. */
int64_t ruxen_retry_inner_value(void) {
    return t_retry_inner_value;
}

/* Run a RetryFuture child future's `def drop` (via Drop_dynamic_drop) + free its
 * heap block. NULL child (never set) is a no-op. Used for BOTH the backoff
 * `TimeSleepFuture` (its drop deregisters the reactor timer) and the current
 * attempt `any Future` (its drop runs its own teardown). Each is dropped at
 * most once per RetryFuture instance (the future is dropped once — by scope-exit
 * or by an outer Scope/Race cancel via Drop_dynamic_drop), so no re-entrancy
 * guard is needed here. */
void ruxen_retry_drop_child(int64_t child_ptr) {
    if (child_ptr == 0) {
        return;
    }
    if (Drop_dynamic_drop) {
        Drop_dynamic_drop(child_ptr);
    }
    ruxen_dealloc((void *)(uintptr_t)child_ptr);
}

/* Alias for the attempt-inner child — same teardown. Two names keep the `.rx`
 * `def drop` self-documenting (backoff vs inner) without two code paths. */
void ruxen_retry_drop_inner(int64_t child_ptr) {
    ruxen_retry_drop_child(child_ptr);
}
