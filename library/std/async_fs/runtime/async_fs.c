/*
 * std::async_fs runtime — backs the AsyncFile / AsyncOpenFuture /
 * AsyncReadToStringFuture / AsyncWriteAllFuture surface declared in
 * library/std/async_fs/src/lib.rx.
 *
 * Spec: docs/specs/stdlib/async_io.spec.md Milestone 4B (B4–B6).
 *
 * Design notes:
 *   - AsyncFile is a thin wrapper around a non-blocking POSIX fd —
 *     same wire shape as RuxenFile (8 bytes: int32 fd + int32 closed)
 *     so future BufReader / shared-fd tooling can cast across without
 *     a translation layer.
 *   - The three futures are hand-written (not async-def-lowered). They
 *     hold an opaque "state" pointer the Ruxen side stashes as an Int
 *     field; the state struct lives entirely in C so the Ruxen future
 *     class stays small (5 Int fields, no heap of its own beyond the
 *     state pointer).
 *   - Reactor integration: when a `read(2)` / `write(2)` returns
 *     EAGAIN, the future asks the reactor to register the fd for
 *     read/write readiness (negative-encoded slot handle, see
 *     library/std/future/runtime/reactor.c). On wake, the executor
 *     re-polls; the future drains again until EOF / EAGAIN / done.
 *
 * Symbols exported here:
 *   ruxen_async_file_open(path, flags) -> Result[AsyncFile, IoError]
 *       Eager non-blocking open(2) — same semantics as
 *       ruxen_file_open but adds O_NONBLOCK. Used by both the read-
 *       and write-side AsyncOpenFuture constructors via their init.
 *   ruxen_async_file_drop(self) -> ()
 *       Drop hook — closes the fd if not already closed. Registered
 *       in the user_drop_classes set by mir/lower/collect (because
 *       the .rx class body declares `def drop`).
 *
 *   ruxen_async_read_state_new(fd) -> state*
 *   ruxen_async_read_step(state) -> int (0 progress, 1 EAGAIN, 2 EOF, 3 error)
 *   ruxen_async_read_state_take_result(state) -> Result[String, IoError]
 *   ruxen_async_read_state_get_fd(state) -> int
 *   ruxen_async_read_state_free(state) -> ()
 *
 *   ruxen_async_write_state_new(fd, content) -> state*
 *   ruxen_async_write_step(state) -> int (0 progress, 1 EAGAIN, 2 done, 3 error)
 *   ruxen_async_write_state_take_result(state) -> Result[(), IoError]
 *   ruxen_async_write_state_get_fd(state) -> int
 *   ruxen_async_write_state_free(state) -> ()
 */

#include "../../core/runtime/runtime.h"

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

/* AsyncFile wire layout — kept binary-compatible with RuxenFile so
 * future cross-package work (BufReader_for_AsyncFile, sendfile bridges)
 * can cast without an adapter. */
typedef struct {
    int32_t fd;
    int32_t closed;
} RuxenAsyncFile;

_Static_assert(sizeof(RuxenAsyncFile) == 8,
    "RuxenAsyncFile wire layout drifted from documented 8-byte form");

/* ── open(2) — eager, non-blocking ─────────────────────────────────── */

/* flags shape mirrors ruxen_file_open / _create:
 *   0 = read-only            (O_RDONLY)
 *   1 = write-create-truncate (O_WRONLY | O_CREAT | O_TRUNC, 0644)
 * O_NONBLOCK is OR'd on always — even though open(2) on regular files
 * is non-blocking either way, the flag is what lets subsequent read /
 * write surface EAGAIN instead of blocking when the file is a fifo /
 * socket-like fd. Pipes and special files transparently get the right
 * non-blocking behaviour without further code. */
void *ruxen_async_file_open(const char *path, int64_t flags) {
    if (!path) {
        return ruxen_result_err_value(
            (int64_t)ruxen_io_error_unit(RUXEN_IO_ERROR_INVALID_INPUT));
    }
    int open_flags;
    int has_mode = 0;
    if (flags == 1) {
        open_flags = O_WRONLY | O_CREAT | O_TRUNC | O_NONBLOCK;
        has_mode = 1;
    } else if (flags == 2) {
        /* read + write, no create. Provided for future symmetry —
         * the 4B fixtures only need flags 0 / 1. */
        open_flags = O_RDWR | O_NONBLOCK;
    } else {
        open_flags = O_RDONLY | O_NONBLOCK;
    }

    int fd;
    do {
        fd = has_mode ? open(path, open_flags, 0644)
                      : open(path, open_flags);
    } while (fd < 0 && errno == EINTR);
    if (fd < 0) {
        return ruxen_io_error_from_errno(errno);
    }

    RuxenAsyncFile *f =
        (RuxenAsyncFile *)ruxen_alloc(sizeof(RuxenAsyncFile));
    f->fd = fd;
    f->closed = 0;
    return ruxen_result_ok_value((int64_t)f);
}

/* Drop hook — closes the fd if still open. Mirrors ruxen_file_drop.
 * Errors are swallowed (nobody to surface them to at scope exit). */
void ruxen_async_file_drop(void *self) {
    if (!self) return;
    RuxenAsyncFile *f = (RuxenAsyncFile *)self;
    if (!f->closed && f->fd >= 0) {
        int rc;
        do {
            rc = close(f->fd);
        } while (rc < 0 && errno == EINTR);
        (void)rc;
        f->closed = 1;
        f->fd = -1;
    }
}

/* Ruxen-callable "get fd" — used by the read/write future constructors
 * to extract the fd from an AsyncFile passed in by the user. Returns
 * the raw fd, or -1 if closed. */
int64_t ruxen_async_file_fd(void *self) {
    if (!self) return -1;
    RuxenAsyncFile *f = (RuxenAsyncFile *)self;
    if (f->closed) return -1;
    return (int64_t)f->fd;
}

/* ── Read-to-string state machine ──────────────────────────────────── */

typedef struct {
    int fd;
    char *buf;
    size_t len;
    size_t cap;
    int err_tag;         /* set when err_set != 0 */
    int err_set;         /* 1 once we've recorded an error */
    int done;            /* 1 once EOF reached */
    int result_taken;    /* 1 once the caller has consumed the result */
    char *out_str;       /* canonical-pool String pointer once done */
} RuxenAsyncReadState;

void *ruxen_async_read_state_new(int64_t fd) {
    RuxenAsyncReadState *s =
        (RuxenAsyncReadState *)ruxen_alloc(sizeof(RuxenAsyncReadState));
    s->fd = (int)fd;
    s->cap = 256;
    s->buf = (char *)malloc(s->cap);
    if (!s->buf) ruxen_panic("ruxen_async_read_state_new: malloc failed");
    s->len = 0;
    s->err_tag = 0;
    s->err_set = 0;
    s->done = 0;
    s->result_taken = 0;
    s->out_str = NULL;
    return s;
}

int64_t ruxen_async_read_state_get_fd(void *state) {
    if (!state) return -1;
    return (int64_t)((RuxenAsyncReadState *)state)->fd;
}

/* Drain the fd as far as it'll go in one step. Returns:
 *   0 — made progress (or zero-progress but no EAGAIN yet); caller may
 *       loop without parking. (We collapse "made progress" into "look
 *       again" — the inner loop here continues until EAGAIN / EOF /
 *       error, so the Ruxen side typically sees only 1 / 2 / 3.)
 *   1 — would block (EAGAIN); caller must park on read-readiness.
 *   2 — EOF reached; result_string is populated.
 *   3 — fatal error; err_tag set.
 *
 * The Ruxen side calls this until it returns non-0. Looping fully here
 * (rather than returning 0 per chunk) keeps the Ruxen control flow
 * trivial — no inner re-loop, just "step until non-0, then dispatch". */
int64_t ruxen_async_read_step(void *state) {
    if (!state) return 3;
    RuxenAsyncReadState *s = (RuxenAsyncReadState *)state;
    if (s->done) return 2;
    if (s->err_set) return 3;

    for (;;) {
        if (s->len + 4096 + 1 > s->cap) {
            size_t next_cap = s->cap * 2;
            while (next_cap < s->len + 4096 + 1) next_cap *= 2;
            char *next = (char *)realloc(s->buf, next_cap);
            if (!next) {
                s->err_set = 1;
                s->err_tag = RUXEN_IO_ERROR_OUT_OF_MEMORY;
                return 3;
            }
            s->buf = next;
            s->cap = next_cap;
        }
        ssize_t got = read(s->fd, s->buf + s->len, s->cap - 1 - s->len);
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
            switch (errno) {
                case ENOENT: s->err_tag = RUXEN_IO_ERROR_NOT_FOUND; break;
                case EACCES:
                case EPERM:  s->err_tag = RUXEN_IO_ERROR_PERMISSION_DENIED; break;
                case EBADF:  s->err_tag = RUXEN_IO_ERROR_INVALID_INPUT; break;
                case EPIPE:  s->err_tag = RUXEN_IO_ERROR_BROKEN_PIPE; break;
                default:     s->err_tag = RUXEN_IO_ERROR_OTHER; break;
            }
            return 3;
        }
        if (got == 0) {
            /* EOF — finalize the String. */
            s->buf[s->len] = '\0';
            s->out_str = ruxen_string_from(s->buf);
            s->done = 1;
            return 2;
        }
        s->len += (size_t)got;
    }
}

/* Pull out the Result[String, IoError] for return. Called by the Ruxen
 * poll method once `step` returns 2 (EOF) or 3 (error). After this,
 * the state's owned heap (`buf`) is freed and out_str ownership has
 * passed to the result. The state struct itself is freed separately by
 * `ruxen_async_read_state_free`. */
void *ruxen_async_read_state_take_result(void *state) {
    if (!state) {
        return ruxen_result_err_value(
            (int64_t)ruxen_io_error_unit(RUXEN_IO_ERROR_INVALID_INPUT));
    }
    RuxenAsyncReadState *s = (RuxenAsyncReadState *)state;
    if (s->result_taken) {
        /* Defensive — double-take returns a fresh error. */
        return ruxen_result_err_value(
            (int64_t)ruxen_io_error_unit(RUXEN_IO_ERROR_INVALID_INPUT));
    }
    s->result_taken = 1;
    if (s->buf) {
        free(s->buf);
        s->buf = NULL;
    }
    if (s->err_set) {
        return ruxen_result_err_value(
            (int64_t)ruxen_io_error_unit(s->err_tag));
    }
    /* out_str points into the canonical String pool; ownership is in
     * the pool itself, so we hand back the pointer without freeing. */
    return ruxen_result_ok_value((int64_t)s->out_str);
}

void ruxen_async_read_state_free(void *state) {
    if (!state) return;
    RuxenAsyncReadState *s = (RuxenAsyncReadState *)state;
    if (s->buf) {
        free(s->buf);
        s->buf = NULL;
    }
    free(s);
}

/* ── Write-all state machine ───────────────────────────────────────── */

typedef struct {
    int fd;
    const char *content;   /* points at the canonical-pool String */
    size_t total;
    size_t written;
    int err_tag;
    int err_set;
    int done;
    int result_taken;
} RuxenAsyncWriteState;

void *ruxen_async_write_state_new(int64_t fd, const char *content) {
    RuxenAsyncWriteState *s =
        (RuxenAsyncWriteState *)ruxen_alloc(sizeof(RuxenAsyncWriteState));
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

int64_t ruxen_async_write_state_get_fd(void *state) {
    if (!state) return -1;
    return (int64_t)((RuxenAsyncWriteState *)state)->fd;
}

/* Same return codes as read_step. */
int64_t ruxen_async_write_step(void *state) {
    if (!state) return 3;
    RuxenAsyncWriteState *s = (RuxenAsyncWriteState *)state;
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
            switch (errno) {
                case EBADF:  s->err_tag = RUXEN_IO_ERROR_INVALID_INPUT; break;
                case EPIPE:  s->err_tag = RUXEN_IO_ERROR_BROKEN_PIPE; break;
                case EACCES:
                case EPERM:  s->err_tag = RUXEN_IO_ERROR_PERMISSION_DENIED; break;
                default:     s->err_tag = RUXEN_IO_ERROR_OTHER; break;
            }
            return 3;
        }
        if (put == 0) {
            s->err_set = 1;
            s->err_tag = RUXEN_IO_ERROR_WRITE_ZERO;
            return 3;
        }
        s->written += (size_t)put;
    }
    s->done = 1;
    return 2;
}

void *ruxen_async_write_state_take_result(void *state) {
    if (!state) {
        return ruxen_result_err_value(
            (int64_t)ruxen_io_error_unit(RUXEN_IO_ERROR_INVALID_INPUT));
    }
    RuxenAsyncWriteState *s = (RuxenAsyncWriteState *)state;
    if (s->result_taken) {
        return ruxen_result_err_value(
            (int64_t)ruxen_io_error_unit(RUXEN_IO_ERROR_INVALID_INPUT));
    }
    s->result_taken = 1;
    if (s->err_set) {
        return ruxen_result_err_value(
            (int64_t)ruxen_io_error_unit(s->err_tag));
    }
    /* Ok(()) — unit payload is just 0. */
    return ruxen_result_ok_value(0);
}

void ruxen_async_write_state_free(void *state) {
    if (!state) return;
    free(state);
}
