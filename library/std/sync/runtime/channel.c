#include "../../core/runtime/runtime.h"
#include <pthread.h>
#include <stdatomic.h>

/* std.sync.mpsc — multi-producer single-consumer unbounded channel.
 *
 * Layout: a heap allocation containing a mutex, a condvar, a linked
 * list of slots (each holding an i64 payload), an atomic sender
 * count, and a closed-flag. Sender and Receiver handles are
 * pointers to a thin wrapper carrying a reference to this control
 * block and a per-half disposition flag.
 *
 * Channels are *not* refcounted via SharedSync — the channel keeps
 * its own sender_count and a single receiver_alive bit. This avoids
 * the double-indirection of Arc<Channel>.
 */

typedef struct ChannelSlot {
    int64_t payload;
    struct ChannelSlot *next;
} ChannelSlot;

typedef struct {
    pthread_mutex_t mu;
    pthread_cond_t  cv;
    ChannelSlot *head;
    ChannelSlot *tail;
    atomic_int sender_count;   /* # of live Sender handles */
    int receiver_alive;        /* 0/1; flipped on Receiver Drop */
} RuxenChannel;

typedef struct {
    RuxenChannel *chan;
} RuxenSender;

typedef struct {
    RuxenChannel *chan;
} RuxenReceiver;

/* Construct a (Sender, Receiver) pair.
 *
 * Returns a 2-element heap array of i64 [sender_ptr, receiver_ptr].
 * The Ruxen side destructures into the tuple at the call site.
 */
int64_t ruxen_channel_new_pair(void) {
    RuxenChannel *c = (RuxenChannel *)malloc(sizeof(RuxenChannel));
    if (!c) ruxen_panic("channel: out of memory");
    pthread_mutex_init(&c->mu, NULL);
    pthread_cond_init(&c->cv, NULL);
    c->head = NULL;
    c->tail = NULL;
    atomic_store(&c->sender_count, 1);
    c->receiver_alive = 1;

    RuxenSender *tx = (RuxenSender *)malloc(sizeof(RuxenSender));
    RuxenReceiver *rx = (RuxenReceiver *)malloc(sizeof(RuxenReceiver));
    if (!tx || !rx) ruxen_panic("channel: out of memory");
    tx->chan = c;
    rx->chan = c;

    int64_t *pair = (int64_t *)malloc(2 * sizeof(int64_t));
    if (!pair) ruxen_panic("channel: out of memory");
    pair[0] = (int64_t)tx;
    pair[1] = (int64_t)rx;
    return (int64_t)pair;
}

/* Tuple accessors used by the Ruxen shim to destructure the pair. */
int64_t ruxen_channel_pair_sender(int64_t pair_ptr) {
    int64_t *pair = (int64_t *)pair_ptr;
    return pair[0];
}
int64_t ruxen_channel_pair_receiver(int64_t pair_ptr) {
    int64_t *pair = (int64_t *)pair_ptr;
    int64_t rx = pair[1];
    free(pair);   /* tuple consumed; both handles extracted */
    return rx;
}

/* Sender.send(value) -> 0 (Ok) / 1 (Err: receiver dropped). */
int64_t ruxen_channel_send(int64_t sender_ptr, int64_t value) {
    RuxenSender *tx = (RuxenSender *)sender_ptr;
    if (!tx) ruxen_panic("Sender.send: null sender");
    RuxenChannel *c = tx->chan;

    pthread_mutex_lock(&c->mu);
    if (!c->receiver_alive) {
        pthread_mutex_unlock(&c->mu);
        return 1;  /* SendError */
    }
    ChannelSlot *slot = (ChannelSlot *)malloc(sizeof(ChannelSlot));
    if (!slot) {
        pthread_mutex_unlock(&c->mu);
        ruxen_panic("Sender.send: out of memory");
    }
    slot->payload = value;
    slot->next = NULL;
    if (c->tail) {
        c->tail->next = slot;
        c->tail = slot;
    } else {
        c->head = c->tail = slot;
    }
    pthread_cond_signal(&c->cv);
    pthread_mutex_unlock(&c->mu);
    return 0;
}

/* Sender.clone -> new Sender handle (same channel). */
int64_t ruxen_channel_sender_clone(int64_t sender_ptr) {
    RuxenSender *tx = (RuxenSender *)sender_ptr;
    if (!tx) ruxen_panic("Sender.clone: null sender");
    atomic_fetch_add(&tx->chan->sender_count, 1);
    RuxenSender *cloned = (RuxenSender *)malloc(sizeof(RuxenSender));
    if (!cloned) ruxen_panic("Sender.clone: out of memory");
    cloned->chan = tx->chan;
    return (int64_t)cloned;
}

/* Drop the channel only if both halves are gone. */
static void try_free_channel(RuxenChannel *c) {
    if (atomic_load(&c->sender_count) == 0 && !c->receiver_alive) {
        /* Drain remaining slots. */
        ChannelSlot *s = c->head;
        while (s) {
            ChannelSlot *next = s->next;
            free(s);
            s = next;
        }
        pthread_mutex_destroy(&c->mu);
        pthread_cond_destroy(&c->cv);
        free(c);
    }
}

/* Sender drop — decrement sender_count; wake any blocked recv if
 * this was the last sender. */
void ruxen_channel_sender_drop(int64_t sender_ptr) {
    RuxenSender *tx = (RuxenSender *)sender_ptr;
    if (!tx) return;
    RuxenChannel *c = tx->chan;
    int prev = atomic_fetch_sub(&c->sender_count, 1);
    if (prev == 1) {
        /* Last sender just left — wake any blocked receiver so it
         * can observe the close and return RecvError. */
        pthread_mutex_lock(&c->mu);
        pthread_cond_broadcast(&c->cv);
        pthread_mutex_unlock(&c->mu);
    }
    free(tx);
    try_free_channel(c);
}

/* Receiver.recv -> Result encoded as { tag: i32, value: i64 } via two
 * separate calls. We expose recv as returning the value when Ok and
 * is_closed as a separate query — saves the multi-value FFI.
 *
 * Returns 0 = Ok+empty-of-next-call-needed shape is fragile. The
 * cleaner approach: pack `Result[Int, RecvError]` as a 2-slot heap
 * tuple { tag, value }. We allocate that tuple and return a pointer.
 */
typedef struct {
    int64_t tag;     /* 0 = Ok, 1 = Err(RecvError) */
    int64_t value;
} RuxenRecvResult;

int64_t ruxen_channel_recv(int64_t receiver_ptr) {
    RuxenReceiver *rx = (RuxenReceiver *)receiver_ptr;
    if (!rx) ruxen_panic("Receiver.recv: null receiver");
    RuxenChannel *c = rx->chan;

    RuxenRecvResult *r = (RuxenRecvResult *)malloc(sizeof(RuxenRecvResult));
    if (!r) ruxen_panic("Receiver.recv: out of memory");

    pthread_mutex_lock(&c->mu);
    while (!c->head && atomic_load(&c->sender_count) > 0) {
        pthread_cond_wait(&c->cv, &c->mu);
    }
    if (c->head) {
        ChannelSlot *s = c->head;
        c->head = s->next;
        if (!c->head) c->tail = NULL;
        r->tag = 0;
        r->value = s->payload;
        free(s);
    } else {
        /* No data and no senders left. */
        r->tag = 1;
        r->value = 0;
    }
    pthread_mutex_unlock(&c->mu);
    return (int64_t)r;
}

/* Receiver.try_recv -> { tag: 0=Some, 1=None, value: i64 }. */
int64_t ruxen_channel_try_recv(int64_t receiver_ptr) {
    RuxenReceiver *rx = (RuxenReceiver *)receiver_ptr;
    if (!rx) ruxen_panic("Receiver.try_recv: null receiver");
    RuxenChannel *c = rx->chan;

    RuxenRecvResult *r = (RuxenRecvResult *)malloc(sizeof(RuxenRecvResult));
    if (!r) ruxen_panic("Receiver.try_recv: out of memory");

    pthread_mutex_lock(&c->mu);
    if (c->head) {
        ChannelSlot *s = c->head;
        c->head = s->next;
        if (!c->head) c->tail = NULL;
        r->tag = 0;
        r->value = s->payload;
        free(s);
    } else {
        r->tag = 1;
        r->value = 0;
    }
    pthread_mutex_unlock(&c->mu);
    return (int64_t)r;
}

/* Tuple accessors for the recv result wrapper. */
int64_t ruxen_channel_recv_result_tag(int64_t r_ptr) {
    RuxenRecvResult *r = (RuxenRecvResult *)r_ptr;
    int64_t t = r->tag;
    return t;
}
int64_t ruxen_channel_recv_result_value(int64_t r_ptr) {
    RuxenRecvResult *r = (RuxenRecvResult *)r_ptr;
    int64_t v = r->value;
    free(r);   /* tuple consumed */
    return v;
}

/* Receiver drop — mark receiver gone, wake nobody (no blocked
 * receiver to wake — that's the dropping side). */
void ruxen_channel_receiver_drop(int64_t receiver_ptr) {
    RuxenReceiver *rx = (RuxenReceiver *)receiver_ptr;
    if (!rx) return;
    RuxenChannel *c = rx->chan;
    pthread_mutex_lock(&c->mu);
    c->receiver_alive = 0;
    pthread_mutex_unlock(&c->mu);
    free(rx);
    try_free_channel(c);
}
