#include "../../core/runtime/runtime.h"

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

/* Forward decl into compiler-synthesised dispatch helper (see
 * docs/specs/types/mixin_vtables.spec.md §B5 — the MIR pass
 * `synthesize_dynamic_dispatch_helpers` emits `Future_dynamic_poll`
 * when any class includes `Future dispatch runtime`). Signature is
 * fixed at the i64 ABI: takes the future's heap pointer and the
 * context's heap pointer, returns the poll result as the Poll[T]
 * tagged enum value at the i64 ABI (Ruxen tagged enums fit in i64
 * for v1). */
int64_t Future_dynamic_poll(int64_t self, int64_t ctx);

/* Forward decl into Context factory (executor.c). The pump call
 * needs a Context pointer to pass into each Future_dynamic_poll;
 * the AST-level block_on rewriter already constructs one per
 * block_on call, and we reuse it via thread-local storage. */
void *ruxen_executor_make_context(void);
void *ruxen_executor_make_task_context(void *task_entry);
void ruxen_executor_context_drop(void *cx);

/* Poll[T] heap layout (pinned by `poll_tag_layout_stability` in
 * compiler/ruxen_core/tests/async_surface.rs; emit lives in
 * compiler/ruxen_core/src/codegen/cranelift/emit.rs SetTag/GetTag/
 * GetPayload):
 *   offset 0: tag (i32, slot is 8-aligned)
 *     Ready = 0
 *     Pending = 1
 *   offset 8: payload (i64; for Ready arms only)
 * Ruxen enums with payloads are heap-allocated as a 16-byte block.
 * Future_dynamic_poll returns the heap pointer as i64 at the FFI
 * boundary. We don't free the Poll block here — accepting a
 * 16-byte leak per Pending poll for v1 (the Ready arm's payload
 * gets stored into the TaskHandle and the Poll header block stays
 * leaked too; total leak is bounded by total polls, not total
 * lifetime). Tracked. */
#define RUXEN_POLL_READY_TAG   0
#define RUXEN_POLL_PENDING_TAG 1

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
static _Thread_local int t_wake_all_pending = 0;

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
    RuxenTaskHandle *h = (RuxenTaskHandle *)malloc(sizeof(RuxenTaskHandle));
    if (!h) {
        ruxen_panic("ruxen_executor_spawn: malloc(handle) failed");
        return 0;
    }
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
    if (t_wake_all_pending && t_queue_head) {
        return 1;
    }
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

void ruxen_executor_wake_all_tasks(void) {
    t_wake_all_pending = 1;
}

/* ---------------------------------------------------------------------
 * Decode a Poll[T] return value. Ruxen tagged enums with payloads
 * are returned as heap pointers to {tag: i64, payload: i64}. We do
 * NOT free the pointer here — Ruxen's existing drop discipline on
 * tagged enums handles that. We read tag + payload then leak our
 * local view (the actual heap block will be GC'd or drop-elaborated
 * by the surrounding match arm in the inline poll loop — but in the
 * scheduler's case, no surrounding match exists; the Poll value is
 * an internal artifact we discard). For v1, accepting the leak of
 * one 16-byte block per Pending poll is the right ship — fixing it
 * properly requires either a dedicated `Future_dynamic_poll_raw`
 * variant that returns by-value, or wiring ruxen_dealloc here.
 *
 * UPDATE: the existing inline block_on loop's match-on-Poll fully
 * consumes the Poll value (Ready arm extracts payload, Pending arm
 * is empty), so the Poll heap block IS dropped by the match's
 * drop-elaboration pass. Our scheduler doesn't get the same
 * elaboration since we're calling Future_dynamic_poll from C —
 * accept the leak for now. Tracked.
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
    if (tag == RUXEN_POLL_READY_TAG) {
        if (out_payload) {
            *out_payload = *(int64_t *)(p + 8);
        }
        return 1;
    }
    return 0;
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
    int wake_all = t_wake_all_pending;
    t_wake_all_pending = 0;
    RuxenTaskEntry **prev_link = &t_queue_head;
    RuxenTaskEntry *cur = t_queue_head;
    while (cur) {
        if (!wake_all && !cur->ready) {
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
 * queued task's future has its drop method fire — clean up reactor
 * registrations, fds, etc. v1 ships WITHOUT calling user-defined
 * `def drop` on the dropped future heap blocks (Drop mixin is not
 * yet runtime-dispatched — see mixin_vtables.spec.md §B9 + §B11).
 * The futures' OS-side registrations leak per task in the v1 ship;
 * spec §B10 acknowledges this as the v1 trade-off. Once Drop joins
 * the runtime-dispatched mixin set, swap in the call to
 * `Drop_dynamic_drop(future_ptr)` here.
 *
 * For v1 we DO free the queue entries + drop the queue-side handle
 * refcount, but leave the future heap blocks themselves alone (no
 * way to free them without their drop fn anyway). This matches the
 * "v1 leak the future heap blocks" carve-out in the scheduler.c
 * header comment.
 */
void ruxen_executor_drain_remaining(void) {
    RuxenTaskEntry *cur = t_queue_head;
    while (cur) {
        RuxenTaskEntry *next = cur->next;
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
