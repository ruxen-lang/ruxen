#include "../../../std/core/runtime/runtime.h"

/* Test-only stream resource for the §4.5 stream drop fixtures
 * (tests/release-e2e/cases/434_stream_drop_on_cancel.rx,
 * 436_stream_chain_drop.rx). Moved out of library/std/future/runtime/stream.c
 * so the production runtime carries neither these symbols nor their printf.
 * A fixture links it by opting in through `RUXEN_TEST_STDLIB`. */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

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
