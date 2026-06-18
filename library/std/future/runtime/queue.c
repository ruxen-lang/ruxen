#include "../../core/runtime/runtime.h"

/* §4.4 Async backpressured Queue — C control block + send/recv step machines.
 *
 * The single channel in-tree before this is the SYNCHRONOUS MPSC
 * (`Sender`/`Receiver`, channel.c) whose `recv` blocks the OS thread on a
 * `pthread_cond_wait` — fatal in rondo's thread-per-core model (blocking the
 * thread stalls the whole per-core reactor). This is the async-aware
 * alternative: a bounded ring buffer whose `send`/`recv` PARK THE TASK via the
 * reactor (never the OS thread). When an op would block, the Ruxen-side
 * hand-written future records the parked task's WAKER on the queue's wake slot
 * and returns Pending; the complementary op (a peer's enqueue/dequeue) wakes
 * that task through the same `ruxen_waker_wake` path spawned-task wakeups use.
 *
 * SINGLE-REACTOR v1 (spec §4.4 risk 3 / §6 open question — the largest genuine
 * unknown): producer and consumer live on ONE worker/reactor. A cross-core wake
 * (an AsyncSender on core A waking an AsyncReceiver on core B) needs a wake
 * routed to a SPECIFIC remote reactor handle, which the executor — described as
 * single-thread/per-reactor — does not evidence a primitive for. Deferred to
 * v2. Consequence of single-reactor: no mutex is needed here (the control block
 * is touched by exactly one thread), exactly like scheduler.c / executor.c. The
 * refcounts are plain int64_t (NOT real atomics) for the same reason; the
 * "SharedSync-style ATOMIC refcount" the spec sketches lands with the
 * multi-threaded scheduler round. Documented, not hidden.
 *
 * Element/error channel is i64 (the established Task/Race/Scope convention).
 * The Ruxen surface hydrates the i64 step result into `Result[Int, QueueError]`
 * (enum construction stays in .rx so the compiler owns the layout), mirroring
 * how AsyncReadWithTimeoutFuture turns a C step code into `Result[String,
 * IoError]`.
 *
 * ABI: every Ruxen-callable entry takes/returns i64 (heap pointers, the
 * class-value ABI). `cx: &var Context` flows in as its raw RuxenContext* — the
 * step reads `cx`'s waker (offset 0) to know which task to wake later.
 */

#include <stdint.h>
#include <stdlib.h>
#include <string.h>

/* Wake one parked task by its waker pointer. `ruxen_waker_wake` (executor.c)
 * marks the owning task's scheduler entry ready AND pokes the reactor wake fd —
 * exactly the path `Waker.wake` uses. We store the `cx->waker` pointer of a
 * parked op and wake it from the complementary op. `ruxen_context_waker` reads
 * the waker slot out of a RuxenContext (whose layout is private to executor.c),
 * so the queue never needs to know the Context struct internals. */
void ruxen_waker_wake(void *waker);
void *ruxen_context_waker(void *cx);

/* Overflow policy tags — MUST match `enum Overflow` declaration order in
 * library/std/future/src/queue.rx (Suspend=0, DropOldest=1, DropNewest=2).
 * Like the Poll Ready=0/Pending=1 tag contract, this is an ordering pin. The
 * Ruxen side passes `Overflow.<variant>` as a HEAP POINTER (a no-payload enum
 * value); the constructor reads the int32 tag at offset 0 of that pointer (the
 * `Shutdown` enum ABI — see ruxen_queue_new), NOT a bare i64.
 */
#define RUXEN_OVERFLOW_SUSPEND    0
#define RUXEN_OVERFLOW_DROP_OLDEST 1
#define RUXEN_OVERFLOW_DROP_NEWEST 2

/* Step result codes (small-int protocol, like ruxen_race_step's winner code).
 *   recv_step: 0 = would-block (parked, Pending) | 1 = value ready | 2 = closed
 *   send_step: 0 = would-block (full + Suspend, parked) | 1 = accepted
 *              | 2 = closed | 3 = dropped-by-policy (DropNewest rejected the
 *                                  item; the send still RESOLVES Ok — the policy
 *                                  is "drop, don't park", so the producer is not
 *                                  blocked). */
#define RUXEN_QUEUE_BLOCK   0
#define RUXEN_QUEUE_READY    1
#define RUXEN_QUEUE_CLOSED  2
#define RUXEN_QUEUE_DROPPED 3

/* ---------------------------------------------------------------------
 * The control block. One per Queue, shared by the AsyncSender + AsyncReceiver
 * ends. Heap-allocated; freed only when BOTH refcounts reach 0 (the last end
 * dropped), mirroring SharedSync / the MPSC channel control block.
 *
 * Ring buffer: `buf[(head + i) % cap]` for i in [0, count) are the live
 * elements, oldest first. `head` is the dequeue index; the enqueue index is
 * `(head + count) % cap`. */
typedef struct RuxenQueue {
    int64_t *buf;          /* ring of `capacity` i64 payloads */
    int64_t  capacity;     /* mandatory bound (no unbounded constructor) */
    int64_t  head;         /* dequeue index */
    int64_t  count;        /* live element count, 0..capacity */
    int      overflow;     /* RUXEN_OVERFLOW_* */
    int      send_closed;  /* last AsyncSender dropped (no more sends) */
    int      recv_closed;  /* AsyncReceiver dropped (no more recvs) */
    int64_t  sender_refs;  /* live AsyncSender count; 0 => send_closed */
    int64_t  receiver_refs;/* live AsyncReceiver count (1 in v1 — SPSC ends) */
    void    *recv_waker;   /* the parked recv's waker, woken by an enqueue/close.
                            * SINGLE slot — single-consumer v1 (one in-flight
                            * recv), so at most one parked receiver exists. */
    void    *send_waker;   /* the parked send's waker, woken by a dequeue/close.
                            * SINGLE slot — v1 supports at most ONE in-flight
                            * PARKED sender under Overflow.Suspend. Multiple
                            * producers (via `share`) are fine as long as they
                            * do not BLOCK simultaneously: a second producer
                            * parking on a full queue overwrites this slot and
                            * would strand the first (its waker is lost). A
                            * waiter LIST is the multi-producer-backpressure fix
                            * — deferred to v2 with the multi-threaded round
                            * (Codex high finding, documented limitation). */
    int64_t  last_taken;   /* scratch: value dequeued by the most recent recv_step
                            * READY. Read by ruxen_queue_recv_taken_value right
                            * after the step (mirrors ruxen_race_winner_payload).
                            * SOUND for v1's SINGLE-CONSUMER contract: one
                            * AsyncReceiver, one in-flight RecvFuture at a time
                            * (you `.await` a recv before issuing the next), so no
                            * two recv_steps interleave between a dequeue and its
                            * value read. Multi-consumer would need per-future
                            * state — deferred with the multi-threaded round. */
} RuxenQueue;

/* Free the control block iff no end holds it. Called from both drop paths. */
static void ruxen_queue_maybe_free(RuxenQueue *q) {
    if (q->sender_refs == 0 && q->receiver_refs == 0) {
        if (q->buf) {
            free(q->buf);
            q->buf = NULL;
        }
        free(q);
    }
}

/* ---------------------------------------------------------------------
 * Construct a bounded queue. capacity MUST be >= 1 (no unbounded variant;
 * a zero-capacity queue could never hold an element handed across a single
 * reactor turn, so reject it loudly rather than deadlock). Starts with one
 * sender ref + one receiver ref (the two ends `bounded` returns).
 *
 * `overflow` is a Ruxen `Overflow` enum VALUE. A no-payload Ruxen enum is a
 * HEAP POINTER whose first 4 bytes (int32) hold the variant tag — NOT a bare
 * i64 tag (the `Shutdown` enum / `ruxen_tcp_stream_shutdown` precedent in
 * net/runtime/tcp.c). So we read the tag with `*(int32_t *)overflow`. The tag
 * order is pinned to RUXEN_OVERFLOW_* (Suspend=0, DropOldest=1, DropNewest=2),
 * matching the `enum Overflow` declaration order in queue.rx.
 *
 * A NULL pointer or an out-of-range tag is a BUG — either the no-payload-enum
 * ABI drifted or a miscompile handed us garbage — so we PANIC loudly rather
 * than silently degrade to a default policy (matching the `Shutdown` precedent,
 * which surfaces an unknown tag rather than picking a fallback). Silent
 * fallback would mask exactly the ABI drift this runtime boundary must expose
 * (Codex medium finding). */
int64_t ruxen_queue_new(int64_t capacity, int64_t overflow) {
    if (overflow == 0) {
        ruxen_panic("Queue.bounded: null Overflow policy (enum ABI drift?)");
        return 0;
    }
    int32_t tag = *(int32_t *)(uintptr_t)overflow;
    if (tag != RUXEN_OVERFLOW_SUSPEND &&
        tag != RUXEN_OVERFLOW_DROP_OLDEST &&
        tag != RUXEN_OVERFLOW_DROP_NEWEST) {
        ruxen_panic("Queue.bounded: unknown Overflow tag (enum ABI drift?)");
        return 0;
    }
    int ov = (int)tag;
    if (capacity < 1) {
        ruxen_panic("Queue.bounded: capacity must be >= 1 (no unbounded queue)");
        return 0;
    }
    /* Guard the `capacity * sizeof(int64_t)` malloc against size_t overflow
     * (a capacity near 1<<61 would wrap to a tiny/zero allocation, then the
     * first enqueue would write OOB). The bound also keeps `head + count`
     * (both <= capacity) well within int64 range, so the ring index addition
     * cannot overflow. */
    if ((uint64_t)capacity > (uint64_t)(SIZE_MAX / sizeof(int64_t))) {
        ruxen_panic("Queue.bounded: capacity too large");
        return 0;
    }
    RuxenQueue *q = (RuxenQueue *)malloc(sizeof(RuxenQueue));
    if (!q) {
        ruxen_panic("ruxen_queue_new: malloc(queue) failed");
        return 0;
    }
    q->buf = (int64_t *)malloc((size_t)capacity * sizeof(int64_t));
    if (!q->buf) {
        free(q);
        ruxen_panic("ruxen_queue_new: malloc(buf) failed");
        return 0;
    }
    q->capacity = capacity;
    q->head = 0;
    q->count = 0;
    q->overflow = ov;
    q->send_closed = 0;
    q->recv_closed = 0;
    q->sender_refs = 1;
    q->receiver_refs = 1;
    q->recv_waker = NULL;
    q->send_waker = NULL;
    q->last_taken = 0;
    return (int64_t)(uintptr_t)q;
}

/* Enqueue `value` at the tail. Caller has verified there is room. */
static void ruxen_queue_push_tail(RuxenQueue *q, int64_t value) {
    int64_t idx = (q->head + q->count) % q->capacity;
    q->buf[idx] = value;
    q->count++;
}

/* Dequeue the head element into *out. Caller has verified count > 0. */
static int64_t ruxen_queue_pop_head(RuxenQueue *q) {
    int64_t value = q->buf[q->head];
    q->head = (q->head + 1) % q->capacity;
    q->count--;
    return value;
}

/* ---------------------------------------------------------------------
 * recv step. Drives ONE poll of a RecvFuture.
 *   - If the buffer has an element: dequeue it into the state's taken slot,
 *     wake a parked SEND (a slot just freed up), return READY.
 *   - Else if the queue is send-closed (last sender dropped) AND empty: no
 *     value will ever arrive → return CLOSED (the future resolves Err).
 *   - Else: would-block. Record this op's waker so a future enqueue/close
 *     wakes it, return BLOCK (Pending).
 *
 * Wake-before-Pending discipline (spec §4.4 risk 2): the waker is stored
 * BEFORE returning BLOCK, every poll, so a concurrent enqueue can never strand
 * the parked task. On READY the dequeued value lands in `q->last_taken`, read
 * by `ruxen_queue_recv_taken_value` (the race-winner-payload split pattern). */
int64_t ruxen_queue_recv_step(int64_t queue_ptr, int64_t cx) {
    if (queue_ptr == 0) {
        return RUXEN_QUEUE_CLOSED;
    }
    RuxenQueue *q = (RuxenQueue *)(uintptr_t)queue_ptr;

    if (q->count > 0) {
        int64_t value = ruxen_queue_pop_head(q);
        q->last_taken = value;
        /* A slot just freed — wake a parked Suspend-send, if any. */
        if (q->send_waker) {
            void *w = q->send_waker;
            q->send_waker = NULL;
            ruxen_waker_wake(w);
        }
        return RUXEN_QUEUE_READY;
    }

    if (q->send_closed) {
        /* Drained AND no more producers → done. */
        return RUXEN_QUEUE_CLOSED;
    }

    /* Empty + open → park. Record the waker BEFORE returning Pending. */
    if (cx != 0) {
        q->recv_waker = ruxen_context_waker((void *)(uintptr_t)cx);
    }
    return RUXEN_QUEUE_BLOCK;
}

/* Read the value dequeued by the most recent recv_step READY. Call ONLY after
 * a recv_step returned RUXEN_QUEUE_READY (1), before any other recv_step on the
 * same queue (single-consumer contract — see `last_taken`). */
int64_t ruxen_queue_recv_taken_value(int64_t queue_ptr) {
    if (queue_ptr == 0) {
        return 0;
    }
    return ((RuxenQueue *)(uintptr_t)queue_ptr)->last_taken;
}

/* Read the recv/send wake slot — the waker a just-returned BLOCK recorded. The
 * RecvFuture/SendFuture calls this right after a BLOCK so it can remember WHICH
 * waker it registered, and clear that exact slot in its `def drop` (the
 * cancel-while-parked UAF guard). Returns 0 if no waker is parked. */
int64_t ruxen_queue_recv_waker(int64_t queue_ptr) {
    if (queue_ptr == 0) {
        return 0;
    }
    return (int64_t)(uintptr_t)((RuxenQueue *)(uintptr_t)queue_ptr)->recv_waker;
}

int64_t ruxen_queue_send_waker(int64_t queue_ptr) {
    if (queue_ptr == 0) {
        return 0;
    }
    return (int64_t)(uintptr_t)((RuxenQueue *)(uintptr_t)queue_ptr)->send_waker;
}

/* ---------------------------------------------------------------------
 * send step. Drives ONE poll of a SendFuture for `value`.
 *   - recv-closed (receiver dropped): no consumer will ever take it → CLOSED.
 *   - room in the buffer: enqueue, wake a parked recv, return READY.
 *   - full:
 *       Suspend    → park (record waker), return BLOCK (Pending).
 *       DropOldest → evict the head element, enqueue the new one (sliding),
 *                    wake a parked recv, return READY. Net count unchanged, so
 *                    no recv was actually unblocked by space — but a NEW newest
 *                    element is now present, so a parked recv should see it.
 *       DropNewest → reject the incoming element (dropping); the send still
 *                    RESOLVES (Ok) — the policy is non-blocking. Return DROPPED.
 *
 * Wake-before-Pending: waker stored before BLOCK is returned. */
int64_t ruxen_queue_send_step(int64_t queue_ptr, int64_t cx, int64_t value) {
    if (queue_ptr == 0) {
        return RUXEN_QUEUE_CLOSED;
    }
    RuxenQueue *q = (RuxenQueue *)(uintptr_t)queue_ptr;

    if (q->recv_closed) {
        return RUXEN_QUEUE_CLOSED;
    }

    if (q->count < q->capacity) {
        ruxen_queue_push_tail(q, value);
        if (q->recv_waker) {
            void *w = q->recv_waker;
            q->recv_waker = NULL;
            ruxen_waker_wake(w);
        }
        return RUXEN_QUEUE_READY;
    }

    /* Buffer is full — apply the overflow policy. */
    if (q->overflow == RUXEN_OVERFLOW_DROP_OLDEST) {
        /* Sliding: evict head, push new newest. */
        (void)ruxen_queue_pop_head(q);
        ruxen_queue_push_tail(q, value);
        if (q->recv_waker) {
            void *w = q->recv_waker;
            q->recv_waker = NULL;
            ruxen_waker_wake(w);
        }
        return RUXEN_QUEUE_READY;
    }
    if (q->overflow == RUXEN_OVERFLOW_DROP_NEWEST) {
        /* Dropping: reject the incoming element; the send resolves anyway. */
        return RUXEN_QUEUE_DROPPED;
    }

    /* Suspend (backpressure): park the producer. Waker before Pending. */
    if (cx != 0) {
        q->send_waker = ruxen_context_waker((void *)(uintptr_t)cx);
    }
    return RUXEN_QUEUE_BLOCK;
}

/* ---------------------------------------------------------------------
 * Sender-end drop. Decrement the sender refcount; when it reaches 0 the queue
 * becomes send-closed — a parked recv must resolve Err(QueueClosed) (this is
 * what closes Go's abandoned-receiver leak: a recv blocked on a queue whose
 * producers are all gone is woken and told the channel is done). Wake any
 * parked recv so it re-polls and observes the close. Then maybe-free the
 * control block if the receiver end is also gone.
 *
 * Reentrancy: we read+clear the waker into a local BEFORE waking, so a wake
 * that re-enters can't double-wake the same slot. The control block is only
 * freed AFTER the wake (the woken task does not touch this dropped end). */
void ruxen_queue_sender_drop(int64_t queue_ptr) {
    if (queue_ptr == 0) {
        return;
    }
    RuxenQueue *q = (RuxenQueue *)(uintptr_t)queue_ptr;
    if (q->sender_refs > 0) {
        q->sender_refs--;
    }
    if (q->sender_refs == 0 && !q->send_closed) {
        q->send_closed = 1;
        /* Last producer gone — wake a parked recv to observe the close. */
        if (q->recv_waker) {
            void *w = q->recv_waker;
            q->recv_waker = NULL;
            ruxen_waker_wake(w);
        }
    }
    ruxen_queue_maybe_free(q);
}

/* Clone a sender end (multi-producer): bump the sender refcount. Returns the
 * SAME control-block pointer — both AsyncSender values share it. */
int64_t ruxen_queue_sender_clone(int64_t queue_ptr) {
    if (queue_ptr == 0) {
        return 0;
    }
    RuxenQueue *q = (RuxenQueue *)(uintptr_t)queue_ptr;
    q->sender_refs++;
    return queue_ptr;
}

/* ---------------------------------------------------------------------
 * Receiver-end drop. Decrement the receiver refcount; when it reaches 0 the
 * queue becomes recv-closed — a parked Suspend-send must resolve
 * Err(QueueClosed) (this closes Go's forgotten-sender leak: a producer blocked
 * on a full queue whose consumer is gone is woken and told the channel is
 * done). Wake any parked send so it re-polls and observes the close. */
void ruxen_queue_receiver_drop(int64_t queue_ptr) {
    if (queue_ptr == 0) {
        return;
    }
    RuxenQueue *q = (RuxenQueue *)(uintptr_t)queue_ptr;
    if (q->receiver_refs > 0) {
        q->receiver_refs--;
    }
    if (q->receiver_refs == 0 && !q->recv_closed) {
        q->recv_closed = 1;
        /* Consumer gone — wake a parked send to observe the close. */
        if (q->send_waker) {
            void *w = q->send_waker;
            q->send_waker = NULL;
            ruxen_waker_wake(w);
        }
    }
    ruxen_queue_maybe_free(q);
}

/* ---------------------------------------------------------------------
 * FFI-alias `def drop` hooks for the AsyncSender / AsyncReceiver END classes.
 *
 * The MIR drop-elaboration pass calls `<Class>_drop(self)` with the END's heap
 * pointer (the established stdlib drop ABI — `ruxen_file_drop(RuxenFile*)`,
 * `ruxen_channel_sender_drop(self)`). An AsyncSender / AsyncReceiver heap block
 * is `{ int64 queue_ptr }` (one field at offset 0), so the control-block
 * pointer is `*(int64_t *)self`. We read it and run the same close logic as the
 * raw `ruxen_queue_{sender,receiver}_drop` (which the QueueEnds untaken-ref path
 * and tests still call by control-block ptr). `self == 0` or a zeroed queue_ptr
 * (an end whose ptr was already consumed) is a no-op via the inner guard.
 *
 * Why FFI-alias and not an `.rx` body: a bootstrap-class `.rx`-body `def drop`
 * is NOT elaborated for a scope-exit local (verified — a local AsyncSender with
 * an .rx-body drop never fired its close), whereas the FFI-alias form IS, like
 * every other stdlib end/handle class. */
void ruxen_async_sender_end_drop(int64_t self) {
    if (self == 0) {
        return;
    }
    int64_t queue_ptr = *(int64_t *)(uintptr_t)self;
    ruxen_queue_sender_drop(queue_ptr);
}

void ruxen_async_receiver_end_drop(int64_t self) {
    if (self == 0) {
        return;
    }
    int64_t queue_ptr = *(int64_t *)(uintptr_t)self;
    ruxen_queue_receiver_drop(queue_ptr);
}

/* ---------------------------------------------------------------------
 * Deregister a parked op's waker slot — called from a RecvFuture/SendFuture
 * `def drop` so a CANCELLED or DROPPED parked future cannot leave a DANGLING
 * waker pointer behind. The waker the op recorded points INTO that op's task
 * Context heap block; if the task is cancelled (`ruxen_executor_cancel_task`)
 * or drained while parked, that Context is freed — a later peer
 * enqueue/dequeue would then `ruxen_waker_wake` a dangling pointer (UAF).
 *
 * We clear the slot ONLY if it still holds THIS op's waker (`==` guard): a
 * matching waker means this op is the currently-parked waiter and is going
 * away, so the slot must be vacated; a non-matching slot belongs to a
 * DIFFERENT live parked op (or was already consumed by a wake) and must be
 * left intact. `queue_ptr == 0` (an op that never parked / already cleared its
 * own ptr) is a no-op. `waker == 0` (the op never recorded a waker — completed
 * on its first poll) is a no-op. */
void ruxen_queue_cancel_recv(int64_t queue_ptr, int64_t waker) {
    if (queue_ptr == 0 || waker == 0) {
        return;
    }
    RuxenQueue *q = (RuxenQueue *)(uintptr_t)queue_ptr;
    if (q->recv_waker == (void *)(uintptr_t)waker) {
        q->recv_waker = NULL;
    }
}

void ruxen_queue_cancel_send(int64_t queue_ptr, int64_t waker) {
    if (queue_ptr == 0 || waker == 0) {
        return;
    }
    RuxenQueue *q = (RuxenQueue *)(uintptr_t)queue_ptr;
    if (q->send_waker == (void *)(uintptr_t)waker) {
        q->send_waker = NULL;
    }
}
