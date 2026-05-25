#include "../../core/runtime/runtime.h"
#include <pthread.h>

/* std.sync.Mutex[T] / MutexGuard[T] runtime.
 *
 * Layout: heap-allocated { pthread_mutex_t, int64_t payload, int poisoned }.
 * The payload is the T value carried as i64 per the existing Ruxen ABI
 * (see library/std/array/src/lib.rx "Generic-stripping at the call site"
 * comment for the canonical pattern).
 *
 * MutexGuard is a separate heap struct holding a pointer back to the
 * Mutex so guard.deref() and guard's Drop can both reach the lock.
 * Two allocations per lock cycle is suboptimal but matches v1's
 * "everything is i64 by reference" simplicity. Optimisation
 * deferred — keeps codegen unchanged.
 *
 * Poison: a panic while holding the lock would normally set
 * `poisoned`. Until unwind support lands, that path is exercised
 * only via the `poison_for_testing` test hook.
 */

typedef struct {
    pthread_mutex_t mu;
    int64_t payload;
    int poisoned;
} RuxenMutex;

typedef struct {
    RuxenMutex *parent;
} RuxenMutexGuard;

int64_t ruxen_mutex_new(int64_t initial) {
    RuxenMutex *m = (RuxenMutex *)malloc(sizeof(RuxenMutex));
    if (!m) ruxen_panic("Mutex.new: out of memory");
    if (pthread_mutex_init(&m->mu, NULL) != 0) {
        free(m);
        ruxen_panic("Mutex.new: pthread_mutex_init failed");
    }
    m->payload = initial;
    m->poisoned = 0;
    return (int64_t)m;
}

/* Mutex.lock_raw -> MutexGuard handle (i64).
 *
 * Wrapped at the Ruxen level by `lock()` (which surfaces poison via
 * Result) and `lock!()` (which panics). Runtime always returns the
 * guard; the Ruxen shim checks the poisoned bit and constructs the
 * appropriate Result/Option.
 */
int64_t ruxen_mutex_lock(int64_t mu_ptr) {
    RuxenMutex *m = (RuxenMutex *)mu_ptr;
    if (!m) ruxen_panic("Mutex.lock: null mutex");
    if (pthread_mutex_lock(&m->mu) != 0) {
        ruxen_panic("Mutex.lock: pthread_mutex_lock failed");
    }
    RuxenMutexGuard *g = (RuxenMutexGuard *)malloc(sizeof(RuxenMutexGuard));
    if (!g) {
        pthread_mutex_unlock(&m->mu);
        ruxen_panic("Mutex.lock: out of memory");
    }
    g->parent = m;
    return (int64_t)g;
}

/* Mutex.try_lock -> guard handle or 0.
 *
 * 0 = lock not acquired (Ruxen side maps to None). Non-zero = guard.
 */
int64_t ruxen_mutex_try_lock(int64_t mu_ptr) {
    RuxenMutex *m = (RuxenMutex *)mu_ptr;
    if (!m) ruxen_panic("Mutex.try_lock: null mutex");
    int rc = pthread_mutex_trylock(&m->mu);
    if (rc == EBUSY) return 0;
    if (rc != 0) ruxen_panic("Mutex.try_lock: pthread_mutex_trylock failed");
    RuxenMutexGuard *g = (RuxenMutexGuard *)malloc(sizeof(RuxenMutexGuard));
    if (!g) {
        pthread_mutex_unlock(&m->mu);
        ruxen_panic("Mutex.try_lock: out of memory");
    }
    g->parent = m;
    return (int64_t)g;
}

/* Mutex.is_poisoned -> 0/1. */
int64_t ruxen_mutex_is_poisoned(int64_t mu_ptr) {
    RuxenMutex *m = (RuxenMutex *)mu_ptr;
    return (m && m->poisoned) ? 1 : 0;
}

/* Mutex.clear_poison -> (). */
void ruxen_mutex_clear_poison(int64_t mu_ptr) {
    RuxenMutex *m = (RuxenMutex *)mu_ptr;
    if (m) m->poisoned = 0;
}

/* Mutex.into_inner -> i64 payload.
 *
 * Destroys the mutex. Caller must ensure no guards are outstanding
 * (compiler enforces via move semantics).
 */
int64_t ruxen_mutex_into_inner(int64_t mu_ptr) {
    RuxenMutex *m = (RuxenMutex *)mu_ptr;
    if (!m) ruxen_panic("Mutex.into_inner: null mutex");
    int64_t v = m->payload;
    pthread_mutex_destroy(&m->mu);
    free(m);
    return v;
}

/* Mutex drop — free the mutex itself.
 *
 * No outstanding guard at this point (typeck would have rejected
 * letting a guard outlive the mutex). Frees the pthread mutex and
 * the heap struct.
 */
void ruxen_mutex_drop(int64_t mu_ptr) {
    RuxenMutex *m = (RuxenMutex *)mu_ptr;
    if (!m) return;
    pthread_mutex_destroy(&m->mu);
    free(m);
}

/* ─── MutexGuard methods ──────────────────────────────────────── */

/* MutexGuard.get -> i64 (the payload).
 *
 * Surface-level deref() in Ruxen returns &T; the runtime returns
 * the i64 payload, the Ruxen layer constructs the reference. For
 * primitive T this is the value itself; for boxed T it's the heap
 * pointer.
 */
int64_t ruxen_mutex_guard_get(int64_t guard_ptr) {
    RuxenMutexGuard *g = (RuxenMutexGuard *)guard_ptr;
    if (!g || !g->parent) ruxen_panic("MutexGuard.deref: null guard");
    return g->parent->payload;
}

/* MutexGuard.set(v) — write through the guard.
 *
 * Ruxen-side deref_var assignment lowers to this.
 */
void ruxen_mutex_guard_set(int64_t guard_ptr, int64_t value) {
    RuxenMutexGuard *g = (RuxenMutexGuard *)guard_ptr;
    if (!g || !g->parent) ruxen_panic("MutexGuard.set: null guard");
    g->parent->payload = value;
}

/* MutexGuard drop — release the lock ONLY. The malloc'd
 * RuxenMutexGuard spine is freed by the MIR scope-exit pass via
 * `ruxen_dealloc` once `Mutex_lock_raw` is whitelisted as a fresh-
 * alloc callee (see `compiler/ruxen_core/src/mir/lower/drops.rs`
 * FRESH_ALLOC_CALLEES). Doing the free here too would double-free
 * the spine; the C side strictly handles the pthread side effect. */
void ruxen_mutex_guard_drop(int64_t guard_ptr) {
    RuxenMutexGuard *g = (RuxenMutexGuard *)guard_ptr;
    if (!g) return;
    if (g->parent) {
        pthread_mutex_unlock(&g->parent->mu);
    }
}
