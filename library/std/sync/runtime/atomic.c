#include "../../core/runtime/runtime.h"
#include <stdatomic.h>

/* std.sync.atomic — lock-free primitives (sequentially consistent).
 *
 * Every type is a heap-allocated wrapper so the handle on the FFI
 * boundary is a pointer (i64). No memory-ordering surface in this
 * round — every op is SEQ_CST.
 *
 * AtomicUsize has no C of its own: its `lib` block aliases the AtomicI64
 * symbols. Both are one 64-bit word, and C11 atomic arithmetic on a
 * signed type wraps in two's complement (7.17.7.5), so the bit patterns
 * an unsigned implementation would produce are identical.
 */

typedef struct { atomic_int_fast64_t v; } RuxenAtomicI64;
typedef struct { atomic_bool        v; } RuxenAtomicBool;

/* ─── AtomicI64 (also AtomicUsize) ────────────────────────────── */

int64_t ruxen_atomic_i64_new(int64_t initial) {
    RuxenAtomicI64 *a = (RuxenAtomicI64 *)malloc(sizeof(RuxenAtomicI64));
    if (!a) ruxen_panic("AtomicI64.new: out of memory");
    atomic_store(&a->v, initial);
    return (int64_t)a;
}

int64_t ruxen_atomic_i64_load(int64_t ptr) {
    RuxenAtomicI64 *a = (RuxenAtomicI64 *)ptr;
    return (int64_t)atomic_load(&a->v);
}

void ruxen_atomic_i64_store(int64_t ptr, int64_t v) {
    RuxenAtomicI64 *a = (RuxenAtomicI64 *)ptr;
    atomic_store(&a->v, v);
}

int64_t ruxen_atomic_i64_fetch_add(int64_t ptr, int64_t delta) {
    RuxenAtomicI64 *a = (RuxenAtomicI64 *)ptr;
    return (int64_t)atomic_fetch_add(&a->v, delta);
}

int64_t ruxen_atomic_i64_fetch_sub(int64_t ptr, int64_t delta) {
    RuxenAtomicI64 *a = (RuxenAtomicI64 *)ptr;
    return (int64_t)atomic_fetch_sub(&a->v, delta);
}

int64_t ruxen_atomic_i64_compare_and_swap(int64_t ptr, int64_t current, int64_t new_val) {
    RuxenAtomicI64 *a = (RuxenAtomicI64 *)ptr;
    int_fast64_t expected = current;
    /* atomic_compare_exchange_strong updates `expected` on failure
       with the actual prior value. We return that prior either way;
       the Ruxen side compares to `current` to learn whether the
       swap happened. */
    atomic_compare_exchange_strong(&a->v, &expected, new_val);
    return (int64_t)expected;
}

void ruxen_atomic_i64_drop(int64_t ptr) {
    RuxenAtomicI64 *a = (RuxenAtomicI64 *)ptr;
    if (a) free(a);
}

/* ─── AtomicBool ──────────────────────────────────────────────── */

int64_t ruxen_atomic_bool_new(int64_t initial) {
    RuxenAtomicBool *a = (RuxenAtomicBool *)malloc(sizeof(RuxenAtomicBool));
    if (!a) ruxen_panic("AtomicBool.new: out of memory");
    atomic_store(&a->v, initial != 0);
    return (int64_t)a;
}

int64_t ruxen_atomic_bool_load(int64_t ptr) {
    RuxenAtomicBool *a = (RuxenAtomicBool *)ptr;
    return atomic_load(&a->v) ? 1 : 0;
}

void ruxen_atomic_bool_store(int64_t ptr, int64_t v) {
    RuxenAtomicBool *a = (RuxenAtomicBool *)ptr;
    atomic_store(&a->v, v != 0);
}

/* C11 stdatomic.h defines `atomic_fetch_and` / `atomic_fetch_or` only
 * on INTEGER atomic types (atomic_int, atomic_long, …) — not on
 * `atomic_bool`. Clang on macOS accepts the bool form as an extension;
 * GCC on Linux rejects it ("operand type `_Atomic _Bool *` is
 * incompatible with argument 1 of `__atomic_fetch_and`"), breaking
 * the ubuntu CI build. Implement bool fetch_and/or via a portable
 * compare-exchange loop instead — same semantics, both compilers happy.
 */
int64_t ruxen_atomic_bool_fetch_and(int64_t ptr, int64_t v) {
    RuxenAtomicBool *a = (RuxenAtomicBool *)ptr;
    bool desired_mask = (v != 0);
    bool expected = atomic_load(&a->v);
    while (!atomic_compare_exchange_weak(&a->v, &expected, expected && desired_mask)) {
        /* `expected` was updated to the current value by the failed CAS;
         * retry with the new observed value. */
    }
    return expected ? 1 : 0;
}

int64_t ruxen_atomic_bool_fetch_or(int64_t ptr, int64_t v) {
    RuxenAtomicBool *a = (RuxenAtomicBool *)ptr;
    bool desired_mask = (v != 0);
    bool expected = atomic_load(&a->v);
    while (!atomic_compare_exchange_weak(&a->v, &expected, expected || desired_mask)) {
        /* see fetch_and above */
    }
    return expected ? 1 : 0;
}

int64_t ruxen_atomic_bool_compare_and_swap(int64_t ptr, int64_t current, int64_t new_val) {
    RuxenAtomicBool *a = (RuxenAtomicBool *)ptr;
    bool expected = (current != 0);
    atomic_compare_exchange_strong(&a->v, &expected, new_val != 0);
    return expected ? 1 : 0;
}

void ruxen_atomic_bool_drop(int64_t ptr) {
    RuxenAtomicBool *a = (RuxenAtomicBool *)ptr;
    if (a) free(a);
}
