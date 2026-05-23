/*
 * std::async_net runtime — backs AsyncTcpListener / AsyncTcpStream +
 * five hand-written futures (bind / accept / connect / read / write /
 * close) declared in library/std/async_net/src/lib.rvn.
 *
 * Spec: docs/specs/stdlib/async_io.spec.md Milestone 4C (B6.5–B12).
 *
 * Design mirrors async_fs.c:
 *   - Two thin wrapper classes (RivenAsyncTcpListener / Stream) with
 *     identical 8-byte (int32 fd + int32 closed) wire shape as
 *     RivenAsyncFile so future BufReader / cross-package work can cast
 *     across without an adapter.
 *   - Each blocking operation has an opaque C state struct held on the
 *     Riven future class as an Int pointer. State `step` calls return
 *     {0 progress, 1 EAGAIN, 2 done, 3 error}. The Riven side caches
 *     a reactor handle (negative-encoded slot index from
 *     riven_reactor_register_fd_{read,write}) and re-parks across
 *     wake cycles.
 *
 * Reactor coupling: NO reactor.c extensions. The five futures all use
 * the existing fd-readiness primitives shipped in sub-phase 4B:
 *   riven_reactor_register_fd_read(reactor, fd) -> handle
 *   riven_reactor_register_fd_write(reactor, fd) -> handle
 *   riven_reactor_check_fired(reactor, handle) -> 0/1 (declared in
 *     library/std/time/src/lib.rvn from 4A; reused here)
 *   riven_reactor_deregister(reactor, handle) -> () (same)
 *
 * Surface deviation from spec B8: the v1 read surface is
 *   read(max_bytes: Int) -> Result[String, IoError]
 * rather than the spec's
 *   read(&var Array[Int]) -> Result[Int, IoError].
 * The spec's exact buf-parameter shape would require Array[Int]
 * wire-format plumbing across the FFI boundary that v1 doesn't
 * have yet. The simpler "one read returns whatever's available
 * (up to max_bytes), as a String" surface exercises the same
 * reactor-park-wake-retry mechanism end to end and matches the
 * existing read_to_string shape from AsyncFile. Tracked as a
 * v1-follow-up in the lib.rvn header.
 *
 * Platform support: macOS (Darwin) + Linux. Both kqueue and epoll
 * cases are routed through the per-thread reactor's existing
 * fd-readiness slot table — no platform-specific code in this TU
 * beyond accept4 (Linux) vs accept+fcntl (macOS) and the
 * SO_REUSEADDR / O_NONBLOCK socket-setup boilerplate.
 */

#include "../../core/runtime/runtime.h"

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

/* Wire layout — C-owned. The Riven-side class fields (`fd: Int`,
 * `closed: Int`) are unused on the access path: the Riven side only
 * touches these structs through FFI accessors, and `*_bind` / `*_from_fd`
 * are the only allocators. Growing the struct to add the persistent
 * reactor handles is safe because Riven never directly indexes fields.
 *
 * Layout note: handles are negative-encoded int64 slot indices returned
 * by riven_reactor_register_fd_*_persistent (0 = not registered).
 * They're per-(reactor, fd, mode) and must be deregistered on the SAME
 * thread that registered them — see the drop functions below.
 */
typedef struct {
    int32_t fd;
    int32_t closed;
    int64_t accept_handle;  /* persistent read-readiness registration */
} RivenAsyncTcpListener;

typedef struct {
    int32_t fd;
    int32_t closed;
    int64_t read_handle;    /* persistent read-readiness registration */
    int64_t write_handle;   /* persistent write-readiness registration */
} RivenAsyncTcpStream;

_Static_assert(sizeof(RivenAsyncTcpListener) == 16,
    "RivenAsyncTcpListener wire layout drifted from documented 16-byte form "
    "(grew from 8 in v1-missing-features to add accept_handle)");
_Static_assert(sizeof(RivenAsyncTcpStream) == 24,
    "RivenAsyncTcpStream wire layout drifted from documented 24-byte form "
    "(grew from 8 in v1-missing-features to add read_handle + write_handle)");

/* Reactor FFI — declared here so we can register persistently at
 * stream/listener construction without going through the Riven side.
 * The persistent variants live in library/std/future/runtime/reactor.c. */
extern int64_t riven_reactor_register_fd_read_persistent(int64_t reactor, int64_t fd);
extern int64_t riven_reactor_register_fd_write_persistent(int64_t reactor, int64_t fd);
extern void    riven_reactor_deregister(int64_t reactor, int64_t handle);

/* ── socket helpers ────────────────────────────────────────────────── */

/* Parse "host:port" into a sockaddr_in. Returns 0 on success, non-0
 * on failure. Supports IPv4 literals (e.g. "127.0.0.1:9000") and host
 * names resolved via getaddrinfo. v1 sync DNS — async DNS is a v2
 * follow-up. */
static int riven_async_net_parse_addr(const char *addr,
                                      struct sockaddr_in *out) {
    if (!addr) return -1;
    const char *colon = strrchr(addr, ':');
    if (!colon) return -1;
    size_t host_len = (size_t)(colon - addr);
    if (host_len >= 256) return -1;

    char host_buf[256];
    memcpy(host_buf, addr, host_len);
    host_buf[host_len] = '\0';
    const char *host = host_buf[0] == '\0' ? "0.0.0.0" : host_buf;

    const char *port_str = colon + 1;
    if (!*port_str) return -1;

    struct addrinfo hints;
    memset(&hints, 0, sizeof(hints));
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    hints.ai_flags = AI_PASSIVE;

    struct addrinfo *res = NULL;
    int rc = getaddrinfo(host, port_str, &hints, &res);
    if (rc != 0 || !res) {
        if (res) freeaddrinfo(res);
        return -1;
    }
    memcpy(out, res->ai_addr, sizeof(*out));
    freeaddrinfo(res);
    return 0;
}

static int riven_async_net_set_nonblock(int fd) {
    int flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0) return -1;
    return fcntl(fd, F_SETFL, flags | O_NONBLOCK);
}

/* BSD/macOS doesn't honour MSG_NOSIGNAL on send(); per-socket
 * SO_NOSIGPIPE is the portable way to keep write-after-peer-close from
 * raising SIGPIPE (which would terminate the process by default).
 * Symmetric with `riven_tcp_set_nosigpipe` in the sync net runtime —
 * we MUST call this on every accepted fd here too, otherwise an HTTP
 * client closing mid-response silently kills the server. (Observed:
 * rondo-async exiting at the END of a wrk run with no log output,
 * after thousands of clean requests, when wrk tore down its
 * connection pool faster than the server finished writing the last
 * few responses.) */
static void riven_async_net_set_nosigpipe(int fd) {
#ifdef SO_NOSIGPIPE
    int one = 1;
    (void)setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
#else
    (void)fd;
#endif
}

/* Map errno -> IoError tag for socket operations. Covers the codes
 * connect / accept / read / write can produce post-EAGAIN. */
static int riven_async_net_io_error_tag(int err) {
    switch (err) {
        case ECONNREFUSED: return RIVEN_IO_ERROR_CONNECTION_REFUSED;
        case ECONNRESET:   return RIVEN_IO_ERROR_CONNECTION_RESET;
        case ECONNABORTED: return RIVEN_IO_ERROR_CONNECTION_ABORTED;
        case ENOTCONN:     return RIVEN_IO_ERROR_NOT_CONNECTED;
        case EADDRINUSE:   return RIVEN_IO_ERROR_ADDR_IN_USE;
        case EADDRNOTAVAIL:return RIVEN_IO_ERROR_ADDR_NOT_AVAILABLE;
        case EPIPE:        return RIVEN_IO_ERROR_BROKEN_PIPE;
        case EACCES:
        case EPERM:        return RIVEN_IO_ERROR_PERMISSION_DENIED;
        case EBADF:
        case EINVAL:       return RIVEN_IO_ERROR_INVALID_INPUT;
        case ETIMEDOUT:    return RIVEN_IO_ERROR_TIMED_OUT;
        default:           return RIVEN_IO_ERROR_OTHER;
    }
}

/* ── AsyncTcpListener.bind (B6.5) ──────────────────────────────────── */
/* Eager bind+listen. No state machine — the syscalls don't block on a
 * fresh socket. The future is single-shot Ready on first poll for
 * surface symmetry with AsyncTcpStream.connect. */

void *riven_async_tcp_listener_bind(const char *addr) {
    if (!addr) {
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(RIVEN_IO_ERROR_INVALID_INPUT));
    }
    struct sockaddr_in sa;
    if (riven_async_net_parse_addr(addr, &sa) != 0) {
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(RIVEN_IO_ERROR_INVALID_INPUT));
    }

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(
                riven_async_net_io_error_tag(errno)));
    }
    riven_async_net_set_nosigpipe(fd);
    int one = 1;
    /* SO_REUSEADDR so the e2e fixture can pick a port and not get
     * TIME_WAIT'd across re-runs. */
    (void)setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
    /* SO_REUSEPORT so multiple async workers can bind the same
     * port and let the kernel load-balance accept calls across
     * them (Rondo's multi-core serving model — docs/rondo_v1_blockers.md
     * §B5). Best-effort: SO_REUSEPORT is Linux 3.9+ / macOS / BSD
     * but absent on some older targets; ignoring failure keeps
     * the single-listener path working unchanged. */
    (void)setsockopt(fd, SOL_SOCKET, SO_REUSEPORT, &one, sizeof(one));

    if (riven_async_net_set_nonblock(fd) != 0) {
        int saved = errno;
        close(fd);
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(
                riven_async_net_io_error_tag(saved)));
    }

    if (bind(fd, (struct sockaddr *)&sa, sizeof(sa)) != 0) {
        int saved = errno;
        close(fd);
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(
                riven_async_net_io_error_tag(saved)));
    }

    if (listen(fd, 128) != 0) {
        int saved = errno;
        close(fd);
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(
                riven_async_net_io_error_tag(saved)));
    }

    RivenAsyncTcpListener *l =
        (RivenAsyncTcpListener *)riven_alloc(sizeof(RivenAsyncTcpListener));
    l->fd = fd;
    l->closed = 0;
    /* Register the listener fd for read-readiness ONCE, edge-triggered,
     * on the constructing thread's reactor. AsyncAcceptFuture then just
     * calls accept_step (which loops until EAGAIN) — no per-poll
     * register/deregister syscalls. 0 = "use current-thread reactor",
     * lazy-acquires if needed. */
    l->accept_handle = riven_reactor_register_fd_read_persistent(0, (int64_t)fd);
    return riven_result_ok_value((int64_t)l);
}

int64_t riven_async_tcp_listener_fd(void *self) {
    if (!self) return -1;
    RivenAsyncTcpListener *l = (RivenAsyncTcpListener *)self;
    if (l->closed) return -1;
    return (int64_t)l->fd;
}

/* Persistent accept-readiness handle. AsyncAcceptFuture reads this
 * from the listener at construction time so it doesn't have to register
 * per-poll. Returns 0 if not registered (defensive — bind always
 * registers, so this only happens for a corrupted listener). */
int64_t riven_async_tcp_listener_accept_handle(void *self) {
    if (!self) return 0;
    return ((RivenAsyncTcpListener *)self)->accept_handle;
}

void riven_async_tcp_listener_drop(void *self) {
    if (!self) return;
    RivenAsyncTcpListener *l = (RivenAsyncTcpListener *)self;
    /* Deregister BEFORE close — kqueue EV_DELETE / epoll EPOLL_CTL_DEL
     * need the fd to still be valid; closing an fd implicitly removes
     * it from kqueue but NOT epoll, and even on kqueue the slot
     * bookkeeping in reactor.c needs the fd to look up the filter. */
    if (l->accept_handle != 0) {
        riven_reactor_deregister(0, l->accept_handle);
        l->accept_handle = 0;
    }
    if (!l->closed && l->fd >= 0) {
        int rc;
        do { rc = close(l->fd); } while (rc < 0 && errno == EINTR);
        (void)rc;
        l->closed = 1;
        l->fd = -1;
    }
}

/* ── AsyncTcpListener.accept state machine (B6.6) ──────────────────── */

typedef struct {
    int listener_fd;     /* borrowed from the listener */
    int accepted_fd;     /* set once accept returns a stream fd */
    char peer_buf[64];   /* "<ip>:<port>" peer address */
    int err_tag;
    int err_set;
    int done;
    int result_taken;
} RivenAsyncAcceptState;

void *riven_async_accept_state_new(int64_t listener_fd) {
    RivenAsyncAcceptState *s =
        (RivenAsyncAcceptState *)riven_alloc(sizeof(RivenAsyncAcceptState));
    s->listener_fd = (int)listener_fd;
    s->accepted_fd = -1;
    s->peer_buf[0] = '\0';
    s->err_tag = 0;
    s->err_set = 0;
    s->done = 0;
    s->result_taken = 0;
    return s;
}

int64_t riven_async_accept_state_get_fd(void *state) {
    if (!state) return -1;
    return (int64_t)((RivenAsyncAcceptState *)state)->listener_fd;
}

int64_t riven_async_accept_step(void *state) {
    if (!state) return 3;
    RivenAsyncAcceptState *s = (RivenAsyncAcceptState *)state;
    if (s->done) return 2;
    if (s->err_set) return 3;

    for (;;) {
        struct sockaddr_in peer;
        socklen_t plen = sizeof(peer);
        int new_fd = accept(s->listener_fd, (struct sockaddr *)&peer, &plen);
        if (new_fd < 0) {
            if (errno == EINTR) continue;
            /* BSD/macOS gotcha (Stevens UNP §15.6): after kqueue signals
             * the listener readable, the head-of-queue connection can
             * abort (peer RST between SYN/ACK and accept). Blocking
             * accept hides this — the kernel silently dequeues and
             * keeps blocking. Non-blocking accept surfaces it as
             * ECONNABORTED. Production network code (Go's net pkg,
             * tokio, libevent) treats it as a soft retry, not a fatal
             * error. Loop back so the user-visible accept future
             * keeps draining the listen queue without dying on the
             * first half-open peer. */
            if (errno == ECONNABORTED) continue;
            if (errno == EAGAIN
#if defined(EWOULDBLOCK) && (EWOULDBLOCK != EAGAIN)
                || errno == EWOULDBLOCK
#endif
            ) {
                return 1;
            }
            /* EMFILE / ENFILE: process or system file-descriptor table
             * full. Standard production-server response (Go's net pkg,
             * libevent, nginx) is to back off briefly and retry — the
             * condition is almost always transient (in-flight closes
             * about to drain). Returning 3 (fatal Err) here would kill
             * the whole accept loop on the first fd-exhaustion spike.
             * 10 ms sleep is enough for close() backlog to clear; the
             * loop continues and the listener stays accepting. */
            if (errno == EMFILE || errno == ENFILE) {
                struct timespec ts = { 0, 10 * 1000 * 1000 };
                nanosleep(&ts, NULL);
                continue;
            }
            s->err_set = 1;
            s->err_tag = riven_async_net_io_error_tag(errno);
            return 3;
        }
        riven_async_net_set_nosigpipe(new_fd);
        if (riven_async_net_set_nonblock(new_fd) != 0) {
            int saved = errno;
            close(new_fd);
            s->err_set = 1;
            s->err_tag = riven_async_net_io_error_tag(saved);
            return 3;
        }
        char ip[INET_ADDRSTRLEN];
        if (!inet_ntop(AF_INET, &peer.sin_addr, ip, sizeof(ip))) {
            ip[0] = '?'; ip[1] = '\0';
        }
        snprintf(s->peer_buf, sizeof(s->peer_buf), "%s:%u",
                 ip, (unsigned)ntohs(peer.sin_port));
        s->accepted_fd = new_fd;
        s->done = 1;
        return 2;
    }
}

/* Result payload is a tuple (AsyncTcpStream, String). The C side
 * doesn't know how to construct tuples generically; the Riven future
 * pulls fd + peer separately and assembles the tuple in surface code.
 * These three takers return the components individually. */
int64_t riven_async_accept_state_take_fd(void *state) {
    if (!state) return -1;
    RivenAsyncAcceptState *s = (RivenAsyncAcceptState *)state;
    if (s->result_taken || s->err_set) return -1;
    return (int64_t)s->accepted_fd;
}

void *riven_async_accept_state_take_peer(void *state) {
    if (!state) return riven_string_from("");
    RivenAsyncAcceptState *s = (RivenAsyncAcceptState *)state;
    if (s->result_taken || s->err_set) return riven_string_from("");
    return riven_string_from(s->peer_buf);
}

int64_t riven_async_accept_state_get_err(void *state) {
    if (!state) return -1;
    RivenAsyncAcceptState *s = (RivenAsyncAcceptState *)state;
    if (!s->err_set) return -1;
    return (int64_t)s->err_tag;
}

void riven_async_accept_state_mark_taken(void *state) {
    if (!state) return;
    ((RivenAsyncAcceptState *)state)->result_taken = 1;
}

void riven_async_accept_state_free(void *state) {
    if (!state) return;
    RivenAsyncAcceptState *s = (RivenAsyncAcceptState *)state;
    /* If the user dropped the future without consuming the accepted
     * fd, close it here so we don't leak. */
    if (s->accepted_fd >= 0 && !s->result_taken) {
        close(s->accepted_fd);
        s->accepted_fd = -1;
    }
    free(s);
}

/* Wrap an accepted (or connected) fd in a freshly-allocated
 * AsyncTcpStream. Called by AsyncAcceptFuture once `accept_step`
 * returns 2 (done), and by `connect_state_to_result` after
 * AsyncConnectFuture resolves.
 *
 * Registers the fd ONCE with the current-thread reactor for BOTH
 * read- and write-readiness, edge-triggered. The handles live on the
 * stream and are deregistered on drop. AsyncReadFuture / AsyncWriteFuture
 * read these handles via the accessors below; they no longer touch the
 * reactor themselves. */
void *riven_async_tcp_stream_from_fd(int64_t fd) {
    RivenAsyncTcpStream *s =
        (RivenAsyncTcpStream *)riven_alloc(sizeof(RivenAsyncTcpStream));
    s->fd = (int)fd;
    s->closed = 0;
    /* 0 = use current-thread reactor (lazy-acquired). Stream construction
     * happens on the same worker thread that called block_on(accept) or
     * block_on(connect), and that thread already has a reactor since
     * block_on installed one. Read/write futures will be polled on the
     * SAME thread, so the per-thread reactor identity is consistent.
     *
     * Registering both r+w upfront wastes one kevent on streams that
     * are read-only (or write-only), but the alternative — lazy
     * registration on first EAGAIN — reintroduces the per-poll syscall
     * we're trying to eliminate. Two kevents at construction (one per
     * filter) is dwarfed by the savings on the hot path. */
    s->read_handle  = riven_reactor_register_fd_read_persistent(0, fd);
    s->write_handle = riven_reactor_register_fd_write_persistent(0, fd);
    return s;
}

/* ── AsyncTcpStream.connect state machine (B7) ─────────────────────── */

typedef struct {
    int fd;
    int started;       /* 1 once socket() + connect() has been called */
    int completed;     /* 1 once getsockopt(SO_ERROR) returned success */
    char addr[256];    /* copy of the parsed address — owned by the state */
    struct sockaddr_in sa;
    int err_tag;
    int err_set;
    int done;
    int result_taken;
} RivenAsyncConnectState;

void *riven_async_connect_state_new(const char *addr) {
    RivenAsyncConnectState *s =
        (RivenAsyncConnectState *)riven_alloc(sizeof(RivenAsyncConnectState));
    s->fd = -1;
    s->started = 0;
    s->completed = 0;
    s->err_tag = 0;
    s->err_set = 0;
    s->done = 0;
    s->result_taken = 0;
    memset(&s->sa, 0, sizeof(s->sa));
    if (addr) {
        size_t n = strnlen(addr, sizeof(s->addr) - 1);
        memcpy(s->addr, addr, n);
        s->addr[n] = '\0';
    } else {
        s->addr[0] = '\0';
    }
    return s;
}

int64_t riven_async_connect_state_get_fd(void *state) {
    if (!state) return -1;
    return (int64_t)((RivenAsyncConnectState *)state)->fd;
}

int64_t riven_async_connect_step(void *state) {
    if (!state) return 3;
    RivenAsyncConnectState *s = (RivenAsyncConnectState *)state;
    if (s->done) return 2;
    if (s->err_set) return 3;

    if (!s->started) {
        if (riven_async_net_parse_addr(s->addr, &s->sa) != 0) {
            s->err_set = 1;
            s->err_tag = RIVEN_IO_ERROR_INVALID_INPUT;
            return 3;
        }
        int fd = socket(AF_INET, SOCK_STREAM, 0);
        if (fd < 0) {
            s->err_set = 1;
            s->err_tag = riven_async_net_io_error_tag(errno);
            return 3;
        }
        riven_async_net_set_nosigpipe(fd);
        if (riven_async_net_set_nonblock(fd) != 0) {
            int saved = errno;
            close(fd);
            s->err_set = 1;
            s->err_tag = riven_async_net_io_error_tag(saved);
            return 3;
        }
        s->fd = fd;
        s->started = 1;

        int rc = connect(fd, (struct sockaddr *)&s->sa, sizeof(s->sa));
        if (rc == 0) {
            s->completed = 1;
            s->done = 1;
            return 2;
        }
        if (errno == EINPROGRESS) {
            return 1;
        }
        s->err_set = 1;
        s->err_tag = riven_async_net_io_error_tag(errno);
        return 3;
    }

    /* Reactor woke us — check SO_ERROR to see if connect completed. */
    int soerr = 0;
    socklen_t slen = sizeof(soerr);
    if (getsockopt(s->fd, SOL_SOCKET, SO_ERROR, &soerr, &slen) != 0) {
        s->err_set = 1;
        s->err_tag = riven_async_net_io_error_tag(errno);
        return 3;
    }
    if (soerr == 0) {
        s->completed = 1;
        s->done = 1;
        return 2;
    }
    if (soerr == EINPROGRESS) {
        return 1;
    }
    s->err_set = 1;
    s->err_tag = riven_async_net_io_error_tag(soerr);
    return 3;
}

int64_t riven_async_connect_state_take_fd(void *state) {
    if (!state) return -1;
    RivenAsyncConnectState *s = (RivenAsyncConnectState *)state;
    if (s->result_taken || s->err_set || !s->completed) return -1;
    return (int64_t)s->fd;
}

int64_t riven_async_connect_state_get_err(void *state) {
    if (!state) return -1;
    RivenAsyncConnectState *s = (RivenAsyncConnectState *)state;
    if (!s->err_set) return -1;
    return (int64_t)s->err_tag;
}

void riven_async_connect_state_mark_taken(void *state) {
    if (!state) return;
    ((RivenAsyncConnectState *)state)->result_taken = 1;
}

void riven_async_connect_state_free(void *state) {
    if (!state) return;
    RivenAsyncConnectState *s = (RivenAsyncConnectState *)state;
    /* If the user dropped the connect future without consuming the
     * fd (failed connect, or half-resolved connect that was
     * cancelled), close it here. The reactor deregister happens on
     * the Riven side via the future's def drop. */
    if (s->fd >= 0 && !s->result_taken) {
        close(s->fd);
        s->fd = -1;
    }
    free(s);
}

/* ── AsyncTcpStream.read state machine (B8) ────────────────────────── */
/* Surface deviation from spec: returns Result[String, IoError] of
 * whatever was read on a single ready cycle, capped at `max_bytes`.
 * Not buf-fill semantics — "give me the next chunk".
 *
 * Step states:
 *   1 EAGAIN — register fd for read-readiness
 *   2 done   — out_str populated (possibly empty for clean EOF)
 *   3 error  — err_tag set */

typedef struct {
    int fd;            /* borrowed from the stream */
    size_t max_bytes;
    char *buf;         /* scratch buffer sized to max_bytes */
    size_t len;
    char *out_str;     /* canonical-pool String pointer once done */
    int err_tag;
    int err_set;
    int done;
    int result_taken;
} RivenAsyncTcpReadState;

void *riven_async_tcp_read_state_new(int64_t fd, int64_t max_bytes) {
    RivenAsyncTcpReadState *s =
        (RivenAsyncTcpReadState *)riven_alloc(sizeof(RivenAsyncTcpReadState));
    s->fd = (int)fd;
    /* Cap at 64 KiB per call — keeps the scratch allocation bounded
     * even if a buggy caller passes a huge max. The Riven surface can
     * always loop. */
    if (max_bytes <= 0) max_bytes = 1;
    if (max_bytes > 65536) max_bytes = 65536;
    s->max_bytes = (size_t)max_bytes;
    s->buf = (char *)malloc(s->max_bytes + 1);
    if (!s->buf) riven_panic("riven_async_tcp_read_state_new: malloc failed");
    s->len = 0;
    s->out_str = NULL;
    s->err_tag = 0;
    s->err_set = 0;
    s->done = 0;
    s->result_taken = 0;
    return s;
}

int64_t riven_async_tcp_read_state_get_fd(void *state) {
    if (!state) return -1;
    return (int64_t)((RivenAsyncTcpReadState *)state)->fd;
}

int64_t riven_async_tcp_read_step(void *state) {
    if (!state) return 3;
    RivenAsyncTcpReadState *s = (RivenAsyncTcpReadState *)state;
    if (s->done) return 2;
    if (s->err_set) return 3;

    for (;;) {
        ssize_t got = read(s->fd, s->buf, s->max_bytes);
        if (got < 0) {
            if (errno == EINTR) continue;
            if (errno == EAGAIN
#if defined(EWOULDBLOCK) && (EWOULDBLOCK != EAGAIN)
                || errno == EWOULDBLOCK
#endif
            ) {
                return 1;
            }
            s->err_set = 1;
            s->err_tag = riven_async_net_io_error_tag(errno);
            return 3;
        }
        /* got == 0 → clean EOF; got > 0 → at least one byte read.
         * Either way, finalize the String with the bytes we have
         * (zero-length on EOF). The caller distinguishes EOF from
         * partial-read by inspecting the returned String length. */
        s->len = (size_t)got;
        s->buf[s->len] = '\0';
        s->out_str = riven_string_from(s->buf);
        s->done = 1;
        return 2;
    }
}

void *riven_async_tcp_read_state_take_result(void *state) {
    if (!state) {
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(RIVEN_IO_ERROR_INVALID_INPUT));
    }
    RivenAsyncTcpReadState *s = (RivenAsyncTcpReadState *)state;
    if (s->result_taken) {
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(RIVEN_IO_ERROR_INVALID_INPUT));
    }
    s->result_taken = 1;
    if (s->buf) {
        free(s->buf);
        s->buf = NULL;
    }
    if (s->err_set) {
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(s->err_tag));
    }
    return riven_result_ok_value((int64_t)s->out_str);
}

void riven_async_tcp_read_state_free(void *state) {
    if (!state) return;
    RivenAsyncTcpReadState *s = (RivenAsyncTcpReadState *)state;
    if (s->buf) {
        free(s->buf);
        s->buf = NULL;
    }
    free(s);
}

/* ── AsyncTcpStream.write state machine (B9) ───────────────────────── */
/* Writes the entire `content` (matches spec phrasing "writes up to
 * content.len bytes" but in practice loops until EOF on the writer or
 * full content is flushed — same shape as AsyncWriteAllFuture from 4B). */

typedef struct {
    int fd;
    const char *content;
    size_t total;
    size_t written;
    int err_tag;
    int err_set;
    int done;
    int result_taken;
} RivenAsyncTcpWriteState;

void *riven_async_tcp_write_state_new(int64_t fd, const char *content) {
    RivenAsyncTcpWriteState *s =
        (RivenAsyncTcpWriteState *)riven_alloc(sizeof(RivenAsyncTcpWriteState));
    s->fd = (int)fd;
    s->content = content ? content : "";
    s->total = strlen(s->content);
    s->written = 0;
    s->err_tag = 0;
    s->err_set = 0;
    s->done = s->total == 0 ? 1 : 0;
    s->result_taken = 0;
    return s;
}

int64_t riven_async_tcp_write_state_get_fd(void *state) {
    if (!state) return -1;
    return (int64_t)((RivenAsyncTcpWriteState *)state)->fd;
}

int64_t riven_async_tcp_write_step(void *state) {
    if (!state) return 3;
    RivenAsyncTcpWriteState *s = (RivenAsyncTcpWriteState *)state;
    if (s->done) return 2;
    if (s->err_set) return 3;

    while (s->written < s->total) {
        ssize_t put = write(s->fd, s->content + s->written,
                            s->total - s->written);
        if (put < 0) {
            if (errno == EINTR) continue;
            if (errno == EAGAIN
#if defined(EWOULDBLOCK) && (EWOULDBLOCK != EAGAIN)
                || errno == EWOULDBLOCK
#endif
            ) {
                return 1;
            }
            s->err_set = 1;
            s->err_tag = riven_async_net_io_error_tag(errno);
            return 3;
        }
        if (put == 0) {
            s->err_set = 1;
            s->err_tag = RIVEN_IO_ERROR_WRITE_ZERO;
            return 3;
        }
        s->written += (size_t)put;
    }
    s->done = 1;
    return 2;
}

void *riven_async_tcp_write_state_take_result(void *state) {
    if (!state) {
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(RIVEN_IO_ERROR_INVALID_INPUT));
    }
    RivenAsyncTcpWriteState *s = (RivenAsyncTcpWriteState *)state;
    if (s->result_taken) {
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(RIVEN_IO_ERROR_INVALID_INPUT));
    }
    s->result_taken = 1;
    if (s->err_set) {
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(s->err_tag));
    }
    /* Return the count written as the Ok payload (matches spec B9). */
    return riven_result_ok_value((int64_t)s->written);
}

void riven_async_tcp_write_state_free(void *state) {
    if (!state) return;
    free(state);
}

/* ── AsyncTcpStream — fd accessor + drop (B10's stream side) ───────── */

int64_t riven_async_tcp_stream_fd(void *self) {
    if (!self) return -1;
    RivenAsyncTcpStream *s = (RivenAsyncTcpStream *)self;
    if (s->closed) return -1;
    return (int64_t)s->fd;
}

/* Persistent r/w handles. AsyncReadFuture / AsyncWriteFuture grab these
 * at construction time so they don't have to register per-poll. */
int64_t riven_async_tcp_stream_read_handle(void *self) {
    if (!self) return 0;
    return ((RivenAsyncTcpStream *)self)->read_handle;
}

int64_t riven_async_tcp_stream_write_handle(void *self) {
    if (!self) return 0;
    return ((RivenAsyncTcpStream *)self)->write_handle;
}

/* Internal helper: tear down both r+w persistent registrations. Idempotent.
 * Called from both drop and shutdown — close() also implicitly
 * removes the fd from kqueue, but our slot table still owns bookkeeping
 * for `fired` / `registered_count`, and Linux epoll does NOT auto-remove
 * on close (only on the last fd dup closes). Always deregister
 * explicitly. */
static void riven_async_tcp_stream_unregister(RivenAsyncTcpStream *s) {
    if (s->read_handle != 0) {
        riven_reactor_deregister(0, s->read_handle);
        s->read_handle = 0;
    }
    if (s->write_handle != 0) {
        riven_reactor_deregister(0, s->write_handle);
        s->write_handle = 0;
    }
}

void riven_async_tcp_stream_drop(void *self) {
    if (!self) return;
    RivenAsyncTcpStream *s = (RivenAsyncTcpStream *)self;
    /* Deregister BEFORE close — see listener_drop note. */
    riven_async_tcp_stream_unregister(s);
    if (!s->closed && s->fd >= 0) {
        int rc;
        do { rc = close(s->fd); } while (rc < 0 && errno == EINTR);
        (void)rc;
        s->closed = 1;
        s->fd = -1;
    }
}

/* AsyncCloseFuture (B10) performs `shutdown(SHUT_RDWR)` + `close` and
 * resolves Ready(()) immediately. The future owns the stream by-move
 * (per spec: `def close(self) -> AsyncCloseFuture`), so once close
 * runs the fd is gone — there is nothing left for the stream's own
 * drop hook to do.
 *
 * Why we close here instead of relying on stream-drop:
 *   At sub-phase 4C ship time, drop elaboration on FFI-bound `def drop`
 *   for fields of a future class is not reliably firing at scope exit
 *   on every code path that consumes an AsyncTcpStream by-move (the
 *   close future's own drop, in particular, observed empirically as
 *   a TIME_WAIT leak under server load — every accepted connection
 *   left its fd open in the process until the program exited).
 *   Closing here makes the public `close` surface authoritative: by
 *   the time `block_on(stream.close())` returns Ready(()), the fd
 *   IS closed. The stream's own drop becomes a no-op fallback for
 *   the "stream dropped without explicit close" path, which is the
 *   only path that still depends on drop elaboration. */
void riven_async_tcp_stream_shutdown(void *self) {
    if (!self) return;
    RivenAsyncTcpStream *s = (RivenAsyncTcpStream *)self;
    /* Deregister r+w handles BEFORE close so the reactor's slot table
     * and registered_count stay consistent (see drop note). The Riven
     * stream's own def-drop will run after this and find handles == 0
     * + closed == 1, making it a no-op. */
    riven_async_tcp_stream_unregister(s);
    if (!s->closed && s->fd >= 0) {
        (void)shutdown(s->fd, SHUT_RDWR);
        int rc;
        do { rc = close(s->fd); } while (rc < 0 && errno == EINTR);
        (void)rc;
        s->closed = 1;
        s->fd = -1;
    }
}
