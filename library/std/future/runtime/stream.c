#include "../../core/runtime/runtime.h"

/* §4.5 Async Stream — C helpers.
 *
 * v1 streams are mostly pure `.rx` (the `Stream` mixin + adapters in
 * `library/std/future/src/stream.rx`), so this file is intentionally small. It
 * exists for (1) source streams that own a C-side resource (a reactor
 * registration / fd / opaque state) and tear it down in `def drop` — the
 * leak-free-on-cancel story (§4.5 Increment 3), and (2) the adapter inner-drop
 * helper `ruxen_stream_drop_inner` that runs an `any Stream` inner's `def drop`
 * via the compiler-synthesised `Drop_dynamic_drop` so dropping an adapter chain
 * drops every stage (§4.5 Increment 4).
 */

#include <stdint.h>
#include <stdlib.h>
#include <stdio.h>

/* §4.5 Increment 4 — leak-free adapter chains. A stream ADAPTER
 * (`MapStream`/`FilterStream`/`TakeStream`) owns its inner `any Stream` by
 * value; its `def drop` calls `ruxen_stream_drop_inner(inner_ptr)` to run the
 * inner's own `def drop` (via the compiler-synthesised `Drop_dynamic_drop`,
 * routed through the Drop vtable slot the compiler now adds to EVERY Stream
 * class_info — the §4.5 Inc4 gate widening, mirroring the §4.1 Future Drop
 * slot) and then free the inner's heap block. Composes down the whole chain:
 * dropping a `TakeStream(MapStream(Source))` runs each stage's `def drop`,
 * including the source's resource teardown. Mirrors `ruxen_race_cancel_arm`.
 *
 * `Drop_dynamic_drop` is the SAME compiler-synthesised helper the scheduler
 * drain/cancel + race loser-cleanup call; declared `__weak` so a program with
 * no async-protocol class at all (helper never emitted) still links — then the
 * call is a guarded no-op (a program using a Stream adapter always emits it). */
__attribute__((weak)) void Drop_dynamic_drop(int64_t self);

/* Run an adapter's inner `any Stream` `def drop` + free its heap block. NULL
 * inner (never set) is a no-op. Each adapter's `def drop` runs EXACTLY ONCE per
 * instance (an adapter value is dropped once — by scope-exit drop-elaboration
 * when owned by a local, or by this same helper when it is itself another
 * adapter's inner; it is not reachable from two owners at once, since each stage
 * is MOVED into the next at construction), so no field-zeroing / re-entrancy
 * guard is needed here: this fires once for a given inner. The inner's
 * `def drop` is arbitrary user code; we hold only the raw `inner_ptr` local
 * across it and free the block after it returns. */
void ruxen_stream_drop_inner(int64_t inner_ptr) {
    if (inner_ptr == 0) {
        return;
    }
    if (Drop_dynamic_drop) {
        Drop_dynamic_drop(inner_ptr);
    }
    ruxen_dealloc((void *)(uintptr_t)inner_ptr);
}

/* A minimal owned "resource": a heap cell tagged with an id. `_new` allocates
 * (the stand-in for registering a reactor fd / timer); `_free` releases it and
 * prints a sentinel so a stream's `def drop` has an observable, free-tracked
 * effect. Single-threaded v1, no locking (the rest of the future runtime). */
typedef struct RuxenStreamRes {
    int64_t tag;
} RuxenStreamRes;

/* §4.5 Inc4 double-drop guard: a process-wide LIVE count a pin reads after a
 * chain drop to prove the source `def drop` fired EXACTLY ONCE (no double-drop
 * via both the adapter chain AND a stray scope-exit drop of the moved-in
 * source). `_new` bumps it; `_free` decrements; `ruxen_stream_live_count`
 * reads it. A double-free drives it negative; a leak leaves it > 0. */
static int64_t t_stream_live = 0;

int64_t ruxen_stream_res_new(int64_t tag) {
    RuxenStreamRes *r = (RuxenStreamRes *)malloc(sizeof(RuxenStreamRes));
    if (!r) {
        ruxen_panic("ruxen_stream_res_new: malloc failed");
        return 0;
    }
    r->tag = tag;
    t_stream_live++;
    return (int64_t)(uintptr_t)r;
}

void ruxen_stream_res_free(int64_t state_ptr) {
    if (state_ptr == 0) {
        return;
    }
    RuxenStreamRes *r = (RuxenStreamRes *)(uintptr_t)state_ptr;
    printf("res freed tag=%lld\n", (long long)r->tag);
    fflush(stdout);
    t_stream_live--;
    free(r);
}

int64_t ruxen_stream_live_count(void) {
    return t_stream_live;
}
