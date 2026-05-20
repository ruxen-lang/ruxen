#include "../../core/runtime/runtime.h"
#include <pthread.h>

/* std.sync.Thread / JoinHandle runtime.
 *
 * Concurrency primitives ship in the multithreading round (Phase 4
 * subset). pthread linkage is already wired by
 * `compiler/riven_core/src/codegen/object.rs` (always passes
 * -lpthread).
 *
 * Closure ABI (per `mir/lower/expr/closure.rs`):
 *   16-byte heap struct { int64_t fn_ptr; int64_t captures_ptr; }
 * passed as a single i64 pointer. The synthesized function takes
 * captures_ptr as its first argument and returns i64.
 *
 * Thread.spawn trampoline: extract (fn_ptr, captures_ptr), package
 * them with a result slot into a heap arg, call pthread_create on a
 * trampoline that invokes fn_ptr(captures_ptr) and stores the i64
 * return value into the result slot.
 */

typedef int64_t (*riven_closure_fn0)(int64_t captures);

typedef struct {
    int64_t fn_ptr;
    int64_t captures_ptr;
} RivenClosure;

typedef struct {
    pthread_t tid;
    int64_t result;       /* set by trampoline before exit */
    int joined;           /* 0/1; flipped by riven_join */
    int panicked;         /* 0/1; reserved for unwind support */
    char panic_msg[256];  /* reserved */
    RivenClosure closure; /* held by the spawned thread */
} RivenJoinHandle;

static void *riven_thread_trampoline(void *arg) {
    RivenJoinHandle *jh = (RivenJoinHandle *)arg;
    riven_closure_fn0 f = (riven_closure_fn0)jh->closure.fn_ptr;
    jh->result = f(jh->closure.captures_ptr);
    return NULL;
}

/* Thread.spawn(closure) -> JoinHandle (as Int handle).
 *
 * Returns a heap-allocated RivenJoinHandle pointer. The handle owns
 * the closure heap allocation for the lifetime of the spawned
 * thread; freeing it on join is the caller's job.
 */
int64_t riven_thread_spawn(int64_t closure_ptr) {
    if (!closure_ptr) {
        riven_panic("Thread.spawn: null closure");
    }
    RivenJoinHandle *jh = (RivenJoinHandle *)malloc(sizeof(RivenJoinHandle));
    if (!jh) riven_panic("Thread.spawn: out of memory");
    jh->result = 0;
    jh->joined = 0;
    jh->panicked = 0;
    jh->panic_msg[0] = '\0';
    /* Copy the closure struct so the caller's copy can move freely. */
    RivenClosure *src = (RivenClosure *)closure_ptr;
    jh->closure.fn_ptr = src->fn_ptr;
    jh->closure.captures_ptr = src->captures_ptr;

    int rc = pthread_create(&jh->tid, NULL, riven_thread_trampoline, jh);
    if (rc != 0) {
        free(jh);
        riven_panic("Thread.spawn: pthread_create failed");
    }
    return (int64_t)jh;
}

/* JoinHandle.join() -> i64 (the closure's return value).
 *
 * Blocks until the spawned thread exits. Caller surface (Result[T, ThreadPanic])
 * is wrapped at the Riven level; runtime returns the raw i64. Panic
 * propagation is deferred until unwind support lands.
 */
int64_t riven_thread_join(int64_t handle_ptr) {
    RivenJoinHandle *jh = (RivenJoinHandle *)handle_ptr;
    if (!jh) riven_panic("JoinHandle.join: null handle");
    if (jh->joined) riven_panic("JoinHandle.join: handle already joined");
    void *retval = NULL;
    int rc = pthread_join(jh->tid, &retval);
    if (rc != 0) riven_panic("JoinHandle.join: pthread_join failed");
    jh->joined = 1;
    int64_t result = jh->result;
    free(jh);
    return result;
}

/* JoinHandle drop — free without joining (detaches the thread).
 *
 * If the user drops a handle without joining, we currently leak the
 * spawned thread (pthread_detach lets it run to completion). The
 * MVP cut prioritises correctness on the join path; detach behaviour
 * is a deliberate v1 limitation.
 */
void riven_thread_join_handle_drop(int64_t handle_ptr) {
    RivenJoinHandle *jh = (RivenJoinHandle *)handle_ptr;
    if (!jh || jh->joined) return;
    pthread_detach(jh->tid);
    free(jh);
}

/* Thread.current_id() -> i64 (opaque ThreadId).
 *
 * pthread_t is an opaque type; on macOS it's `__darwin_pthread_t*`,
 * on glibc Linux it's `unsigned long`. Both fit in 8 bytes on the
 * supported targets, so casting to int64_t produces a stable id for
 * the lifetime of the calling thread.
 */
int64_t riven_thread_current_id(void) {
    pthread_t t = pthread_self();
    return (int64_t)(uintptr_t)t;
}
