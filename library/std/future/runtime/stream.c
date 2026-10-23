#include "../../core/runtime/runtime.h"
#include "future_rt.h"

/* §4.5 Async Stream — C helpers.
 *
 * v1 streams are mostly pure `.rx` (the `Stream` mixin + adapters in
 * `library/std/future/src/stream.rx`), so this file is intentionally small. It
 * holds the adapter inner-drop helper `ruxen_stream_drop_inner` that runs an
 * `any Stream` inner's `def drop` via the compiler-synthesised
 * `Drop_dynamic_drop` so dropping an adapter chain drops every stage (§4.5
 * Increment 4). The free-tracked test resource the stream drop fixtures use
 * lives in `library/test_std/stream_res/runtime/stream_res.c`.
 */

#include <stdint.h>

/* §4.5 Increment 4 — leak-free adapter chains. A stream ADAPTER
 * (`MapStream`/`FilterStream`/`TakeStream`) owns its inner `any Stream` by
 * value; its `def drop` calls `ruxen_stream_drop_inner(inner_ptr)` to run the
 * inner's own `def drop` (via the compiler-synthesised `Drop_dynamic_drop`,
 * routed through the Drop vtable slot the compiler now adds to EVERY Stream
 * class_info — the §4.5 Inc4 gate widening, mirroring the §4.1 Future Drop
 * slot) and then free the inner's heap block. Composes down the whole chain:
 * dropping a `TakeStream(MapStream(Source))` runs each stage's `def drop`,
 * including the source's resource teardown. Shares `ruxen_future_drop_and_free` (future_rt.h) with Race losers.
 */

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
    ruxen_future_drop_and_free(inner_ptr);
}
