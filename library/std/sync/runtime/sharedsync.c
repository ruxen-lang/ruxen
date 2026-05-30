#include "../../core/runtime/runtime.h"
#include <stdatomic.h>

/* std.sync.SharedSync[T] runtime (atomically-refcounted Arc).
 *
 * Layout: a heap allocation with an 8-byte atomic refcount header
 * followed by the i64 payload. Clone increments via atomic
 * fetch_add; Drop decrements via fetch_sub and frees on zero.
 *
 * The handle exposed to Ruxen is a pointer to the *payload* (the
 * header sits at offset -8). This lets deref() return the same i64
 * shape Mutex.guard.get returns — keeps the codegen indirection
 * consistent. The header is reached via pointer subtraction.
 */

typedef struct {
    atomic_int_fast64_t refcount;
    int64_t payload;
} RuxenSharedSync;

int64_t ruxen_sharedsync_new(int64_t initial) {
    RuxenSharedSync *s = (RuxenSharedSync *)malloc(sizeof(RuxenSharedSync));
    if (!s) ruxen_panic("SharedSync.new: out of memory");
    atomic_store_explicit(&s->refcount, 1, memory_order_release);
    s->payload = initial;
    return (int64_t)s;
}

int64_t ruxen_sharedsync_clone(int64_t ptr) {
    RuxenSharedSync *s = (RuxenSharedSync *)ptr;
    if (!s) ruxen_panic("SharedSync.clone: null handle");
    atomic_fetch_add_explicit(&s->refcount, 1, memory_order_acq_rel);
    return (int64_t)s;
}

int64_t ruxen_sharedsync_strong_count(int64_t ptr) {
    RuxenSharedSync *s = (RuxenSharedSync *)ptr;
    if (!s) return 0;
    return (int64_t)atomic_load_explicit(&s->refcount, memory_order_acquire);
}

int64_t ruxen_sharedsync_get(int64_t ptr) {
    RuxenSharedSync *s = (RuxenSharedSync *)ptr;
    if (!s) ruxen_panic("SharedSync.deref: null handle");
    return s->payload;
}

/* SharedSync drop — decrement and free on zero.
 *
 * On the final drop, the payload is left untouched. The Ruxen
 * codegen is responsible for emitting the payload's own drop
 * (calling T's drop on the i64 if T owns heap) before the
 * SharedSync drop runs. This matches the Vec[T] story today:
 * runtime frees the outer container; recursive element drops are
 * codegen-druxen.
 */
void ruxen_sharedsync_drop(int64_t ptr) {
    RuxenSharedSync *s = (RuxenSharedSync *)ptr;
    if (!s) return;
    int_fast64_t prev = atomic_fetch_sub_explicit(&s->refcount, 1, memory_order_acq_rel);
    if (prev == 1) {
        /* Last reference — free the allocation. */
        free(s);
    }
}
