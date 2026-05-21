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
 *     `RivenTaskHandle` so the caller can hold a pointer to it
 *     across the task's lifetime; the queue entry holds the same
 *     pointer so `pump` can set the done bit + write the result.
 *
 *   - `riven_executor_spawn(future_ptr)` enqueues a new task and
 *     returns the handle pointer. Does NOT poll the future.
 *
 *   - `riven_executor_pump_tasks()` walks the queue ONCE. For each
 *     live task: calls `Future_dynamic_poll(self, ctx)` (the
 *     compiler-synthesised dispatch helper for the `Future` mixin).
 *     If Ready, writes the payload into the handle's result slot
 *     and marks done; the queue entry is freed but the handle
 *     persists for the caller's `task_join`. If Pending, leaves
 *     the task in the queue.
 *
 *   - `riven_executor_queue_nonempty()` — fast bool check used by
 *     the AST-level block_on rewriter to skip the pump call when
 *     no tasks were ever spawned (zero overhead on the existing
 *     2A/2B/3 / 4 / 770 fixtures that never spawn).
 *
 *   - On `Context.drop` (end of block_on), `riven_executor_drain_remaining`
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
 * Wake routing: wake-all. The reactor wakes the calling thread on
 * any registered event; the block_on poll loop's pump call re-polls
 * every queued task on every iteration. Per-task waker routing is a
 * v2 optimisation (spec §B5; the wake state lives in the
 * RivenTaskHandle struct so adding a per-task waker later is a
 * structural change, not an ABI break).
 *
 * Recursion safety: `pump_tasks` walks by index, NOT by iterator.
 * If a polled task itself calls `Task.spawn`, the new entry is
 * pushed onto the end of the queue and `pump_tasks` will see it on
 * the same pass (or the next iteration of the outer block_on loop —
 * either is correct). Removing a completed task during the walk
 * uses the linked-list unlink pattern to keep the walk valid.
 *
 * ABI summary (all entries take/return i64 at the Riven call site):
 *   riven_executor_spawn(future_ptr: i64) -> i64   (handle pointer)
 *   riven_executor_pump_tasks() -> i64             (count of completions this pass)
 *   riven_executor_queue_nonempty() -> i64         (1 if any live task)
 *   riven_executor_drain_remaining() -> ()
 *   riven_task_handle_is_done(handle: i64) -> i64
 *   riven_task_handle_result(handle: i64) -> i64
 *   riven_task_handle_drop(handle: i64) -> ()
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
 * tagged enum value at the i64 ABI (Riven tagged enums fit in i64
 * for v1). */
int64_t Future_dynamic_poll(int64_t self, int64_t ctx);

/* Forward decl into Context factory (executor.c). The pump call
 * needs a Context pointer to pass into each Future_dynamic_poll;
 * the AST-level block_on rewriter already constructs one per
 * block_on call, and we reuse it via thread-local storage. */
void *riven_executor_make_context(void);
void riven_executor_context_drop(void *cx);

/* Poll[T] heap layout (pinned by `poll_tag_layout_stability` in
 * compiler/riven_core/tests/async_surface.rs; emit lives in
 * compiler/riven_core/src/codegen/cranelift/emit.rs SetTag/GetTag/
 * GetPayload):
 *   offset 0: tag (i32, slot is 8-aligned)
 *     Ready = 0
 *     Pending = 1
 *   offset 8: payload (i64; for Ready arms only)
 * Riven enums with payloads are heap-allocated as a 16-byte block.
 * Future_dynamic_poll returns the heap pointer as i64 at the FFI
 * boundary. We don't free the Poll block here — accepting a
 * 16-byte leak per Pending poll for v1 (the Ready arm's payload
 * gets stored into the TaskHandle and the Poll header block stays
 * leaked too; total leak is bounded by total polls, not total
 * lifetime). Tracked. */
#define RIVEN_POLL_READY_TAG   0
#define RIVEN_POLL_PENDING_TAG 1

/* ---------------------------------------------------------------------
 * Task handle (Riven-visible). Caller-held; outlives the queue entry.
 *
 * Layout MUST match the C-side ABI assumed by the Riven-level
 * `class TaskHandle[T]` lib decls in library/std/future/src/lib.rvn.
 * Keep this struct private to scheduler.c — Riven sees only the
 * opaque pointer. */
typedef struct RivenTaskHandle {
    int64_t done;        /* 0 = pending, 1 = ready */
    int64_t result;      /* valid only when done == 1 */
    int64_t refcount;    /* 2 on spawn (queue + caller), drops to 0 frees */
} RivenTaskHandle;

/* ---------------------------------------------------------------------
 * Queue entry. Lives on the heap. Linked-list intrusive next pointer.
 *
 * `future_ptr` is the heap pointer to the future's instance — the
 * class_info_ptr lives at *(void**)future_ptr per mixin_vtables
 * §B2. Future_dynamic_poll reads it. */
typedef struct RivenTaskEntry {
    int64_t future_ptr;
    RivenTaskHandle *handle;
    struct RivenTaskEntry *next;
} RivenTaskEntry;

/* Per-thread queue head + a non-NULL ctx pointer reused across all
 * pump calls. The ctx is lazily created on first spawn (we cannot
 * reach into the block_on rewriter's local ctx — the rewriter's ctx
 * is on the C stack at block_on time, not addressable from C). Spec
 * §B5 says Context.test_dummy's waker is no-op, and the executor
 * Context's waker is also no-op in v1, so any Context will do — we
 * use one allocated by riven_executor_make_context on first spawn
 * and free it in drain_remaining. */
static _Thread_local RivenTaskEntry *t_queue_head = NULL;
static _Thread_local void *t_pump_ctx = NULL;
static _Thread_local int t_pump_in_progress = 0;

/* ---------------------------------------------------------------------
 * Spawn.
 *
 * Allocates the queue entry + handle, pushes onto the tail of the
 * queue. Returns the handle pointer to Riven as an i64. The future
 * pointer's lifetime is now owned by the queue — the caller's local
 * binding for the future must fall out of scope (per spec §B1 —
 * Task.spawn moves the future into the queue). */
int64_t riven_executor_spawn(int64_t future_ptr) {
    if (future_ptr == 0) {
        riven_panic("riven_executor_spawn: null future");
        return 0;
    }
    RivenTaskHandle *h = (RivenTaskHandle *)malloc(sizeof(RivenTaskHandle));
    if (!h) {
        riven_panic("riven_executor_spawn: malloc(handle) failed");
        return 0;
    }
    h->done = 0;
    h->result = 0;
    h->refcount = 2; /* queue + caller */

    RivenTaskEntry *e = (RivenTaskEntry *)malloc(sizeof(RivenTaskEntry));
    if (!e) {
        free(h);
        riven_panic("riven_executor_spawn: malloc(entry) failed");
        return 0;
    }
    e->future_ptr = future_ptr;
    e->handle = h;
    e->next = NULL;

    /* Append to tail. Round-robin = FIFO walk, so newer tasks land
     * at the end and won't be polled before the existing ones get
     * their fair shake. */
    if (!t_queue_head) {
        t_queue_head = e;
    } else {
        RivenTaskEntry *cur = t_queue_head;
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
 * to match the Riven Bool ABI. */
int64_t riven_executor_queue_nonempty(void) {
    return t_queue_head ? 1 : 0;
}

/* ---------------------------------------------------------------------
 * Lazy ctx getter. v1 reuses the no-op-waker Context for all pump
 * calls. The ctx persists across pump calls and is freed in
 * drain_remaining. */
static void *get_or_init_pump_ctx(void) {
    if (!t_pump_ctx) {
        t_pump_ctx = riven_executor_make_context();
    }
    return t_pump_ctx;
}

/* ---------------------------------------------------------------------
 * Decode a Poll[T] return value. Riven tagged enums with payloads
 * are returned as heap pointers to {tag: i64, payload: i64}. We do
 * NOT free the pointer here — Riven's existing drop discipline on
 * tagged enums handles that. We read tag + payload then leak our
 * local view (the actual heap block will be GC'd or drop-elaborated
 * by the surrounding match arm in the inline poll loop — but in the
 * scheduler's case, no surrounding match exists; the Poll value is
 * an internal artifact we discard). For v1, accepting the leak of
 * one 16-byte block per Pending poll is the right ship — fixing it
 * properly requires either a dedicated `Future_dynamic_poll_raw`
 * variant that returns by-value, or wiring riven_dealloc here.
 *
 * UPDATE: the existing inline block_on loop's match-on-Poll fully
 * consumes the Poll value (Ready arm extracts payload, Pending arm
 * is empty), so the Poll heap block IS dropped by the match's
 * drop-elaboration pass. Our scheduler doesn't get the same
 * elaboration since we're calling Future_dynamic_poll from C —
 * accept the leak for now. Tracked.
 */
static int riven_poll_is_ready(int64_t poll_val, int64_t *out_payload) {
    if (poll_val == 0) {
        /* Defensive: shouldn't happen — Future_dynamic_poll always
         * returns a valid Poll. Treat as Pending. */
        return 0;
    }
    /* Tag is i32 at offset 0; payload i64 at offset 8 (cranelift emit
     * in compiler/riven_core/src/codegen/cranelift/emit.rs::SetTag /
     * GetTag / GetPayload). Reading the tag as i32 avoids picking up
     * 4 bytes of uninitialised slop after the tag word. */
    char *p = (char *)(uintptr_t)poll_val;
    int32_t tag = *(int32_t *)(p + 0);
    if (tag == RIVEN_POLL_READY_TAG) {
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
int64_t riven_executor_pump_tasks(void) {
    if (!t_queue_head) {
        return 0;
    }
    if (t_pump_in_progress) {
        /* Defensive: pump-from-within-pump. Bail. */
        return 0;
    }
    t_pump_in_progress = 1;

    void *ctx = get_or_init_pump_ctx();
    if (!ctx) {
        t_pump_in_progress = 0;
        return 0;
    }

    int64_t completions = 0;
    RivenTaskEntry **prev_link = &t_queue_head;
    RivenTaskEntry *cur = t_queue_head;
    while (cur) {
        int64_t poll_val = Future_dynamic_poll(cur->future_ptr, (int64_t)(uintptr_t)ctx);
        int64_t payload = 0;
        if (riven_poll_is_ready(poll_val, &payload)) {
            cur->handle->result = payload;
            cur->handle->done = 1;
            /* Queue-side refcount drop. */
            cur->handle->refcount--;
            if (cur->handle->refcount == 0) {
                free(cur->handle);
            }
            RivenTaskEntry *next = cur->next;
            *prev_link = next;
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
void riven_executor_drain_remaining(void) {
    RivenTaskEntry *cur = t_queue_head;
    while (cur) {
        RivenTaskEntry *next = cur->next;
        /* Decrement handle refcount; free if no caller holds it
         * (rare — usually the caller has dropped its TaskHandle by
         * the time block_on exits; the join-fence in B6 is the
         * recommended pattern). */
        cur->handle->refcount--;
        if (cur->handle->refcount == 0) {
            free(cur->handle);
        }
        free(cur);
        cur = next;
    }
    t_queue_head = NULL;
    if (t_pump_ctx) {
        riven_executor_context_drop(t_pump_ctx);
        /* The Context heap block itself isn't freed by context_drop
         * (it relies on Riven's drop-elaboration to call riven_dealloc
         * after the drop fn). We're calling from C, no elaboration —
         * free explicitly. */
        free(t_pump_ctx);
        t_pump_ctx = NULL;
    }
}

/* ---------------------------------------------------------------------
 * TaskHandle accessors (used by the Riven-level `class TaskHandle[T]`
 * lib decls and by the synthesised `__TaskJoinFuture` in commit 2). */
int64_t riven_task_handle_is_done(int64_t handle_ptr) {
    if (handle_ptr == 0) {
        return 0;
    }
    return ((RivenTaskHandle *)(uintptr_t)handle_ptr)->done;
}

int64_t riven_task_handle_result(int64_t handle_ptr) {
    if (handle_ptr == 0) {
        return 0;
    }
    return ((RivenTaskHandle *)(uintptr_t)handle_ptr)->result;
}

/* Caller-side drop. The Riven `class TaskHandle[T]` declares this as
 * its `def drop`; it drops the caller-side refcount. When the queue
 * is also done (refcount → 0), the handle is freed here. */
void riven_task_handle_drop(int64_t handle_ptr) {
    if (handle_ptr == 0) {
        return;
    }
    RivenTaskHandle *h = (RivenTaskHandle *)(uintptr_t)handle_ptr;
    h->refcount--;
    if (h->refcount == 0) {
        free(h);
    }
}
