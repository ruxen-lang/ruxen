#include "../../core/runtime/runtime.h"
#include "future_rt.h"

/* Sub-phase 5 of the async round (docs/specs/stdlib/task_spawn.spec.md):
 * single-threaded task scheduler that lives alongside the inline
 * block_on poll-loop.
 *
 * Architecture:
 *
 *   - One task queue per thread, held in a `_Thread_local` linked
 *     list. Each entry holds (future_ptr, classinfo_ptr — implied
 *     by future_ptr's offset-0 header, done_flag, result_slot,
 *     handle_ptr). The handle is a separately heap-allocated
 *     `RuxenTaskHandle` so the caller can hold a pointer to it
 *     across the task's lifetime; the queue entry holds the same
 *     pointer so `pump` can set the done bit + write the result.
 *
 *   - `ruxen_executor_spawn(future_ptr)` enqueues a new task and
 *     returns the handle pointer. Does NOT poll the future.
 *
 *   - `ruxen_executor_pump_tasks()` walks the queue ONCE. For each
 *     live task: calls `Future_dynamic_poll(self, ctx)` (the
 *     compiler-synthesised dispatch helper for the `Future` mixin).
 *     If Ready, writes the payload into the handle's result slot
 *     and marks done; the queue entry is freed but the handle
 *     persists for the caller's `task_join`. If Pending, leaves
 *     the task in the queue.
 *
 *   - `ruxen_executor_queue_nonempty()` — fast bool check used by
 *     the AST-level block_on rewriter to skip the pump call when
 *     no tasks were ever spawned (zero overhead on the existing
 *     2A/2B/3 / 4 / 770 fixtures that never spawn).
 *
 *   - On `Context.drop` (end of block_on), `ruxen_executor_drain_remaining`
 *     fires `def drop` on every still-queued future per spec B10.
 *     v1 drop discipline: leak the future heap blocks if no `def drop`
 *     was registered, mirror the same loose contract `Thread.spawn`'s
 *     join-handle drop has. Once Drop becomes a runtime-dispatched
 *     mixin (mixin_vtables.spec.md §B9 + B11), we revisit this.
 *
 * Why a separate file from executor.c: scheduler.c carries the new
 * task-queue state; executor.c keeps the Context lifecycle. Keeping
 * them split means the AST-level pump hook can be a single
 * forward-decl into here without touching the existing Context ABI.
 *
 * Wake routing: selective when a Future calls `cx.waker().wake()`.
 * Each queued task owns a stable Context whose Waker points back to
 * that queue entry; waking marks just that entry ready and pokes the
 * reactor wake fd. Existing async-net/timer futures still use reactor
 * readiness directly rather than storing wakers in fd/timer slots, so
 * OS readiness falls back to marking all queued tasks ready.
 *
 * Recursion safety: `pump_tasks` walks by index, NOT by iterator.
 * If a polled task itself calls `Task.spawn`, the new entry is
 * pushed onto the end of the queue and `pump_tasks` will see it on
 * the same pass (or the next iteration of the outer block_on loop —
 * either is correct). Removing a completed task during the walk
 * uses the linked-list unlink pattern to keep the walk valid.
 *
 * ABI summary (all entries take/return i64 at the Ruxen call site):
 *   ruxen_executor_spawn(future_ptr: i64) -> i64   (handle pointer)
 *   ruxen_executor_pump_tasks() -> i64             (count of completions this pass)
 *   ruxen_executor_queue_nonempty() -> i64         (1 if any live task)
 *   ruxen_executor_drain_remaining() -> ()
 *   ruxen_task_handle_is_done(handle: i64) -> i64
 *   ruxen_task_handle_result(handle: i64) -> i64
 *   ruxen_task_handle_drop(handle: i64) -> ()
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* Forward decl into Context factory (executor.c). The pump call
 * needs a Context pointer to pass into each Future_dynamic_poll;
 * the AST-level block_on rewriter already constructs one per
 * block_on call, and we reuse it via thread-local storage. */
void *ruxen_executor_make_context(void);
void *ruxen_executor_make_task_context(void *task_entry);
void ruxen_executor_context_drop(void *cx);

/* ---------------------------------------------------------------------
 * Task handle (Ruxen-visible). Caller-held; outlives the queue entry.
 *
 * Layout MUST match the C-side ABI assumed by the Ruxen-level
 * `class TaskHandle[T]` lib decls in library/std/future/src/lib.rx.
 * Keep this struct private to scheduler.c — Ruxen sees only the
 * opaque pointer. */
typedef struct RuxenTaskHandle {
    int64_t done;        /* 0 = pending, 1 = ready */
    int64_t result;      /* valid only when done == 1 */
    int64_t refcount;    /* 2 on spawn (queue + caller), drops to 0 frees */
} RuxenTaskHandle;

/* ---------------------------------------------------------------------
 * Queue entry. Lives on the heap. Linked-list intrusive next pointer.
 *
 * `future_ptr` is the heap pointer to the future's instance — the
 * class_info_ptr lives at *(void**)future_ptr per mixin_vtables
 * §B2. Future_dynamic_poll reads it. */
typedef struct RuxenTaskEntry {
    int64_t future_ptr;
    RuxenTaskHandle *handle;
    void *ctx;
    int ready;
    struct RuxenTaskEntry *next;
} RuxenTaskEntry;

/* Per-thread queue head. Each task entry owns its own Context so
 * `cx.waker()` can be stashed by the future and still point back to
 * the same task after the scheduler polls other entries. */
static _Thread_local RuxenTaskEntry *t_queue_head = NULL;
static _Thread_local int t_pump_in_progress = 0;

/* ---------------------------------------------------------------------
 * Spawn.
 *
 * Allocates the queue entry + handle, pushes onto the tail of the
 * queue. Returns the handle pointer to Ruxen as an i64. The future
 * pointer's lifetime is now owned by the queue — the caller's local
 * binding for the future must fall out of scope (per spec §B1 —
 * Task.spawn moves the future into the queue). */
int64_t ruxen_executor_spawn(int64_t future_ptr) {
    if (future_ptr == 0) {
        ruxen_panic("ruxen_executor_spawn: null future");
        return 0;
    }
    RuxenTaskHandle *h = (RuxenTaskHandle *)ruxen_xmalloc(sizeof(RuxenTaskHandle));
    h->done = 0;
    h->result = 0;
    h->refcount = 2; /* queue + caller */

    RuxenTaskEntry *e = (RuxenTaskEntry *)malloc(sizeof(RuxenTaskEntry));
    if (!e) {
        free(h);
        ruxen_panic("ruxen_executor_spawn: malloc(entry) failed");
        return 0;
    }
    e->future_ptr = future_ptr;
    e->handle = h;
    e->ready = 1; /* First poll after spawn must happen without a wake. */
    e->next = NULL;
    e->ctx = ruxen_executor_make_task_context((void *)e);
    if (!e->ctx) {
        free(e);
        free(h);
        ruxen_panic("ruxen_executor_spawn: make task context failed");
        return 0;
    }

    /* Append to tail. Round-robin = FIFO walk, so newer tasks land
     * at the end and won't be polled before the existing ones get
     * their fair shake. */
    if (!t_queue_head) {
        t_queue_head = e;
    } else {
        RuxenTaskEntry *cur = t_queue_head;
        while (cur->next) {
            cur = cur->next;
        }
        cur->next = e;
    }
    return (int64_t)(uintptr_t)h;
}

/* ---------------------------------------------------------------------
 * Queue-nonempty fast check. Used by the AST-level block_on rewriter
 * to skip the pump call when no tasks ever spawned. Returns i64 1/0
 * to match the Ruxen Bool ABI. */
int64_t ruxen_executor_queue_nonempty(void) {
    return t_queue_head ? 1 : 0;
}

int64_t ruxen_executor_ready_nonempty(void) {
    RuxenTaskEntry *cur = t_queue_head;
    while (cur) {
        if (cur->ready) {
            return 1;
        }
        cur = cur->next;
    }
    return 0;
}

void ruxen_executor_wake_task(void *task_entry) {
    if (!task_entry) {
        return;
    }
    ((RuxenTaskEntry *)task_entry)->ready = 1;
}

/* ---------------------------------------------------------------------
 * Decode and consume a Poll[T] returned by Future_dynamic_poll. The C
 * caller is the Poll's only owner (no Ruxen match drop-elaborates it),
 * so the 16-byte box is freed here after the tag and payload are read.
 * Only the box: the Ready payload (possibly a heap pointer) is moved
 * out to `*out_payload` and owned by the caller from then on.
 */
static int ruxen_poll_is_ready(int64_t poll_val, int64_t *out_payload) {
    if (poll_val == 0) {
        /* Defensive: shouldn't happen — Future_dynamic_poll always
         * returns a valid Poll. Treat as Pending. */
        return 0;
    }
    /* Tag is i32 at offset 0; payload i64 at offset 8 (cranelift emit
     * in compiler/ruxen_core/src/codegen/cranelift/emit.rs::SetTag /
     * GetTag / GetPayload). Reading the tag as i32 avoids picking up
     * 4 bytes of uninitialised slop after the tag word. */
    char *p = (char *)(uintptr_t)poll_val;
    int32_t tag = *(int32_t *)(p + 0);
    int ready = tag == RUXEN_POLL_READY_TAG;
    if (ready && out_payload) {
        *out_payload = *(int64_t *)(p + 8);
    }
    ruxen_dealloc(p);
    return ready;
}

/* ---------------------------------------------------------------------
 * Pump.
 *
 * Walk the queue once. For each task:
 *   - Call Future_dynamic_poll(future_ptr, ctx).
 *   - If Ready: mark handle done, write result, unlink + free entry.
 *   - If Pending: leave in place.
 *
 * Returns the number of completions this pass (informational —
 * caller doesn't need it, but the i64 return helps debug/logging).
 *
 * Walk-by-index variant: we use a prev/cur pointer pair so unlinking
 * during the walk is safe. Newly spawned tasks appended during a
 * poll callback land after our cur->next path; we'll see them on
 * this same pass IF they get appended after the cur pointer, OR on
 * the next outer-loop iteration if they got appended before our
 * cur position (impossible given append-to-tail, but the index walk
 * is invariant-correct regardless).
 *
 * Re-entrance: t_pump_in_progress guards against a pump-from-within-
 * pump call (a polled future that somehow triggers pump again). v1
 * shouldn't see this — no surface lets a future synchronously
 * pump — but the flag is cheap insurance.
 */
int64_t ruxen_executor_pump_tasks(void) {
    if (!t_queue_head) {
        return 0;
    }
    if (t_pump_in_progress) {
        /* Defensive: pump-from-within-pump. Bail. */
        return 0;
    }
    t_pump_in_progress = 1;

    int64_t completions = 0;
    RuxenTaskEntry **prev_link = &t_queue_head;
    RuxenTaskEntry *cur = t_queue_head;
    while (cur) {
        if (!cur->ready) {
            prev_link = &cur->next;
            cur = cur->next;
            continue;
        }
        cur->ready = 0;
        int64_t poll_val = Future_dynamic_poll(cur->future_ptr, (int64_t)(uintptr_t)cur->ctx);
        int64_t payload = 0;
        if (ruxen_poll_is_ready(poll_val, &payload)) {
            cur->handle->result = payload;
            cur->handle->done = 1;
            /* Queue-side refcount drop. */
            cur->handle->refcount--;
            if (cur->handle->refcount == 0) {
                free(cur->handle);
            }
            RuxenTaskEntry *next = cur->next;
            *prev_link = next;
            if (cur->ctx) {
                ruxen_executor_context_drop(cur->ctx);
                free(cur->ctx);
            }
            free(cur);
            cur = next;
            completions++;
            continue;
        }
        prev_link = &cur->next;
        cur = cur->next;
    }

    t_pump_in_progress = 0;
    return completions;
}

/* ---------------------------------------------------------------------
 * Drain remaining tasks at block_on exit. Spec §B10: every still-
 * queued task's future has its `def drop` fire — clean up reactor
 * registrations, fds, etc.
 *
 * §4.1 NARROW cancellation path (docs/dev/concurrency-impl-plan.md
 * Increment 1): we now call `Drop_dynamic_drop(future_ptr)` on every
 * still-queued/parked future BEFORE freeing the queue entry. The
 * compiler routes this through the Drop vtable slot it added to the
 * future's existing class_info (no global `mixin Drop dispatch runtime`),
 * so the concrete `<Class>_drop(self)` runs — deregistering the reactor
 * timer, closing the fd, releasing the refcount the hand-written future
 * `def drop`s own. `Drop_dynamic_drop` is `__weak`: a program with no
 * drop-defining future never emits the helper, so the symbol is NULL and
 * we skip the call (the future then carries no `def drop` to run anyway).
 *
 * Drop ordering: we run the future's `def drop` first (it may touch the
 * reactor via `ruxen_reactor_current_handle()`), THEN release the
 * queue-side handle refcount and the Context. The future heap block
 * itself is still leaked here — freeing it needs an allocator-side
 * companion (a `ruxen_dealloc(future_ptr)` after drop) tracked as the
 * residual; the user-visible RESOURCE leak (fds/timers/refcounts) is
 * what `def drop` releases and is now closed.
 */
void ruxen_executor_drain_remaining(void) {
    RuxenTaskEntry *cur = t_queue_head;
    while (cur) {
        RuxenTaskEntry *next = cur->next;
        /* Run the future's `def drop` (or its no-op) before tearing
         * down the queue bookkeeping. The weak symbol is NULL (call
         * skipped) only when the program has NO future at all, so the
         * helper was never emitted. */
        if (Drop_dynamic_drop && cur->future_ptr) {
            Drop_dynamic_drop(cur->future_ptr);
        }
        /* Decrement handle refcount; free if no caller holds it
         * (rare — usually the caller has dropped its TaskHandle by
         * the time block_on exits; the join-fence in B6 is the
         * recommended pattern). */
        cur->handle->refcount--;
        if (cur->handle->refcount == 0) {
            free(cur->handle);
        }
        if (cur->ctx) {
            ruxen_executor_context_drop(cur->ctx);
            free(cur->ctx);
        }
        free(cur);
        cur = next;
    }
    t_queue_head = NULL;
}

/* ---------------------------------------------------------------------
 * §4.2 Structured Scope — cancel ONE still-queued task by its handle.
 *
 * Unlike `drain_remaining` (which hits the WHOLE thread-local queue), this
 * cancels exactly the task whose handle matches `handle_ptr` — so a
 * `Scope#drop` cancels only ITS OWN children, leaving nested scopes / block_on
 * roots / other scopes' tasks untouched. Used by `Scope#drop` per still-pending
 * child.
 *
 * Steps (ordered for REENTRANCY SAFETY — the user `def drop` may re-enter
 * the scheduler, so the entry must be off-queue before it runs):
 *   1. find the queue entry with `entry->handle == handle_ptr`;
 *   2. UNLINK it (`*prev_link = cur->next`) + mark the handle `done` so a
 *      dangling `Task.join` resolves (default 0 result — cancelled, not run)
 *      and a nested pump/cancel can no longer reach this entry;
 *   3. THEN fire the child future's `def drop` via the §4.1 `Drop_dynamic_drop`
 *      (reactor deregister / fd close), with cur/future_ptr/ctx held in locals;
 *   4. drop the QUEUE-side handle refcount + free the Context + free the entry.
 *
 * No-op if the handle isn't in the queue (already completed via the pump, or
 * already cancelled) — idempotent, so `Scope#drop` can call it unconditionally
 * on any not-yet-done child. */
void ruxen_executor_cancel_task(int64_t handle_ptr) {
    if (handle_ptr == 0) {
        return;
    }
    RuxenTaskHandle *target = (RuxenTaskHandle *)(uintptr_t)handle_ptr;
    RuxenTaskEntry **prev_link = &t_queue_head;
    RuxenTaskEntry *cur = t_queue_head;
    while (cur) {
        if (cur->handle == target) {
            /* REENTRANCY SAFETY: unlink the entry from the queue and mark the
             * handle done BEFORE running the user `def drop`. A future's
             * `def drop` is arbitrary user code that may re-enter the scheduler
             * (e.g. call `ruxen_executor_pump_tasks`); if `cur` were still
             * linked with `done == 0` during the drop, a nested pump could poll
             * it Ready and `free(cur)`, then this frame would double-free `cur`
             * on resume. By unlinking + marking done first, `cur` is
             * unreachable from the queue while the drop runs — the nested pump
             * can't see it. We hold `cur`/`cur->future_ptr`/`cur->ctx` alive in
             * locals across the drop, then tear them down after it returns. */
            *prev_link = cur->next;
            cur->handle->done = 1; /* dangling join → resolves (0 result) */

            /* Now run the child's user `def drop` (off-queue). */
            if (Drop_dynamic_drop && cur->future_ptr) {
                Drop_dynamic_drop(cur->future_ptr);
            }

            /* Queue-side refcount drop + Context teardown + free the entry. */
            cur->handle->refcount--;
            if (cur->handle->refcount == 0) {
                free(cur->handle);
            }
            if (cur->ctx) {
                ruxen_executor_context_drop(cur->ctx);
                free(cur->ctx);
            }
            free(cur);
            return;
        }
        prev_link = &cur->next;
        cur = cur->next;
    }
    /* Not found: already completed or cancelled — no-op. */
}

/* ---------------------------------------------------------------------
 * §4.3 General `Race` — C step machine for the 2-arm `Race.of(a, b)`.
 *
 * Mirrors the AsyncReadWithTimeoutFuture 2-way race, generalised to arbitrary
 * `Future` arms (polled polymorphically via `Future_dynamic_poll`). The Ruxen
 * `RaceFuture` owns a `RuxenRaceState` (the `state_ptr` pattern) and drives it
 * from `poll`; the C side polls each LIVE arm, and on the FIRST arm that returns
 * Ready it CANCELS the other arm (runs its user `def drop` via the §4.1
 * `Drop_dynamic_drop`, then frees its heap block) BEFORE reporting the winner.
 * Loser cleanup is synchronous — no async-drop machinery.
 *
 * Both arms share the per-thread reactor, so `park_current` (driven by the
 * block_on/collect pump's Pending arm) wakes as soon as EITHER arm's
 * registration fires; the next step resolves the race. No new reactor
 * primitive. Arms are the i64 heap pointers of the two future instances. */
typedef struct RuxenRaceState {
    int64_t arm_a;          /* future heap ptr; 0 once consumed/cancelled */
    int64_t arm_b;          /* future heap ptr; 0 once consumed/cancelled */
    int64_t winner_payload; /* the winning Poll.Ready payload (i64 ABI) */
    int winner;             /* 0 = undecided, 1 = arm_a won, 2 = arm_b won */
} RuxenRaceState;

int64_t ruxen_race_state_new(int64_t arm_a, int64_t arm_b) {
    RuxenRaceState *s = (RuxenRaceState *)ruxen_xmalloc(sizeof(RuxenRaceState));
    s->arm_a = arm_a;
    s->arm_b = arm_b;
    s->winner_payload = 0;
    s->winner = 0;
    return (int64_t)(uintptr_t)s;
}

/* Poll the live arms once. Returns 0 = both still Pending (undecided),
 * 1 = arm_a won, 2 = arm_b won. On a win: record the winner payload, then
 * CANCEL the LOSER (its `def drop` runs + free). The WINNER block is NOT
 * touched — it is LEAKED, deliberately:
 *   - the winner returned `Poll.Ready(payload)`, MOVING ownership of its
 *     Output out (the i64 `payload` may itself be a heap pointer — String /
 *     class — that the caller now owns). Running the winner's `def drop` could
 *     free that moved-out payload → use-after-free of the RESULT (Codex
 *     blocker). Freeing the winner block without its drop could leak/UAF its
 *     other fields. So we leak the winner block — matching the established v1
 *     scheduler discipline (the pump-completion path also leaks completed
 *     future blocks; only the payload is consumed). The user-visible RESOURCE
 *     guarantee Race makes is loser-cleanup, which is what fires `def drop`.
 * Left-to-right poll order (arm_a first) — documented, NOT fair. */
int64_t ruxen_race_step(int64_t state_ptr, int64_t cx) {
    if (state_ptr == 0) {
        return 0;
    }
    RuxenRaceState *s = (RuxenRaceState *)(uintptr_t)state_ptr;
    if (s->winner != 0) {
        /* Already decided — idempotent. */
        return s->winner;
    }

    /* Arm A. */
    if (s->arm_a != 0) {
        int64_t pa = Future_dynamic_poll(s->arm_a, cx);
        int64_t payload = 0;
        if (ruxen_poll_is_ready(pa, &payload)) {
            /* A won. Clear both slots BEFORE cancelling (reentrancy). The
             * winner block is leaked (see header); only the loser is dropped. */
            int64_t loser = s->arm_b;
            s->arm_a = 0;
            s->arm_b = 0;
            s->winner_payload = payload;
            s->winner = 1;
            ruxen_future_drop_and_free(loser); /* loser: def drop + free */
            return 1;
        }
    }

    /* Arm B. */
    if (s->arm_b != 0) {
        int64_t pb = Future_dynamic_poll(s->arm_b, cx);
        int64_t payload = 0;
        if (ruxen_poll_is_ready(pb, &payload)) {
            int64_t loser = s->arm_a;
            s->arm_a = 0;
            s->arm_b = 0;
            s->winner_payload = payload;
            s->winner = 2;
            ruxen_future_drop_and_free(loser);
            return 2;
        }
    }

    return 0; /* both Pending */
}

int64_t ruxen_race_winner_payload(int64_t state_ptr) {
    if (state_ptr == 0) {
        return 0;
    }
    return ((RuxenRaceState *)(uintptr_t)state_ptr)->winner_payload;
}

/* Free the race state. If the race was DROPPED before resolving (e.g. an outer
 * Scope cancelled the RaceFuture), CANCEL both still-live arms first — their
 * `def drop`s run, no leak. Called from `RaceFuture#drop`. */
void ruxen_race_state_free(int64_t state_ptr) {
    if (state_ptr == 0) {
        return;
    }
    RuxenRaceState *s = (RuxenRaceState *)(uintptr_t)state_ptr;
    /* Clear slots before cancelling (reentrancy discipline). */
    int64_t a = s->arm_a;
    int64_t b = s->arm_b;
    s->arm_a = 0;
    s->arm_b = 0;
    ruxen_future_drop_and_free(a);
    ruxen_future_drop_and_free(b);
    free(s);
}

/* ---------------------------------------------------------------------
 * §4.3 General `Race.any(arr)` — N-way race over a runtime-length array of
 * arms. Same semantics as `Race.of` generalised to N: first arm Ready wins;
 * ALL other live arms are cancelled (their `def drop` runs) + freed before the
 * winner is reported. Forward decls into the array runtime (vec.c). */
uint64_t ruxen_vec_len(RuxenVec *v);
int64_t ruxen_vec_get(RuxenVec *v, uint64_t index);

typedef struct RuxenRaceAnyState {
    int64_t *arms;          /* malloc'd copy of the N arm ptrs; entry 0 once consumed/cancelled */
    int64_t n;              /* number of arms */
    int64_t winner_idx;     /* 0-based index of the winning arm (valid once decided) */
    int64_t winner_payload; /* the winning Poll.Ready payload */
    int decided;            /* 0 = undecided, 1 = a winner recorded */
    int busy;               /* reentrancy guard: 1 while step/free is running a
                             * user `def drop` — a re-entrant `state_free` while
                             * `busy` defers the free instead of freeing the
                             * state out from under the running frame. */
    int free_pending;       /* a re-entrant free arrived while busy; the
                             * outermost frame performs it when busy clears. */
} RuxenRaceAnyState;

/* Copy the arm pointers out of the Ruxen `Array[Int]` (a RuxenVec*) into a
 * C-owned array, so the source vec can be dropped independently. */
int64_t ruxen_race_any_state_new(int64_t arms_vec) {
    RuxenRaceAnyState *s = (RuxenRaceAnyState *)ruxen_xmalloc(sizeof(RuxenRaceAnyState));
    RuxenVec *v = (RuxenVec *)(uintptr_t)arms_vec;
    int64_t n = (int64_t)ruxen_vec_len(v);
    if (n <= 0) {
        /* An empty race has no arm to ever complete → it would Pending
         * forever (block_on hangs). Reject it loudly rather than deadlock. */
        free(s);
        ruxen_panic("Race.any_of: empty arms array (a race with no arms never resolves)");
        return 0;
    }
    s->arms = NULL;
    if (n > 0) {
        s->arms = (int64_t *)malloc((size_t)n * sizeof(int64_t));
        if (!s->arms) {
            free(s);
            ruxen_panic("ruxen_race_any_state_new: malloc(arms) failed");
            return 0;
        }
        for (int64_t i = 0; i < n; i++) {
            s->arms[i] = ruxen_vec_get(v, (uint64_t)i);
        }
    }
    s->n = n;
    s->winner_idx = 0;
    s->winner_payload = 0;
    s->decided = 0;
    s->busy = 0;
    s->free_pending = 0;
    return (int64_t)(uintptr_t)s;
}

/* Poll each live arm in index order; first Ready wins. On a win: record the
 * winner idx+payload, then CANCEL every OTHER live arm (its `def drop` runs +
 * free). The WINNER block is LEAKED, not dropped (same rationale as the 2-arm
 * `ruxen_race_step` — the payload was moved out; running the winner's drop
 * could free a heap-pointer result → UAF). Returns winner-index + 1
 * (0 = undecided / Pending), matching the 1-based convention `Race.of` uses.
 *
 * REENTRANCY: snapshot every arm pointer into locals and ZERO every slot
 * BEFORE invoking any user `def drop` (so a re-entrant `state_free` from inside
 * a loser's drop sees all slots already 0 → no double-free / no
 * walk-over-freed-memory). */
int64_t ruxen_race_any_step(int64_t state_ptr, int64_t cx) {
    if (state_ptr == 0) {
        return 0;
    }
    RuxenRaceAnyState *s = (RuxenRaceAnyState *)(uintptr_t)state_ptr;
    if (s->decided) {
        return s->winner_idx + 1;
    }
    for (int64_t i = 0; i < s->n; i++) {
        if (s->arms[i] == 0) {
            continue;
        }
        int64_t p = Future_dynamic_poll(s->arms[i], cx);
        int64_t payload = 0;
        if (ruxen_poll_is_ready(p, &payload)) {
            s->winner_idx = i;
            s->winner_payload = payload;
            s->decided = 1;
            /* Snapshot + zero ALL slots BEFORE any user drop runs (reentrancy:
             * a loser's drop that re-enters and frees the state must see every
             * slot already 0). Then cancel each LOSER from the snapshot; LEAK
             * the winner block. */
            int64_t *snapshot = (int64_t *)ruxen_xmalloc((size_t)s->n * sizeof(int64_t));
            for (int64_t j = 0; j < s->n; j++) {
                snapshot[j] = s->arms[j];
                s->arms[j] = 0;
            }
            /* `busy` guards the state's LIFETIME: if a loser's `def drop`
             * re-enters and calls `ruxen_race_any_state_free(this)`, that free
             * is DEFERRED (free_pending) so the state isn't freed out from
             * under us — we read no state fields after the cancel loop, but the
             * caller (`poll`) reads winner_idx/payload right after, so the state
             * must outlive this call. */
            s->busy = 1;
            for (int64_t j = 0; j < s->n; j++) {
                if (j == i) {
                    continue; /* winner: leaked, not dropped */
                }
                if (snapshot[j] != 0) {
                    ruxen_future_drop_and_free(snapshot[j]); /* loser: def drop + free */
                }
            }
            free(snapshot);
            s->busy = 0;
            /* If a re-entrant free arrived during the cancel loop, the state is
             * logically dead — but `poll` still needs winner_idx/payload from
             * it. We keep the state alive here (do NOT free) and let the
             * RaceAnyFuture's own `#drop` free it later; a deferred-free flag is
             * recorded but the final free is the future's drop, which runs once
             * `poll` has consumed the result. (Single-threaded: the future is
             * borrowed during poll, so its own drop can't run until poll
             * returns — the re-entrant free can only have come from a loser's
             * drop reaching a DIFFERENT alias of this state, which the move
             * contract already forbids.) */
            return i + 1;
        }
    }
    return 0; /* all Pending */
}

int64_t ruxen_race_any_winner_idx(int64_t state_ptr) {
    if (state_ptr == 0) {
        return 0;
    }
    return ((RuxenRaceAnyState *)(uintptr_t)state_ptr)->winner_idx;
}

int64_t ruxen_race_any_winner_payload(int64_t state_ptr) {
    if (state_ptr == 0) {
        return 0;
    }
    return ((RuxenRaceAnyState *)(uintptr_t)state_ptr)->winner_payload;
}

/* Free the N-way race state, cancelling any still-live arms first (a race
 * dropped before resolving). Called from `RaceAnyFuture#drop`. */
void ruxen_race_any_state_free(int64_t state_ptr) {
    if (state_ptr == 0) {
        return;
    }
    RuxenRaceAnyState *s = (RuxenRaceAnyState *)(uintptr_t)state_ptr;
    /* Reentrancy: if a free arrives while a step's cancel loop is running this
     * state's user drops (busy), DEFER the actual free — the outer frame still
     * reads winner fields. Record the request; the owning RaceAnyFuture's own
     * `#drop` (which runs after poll returns, single-threaded) performs the
     * real free. Returning here avoids freeing the state out from under the
     * running step. */
    if (s->busy) {
        s->free_pending = 1;
        return;
    }
    if (s->arms) {
        /* Snapshot + zero every slot BEFORE any user drop (reentrancy: a
         * cancelled arm's `def drop` must not see a live slot here either).
         * Guard `busy` so a re-entrant free during a cancelled arm's drop is
         * deferred rather than double-freeing this state. */
        s->busy = 1;
        int64_t *snapshot = (int64_t *)ruxen_xmalloc((size_t)s->n * sizeof(int64_t));
        for (int64_t i = 0; i < s->n; i++) {
            snapshot[i] = s->arms[i];
            s->arms[i] = 0;
        }
        for (int64_t i = 0; i < s->n; i++) {
            if (snapshot[i] != 0) {
                ruxen_future_drop_and_free(snapshot[i]);
            }
        }
        free(snapshot);
        free(s->arms);
        s->arms = NULL;
        s->busy = 0;
    }
    free(s);
}

/* ---------------------------------------------------------------------
 * TaskHandle accessors (used by the Ruxen-level `class TaskHandle[T]`
 * lib decls and by the synthesised `__TaskJoinFuture` in commit 2). */
int64_t ruxen_task_handle_is_done(int64_t handle_ptr) {
    if (handle_ptr == 0) {
        return 0;
    }
    return ((RuxenTaskHandle *)(uintptr_t)handle_ptr)->done;
}

int64_t ruxen_task_handle_result(int64_t handle_ptr) {
    if (handle_ptr == 0) {
        return 0;
    }
    return ((RuxenTaskHandle *)(uintptr_t)handle_ptr)->result;
}

/* Caller-side drop. The Ruxen `class TaskHandle[T]` declares this as
 * its `def drop`; it drops the caller-side refcount. When the queue
 * is also done (refcount → 0), the handle is freed here. */
void ruxen_task_handle_drop(int64_t handle_ptr) {
    if (handle_ptr == 0) {
        return;
    }
    RuxenTaskHandle *h = (RuxenTaskHandle *)(uintptr_t)handle_ptr;
    h->refcount--;
    if (h->refcount == 0) {
        free(h);
    }
}
