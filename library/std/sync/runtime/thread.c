#include "../../core/runtime/runtime.h"
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>

/* Default thread stack size for Thread.spawn.
 *
 * Match macOS's pthread default (512 KB) so user FFI handlers don't
 * SIGSEGV on the deeper stacks LLVM-generated code can want. The
 * earlier 256 KB shaved a small amount off pthread_create cost but
 * was below the OS default — risky once any non-trivial user code
 * is on the spawned stack (large structs on the stack, deep
 * recursive parsers, LLVM intrinsics with high stack usage).
 *
 * On Linux glibc's default is 8 MB. Capping at 512 KB is still a
 * meaningful reduction there; the actual page footprint is lazy
 * via overcommit so the smaller upper bound costs nothing per
 * thread that doesn't use it.
 *
 * Userland that needs more can override via RIVEN_THREAD_STACK_KB
 * at process start. Setting RIVEN_THREAD_STACK_KB=0 falls back to
 * the OS default — useful when debugging stack-overflow suspicions
 * in user code on Linux (where the OS default is 8 MB).
 */
#define RIVEN_DEFAULT_THREAD_STACK_BYTES (512u * 1024u)

static size_t riven_thread_stack_size(void) {
    static size_t cached = 0;
    static int loaded = 0;
    if (loaded) return cached;
    /* Race-tolerant: worst case two threads compute the same value
     * before publishing. The value is a deterministic function of
     * env vars, so the store order doesn't matter. */
    const char *env = getenv("RIVEN_THREAD_STACK_KB");
    if (env && *env) {
        char *endp = NULL;
        long kb = strtol(env, &endp, 10);
        if (endp != env && kb >= 0) {
            cached = (size_t)kb * 1024u;
            loaded = 1;
            return cached;
        }
    }
    cached = RIVEN_DEFAULT_THREAD_STACK_BYTES;
    loaded = 1;
    return cached;
}

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

/* W15 fix: the spawned thread reads from a SEPARATE allocation
 * (`RivenSpawnCtx`) that lives independently of the caller's
 * JoinHandle. The caller's MIR drop-elab is free to call
 * `JoinHandle_drop` + `riven_dealloc` on `jh` at any time — the
 * thread keeps running because it never touches `jh` again after
 * trampoline entry. The spawn ctx is freed by the trampoline
 * itself after the closure body returns. The result slot lives in
 * the ctx (not jh) so join can read it without racing against the
 * caller's free. */
typedef struct {
    int64_t fn_ptr;
    int64_t captures_ptr;
    /* Result is published here by the trampoline. join_raw reads
     * it via the back-link held in the JoinHandle. */
    int64_t result;
    /* refcount: one ref for the spawned thread (trampoline releases
     * it after body), one for the caller's JoinHandle (drop / join
     * releases). Whoever takes the count to zero frees ctx. */
    atomic_int refcount;
} RivenSpawnCtx;

typedef struct {
    pthread_t tid;
    int joined;             /* 0/1; flipped by riven_join */
    int panicked;           /* 0/1; reserved for unwind support */
    char panic_msg[256];    /* reserved */
    RivenSpawnCtx *ctx;     /* points to the independently-owned ctx */
} RivenJoinHandle;

static void riven_spawn_ctx_release(RivenSpawnCtx *ctx) {
    if (atomic_fetch_sub_explicit(&ctx->refcount, 1, memory_order_acq_rel) == 1) {
        free(ctx);
    }
}

static void *riven_thread_trampoline(void *arg) {
    RivenSpawnCtx *ctx = (RivenSpawnCtx *)arg;
    riven_closure_fn0 f = (riven_closure_fn0)(uintptr_t)ctx->fn_ptr;
    ctx->result = f(ctx->captures_ptr);
    riven_spawn_ctx_release(ctx);
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
    RivenClosure *src = (RivenClosure *)(uintptr_t)closure_ptr;
    /* Independent spawn ctx (survives caller's JoinHandle drop). */
    RivenSpawnCtx *ctx = (RivenSpawnCtx *)malloc(sizeof(RivenSpawnCtx));
    if (!ctx) riven_panic("Thread.spawn: out of memory (ctx)");
    ctx->fn_ptr = src->fn_ptr;
    ctx->captures_ptr = src->captures_ptr;
    ctx->result = 0;
    atomic_store_explicit(&ctx->refcount, 2, memory_order_relaxed);

    RivenJoinHandle *jh = (RivenJoinHandle *)malloc(sizeof(RivenJoinHandle));
    if (!jh) {
        free(ctx);
        riven_panic("Thread.spawn: out of memory (jh)");
    }
    jh->joined = 0;
    jh->panicked = 0;
    jh->panic_msg[0] = '\0';
    jh->ctx = ctx;

    pthread_attr_t attr;
    pthread_attr_t *attrp = NULL;
    size_t stack_bytes = riven_thread_stack_size();
    if (stack_bytes > 0 && pthread_attr_init(&attr) == 0) {
        /* Quietly skip the stack-size hint if libc rejects it (e.g.
         * value below PTHREAD_STACK_MIN on a platform with a larger
         * minimum than we expect). pthread_create with the unmodified
         * attr is still a no-op vs. NULL on that path. */
        if (pthread_attr_setstacksize(&attr, stack_bytes) == 0) {
            attrp = &attr;
        } else {
            pthread_attr_destroy(&attr);
        }
    }
    int rc = pthread_create(&jh->tid, attrp, riven_thread_trampoline, ctx);
    if (attrp) pthread_attr_destroy(attrp);
    if (rc != 0) {
        free(jh);
        free(ctx);
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
    RivenJoinHandle *jh = (RivenJoinHandle *)(uintptr_t)handle_ptr;
    if (!jh) riven_panic("JoinHandle.join: null handle");
    if (jh->joined) riven_panic("JoinHandle.join: handle already joined");
    void *retval = NULL;
    int rc = pthread_join(jh->tid, &retval);
    if (rc != 0) riven_panic("JoinHandle.join: pthread_join failed");
    jh->joined = 1;
    int64_t result = jh->ctx->result;
    /* W15: release the caller's ref on the spawn ctx. Trampoline
     * already released its own ref when it returned, so this is
     * the last ref and the ctx is freed here. The JoinHandle
     * struct itself is freed by MIR-emitted riven_dealloc — we
     * just let that happen. */
    riven_spawn_ctx_release(jh->ctx);
    return result;
}

/* JoinHandle drop — detach the thread, drop the caller's ref on
 * the spawn ctx. The JoinHandle struct itself is freed by the
 * MIR-emitted riven_dealloc that follows; we just need to make
 * sure the SPAWN CTX outlives us, which the refcount handles.
 */
void riven_thread_join_handle_drop(int64_t handle_ptr) {
    RivenJoinHandle *jh = (RivenJoinHandle *)(uintptr_t)handle_ptr;
    if (!jh || jh->joined) return;
    pthread_detach(jh->tid);
    /* Spawn ctx might already have been released by the trampoline
     * if the thread finished before us — atomic refcount handles
     * the race. */
    riven_spawn_ctx_release(jh->ctx);
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
