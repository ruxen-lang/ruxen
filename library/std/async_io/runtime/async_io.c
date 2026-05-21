/*
 * std::async_io runtime — backs the AsyncStdin / AsyncReadLineFuture
 * surface declared in library/std/async_io/src/lib.rvn.
 *
 * Spec: docs/prompts/v1/15_phase4_async.md DoD bullet 4
 * ("All 9 stdlib types have Async* variants"). This is the AsyncStdin
 * slice; AsyncStdout / AsyncStderr are explicitly deferred to v1.1
 * (kernel buffers writes; blocking writes via std::io cover the use
 * case for v1).
 *
 * Why a separate package and not a tack-on to async_fs:
 *   - Lifecycle model is different: stdin's fd is owned by the parent
 *     process and only borrowed by the read state. async_fs's state
 *     opens (and on drop, closes) its own fd. Conflating the two
 *     leaks ownership concerns into the wrong abstraction.
 *   - The non-blocking flag must be restored on drop. async_fs never
 *     touches the flags of an fd it doesn't own. Keeping the
 *     fcntl save/restore dance in its own package documents the
 *     responsibility.
 *
 * Reactor coupling: NO new reactor primitives. Uses
 *   riven_reactor_register_fd_read / _deregister / _current_handle
 * shipped in sub-phase 4B (declared in async_fs/src/lib.rvn — we
 * re-declare them here in the .rvn so this package stands alone).
 *
 * Symbols exported here:
 *   riven_async_stdin_state_new() -> state*
 *       Sets O_NONBLOCK on fd 0, saving the previous flags in the
 *       state struct for restoration on free. If fd 0 is already
 *       non-blocking, the save still records that (drop is a no-op
 *       in that case).
 *   riven_async_stdin_state_get_fd(state) -> int64
 *       Always 0 — there for the Riven side to feed to
 *       reactor_register_fd_read without hard-coding the fd number.
 *   riven_async_stdin_step(state) -> int
 *       0 progress, 1 EAGAIN, 2 line-or-EOF reached, 3 error.
 *       "Line-or-EOF" means: we hit either '\n' (included in the
 *       returned String per Rust's BufRead::read_line semantics) or
 *       EOF (read returned 0 with no preceding bytes in this poll
 *       cycle — in that case the returned String is empty and the
 *       caller observes it as the conventional EOF signal).
 *   riven_async_stdin_state_take_result(state) -> Result[String, IoError]
 *   riven_async_stdin_state_free(state) -> ()
 *       Restores the original fcntl flags on fd 0. Idempotent.
 */

#include "../../core/runtime/runtime.h"

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <unistd.h>

typedef struct {
    int fd;              /* always 0 in v1; kept as a field so reactor
                          * registration goes through state_get_fd
                          * instead of a magic constant. */
    int saved_flags;     /* fcntl(0, F_GETFL) at construction. */
    int flags_saved;     /* 1 once saved_flags is valid; 0 if fcntl failed
                          * (in which case drop won't try to restore — we
                          * never modified them). */
    char *buf;
    size_t len;
    size_t cap;
    int err_tag;         /* set when err_set != 0 */
    int err_set;
    int done;            /* 1 once '\n' or EOF reached */
    int result_taken;
    char *out_str;       /* canonical-pool String pointer once done */
} RivenAsyncStdinState;

void *riven_async_stdin_state_new(void) {
    RivenAsyncStdinState *s =
        (RivenAsyncStdinState *)riven_alloc(sizeof(RivenAsyncStdinState));
    s->fd = 0;
    s->saved_flags = 0;
    s->flags_saved = 0;
    s->buf = NULL;
    s->len = 0;
    s->cap = 0;
    s->err_tag = 0;
    s->err_set = 0;
    s->done = 0;
    s->result_taken = 0;
    s->out_str = NULL;

    /* Save current flags; set O_NONBLOCK. If fcntl fails (extremely
     * unusual — fd 0 not open?), record an error and skip the flag
     * change. The step function will surface it on first call. */
    int flags = fcntl(0, F_GETFL, 0);
    if (flags < 0) {
        s->err_set = 1;
        s->err_tag = RIVEN_IO_ERROR_INVALID_INPUT;
        return s;
    }
    s->saved_flags = flags;
    s->flags_saved = 1;
    if (!(flags & O_NONBLOCK)) {
        if (fcntl(0, F_SETFL, flags | O_NONBLOCK) < 0) {
            /* Couldn't go non-blocking — proceed in blocking mode.
             * step() will still work (read will block until data),
             * the future just won't park on EAGAIN. This is a soft
             * degradation rather than a hard failure. */
            s->flags_saved = 0;   /* don't restore what we didn't set */
        }
    } else {
        /* Already non-blocking — nothing to restore on drop. */
        s->flags_saved = 0;
    }

    /* Pre-allocate a small line buffer. 256 covers most interactive
     * input; grows on demand below. */
    s->cap = 256;
    s->buf = (char *)malloc(s->cap);
    if (!s->buf) riven_panic("riven_async_stdin_state_new: malloc failed");
    return s;
}

int64_t riven_async_stdin_state_get_fd(void *state) {
    if (!state) return -1;
    return (int64_t)((RivenAsyncStdinState *)state)->fd;
}

/* Single-byte read loop so we can stop precisely at '\n' without
 * over-reading into the next line. v1 trades throughput for shape
 * simplicity — interactive stdin is not a perf hotspot. v1.1 can
 * switch to a chunked read with a pushback buffer if profiling shows
 * this matters. */
int64_t riven_async_stdin_step(void *state) {
    if (!state) return 3;
    RivenAsyncStdinState *s = (RivenAsyncStdinState *)state;
    if (s->done) return 2;
    if (s->err_set) return 3;

    for (;;) {
        if (s->len + 2 > s->cap) {
            size_t next_cap = s->cap * 2;
            char *next = (char *)realloc(s->buf, next_cap);
            if (!next) {
                s->err_set = 1;
                s->err_tag = RIVEN_IO_ERROR_OUT_OF_MEMORY;
                return 3;
            }
            s->buf = next;
            s->cap = next_cap;
        }
        char c;
        ssize_t got = read(s->fd, &c, 1);
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
                case EBADF:  s->err_tag = RIVEN_IO_ERROR_INVALID_INPUT; break;
                case EPIPE:  s->err_tag = RIVEN_IO_ERROR_BROKEN_PIPE; break;
                default:     s->err_tag = RIVEN_IO_ERROR_OTHER; break;
            }
            return 3;
        }
        if (got == 0) {
            /* EOF — finalize whatever we've buffered (may be empty). */
            s->buf[s->len] = '\0';
            s->out_str = riven_string_from(s->buf);
            s->done = 1;
            return 2;
        }
        s->buf[s->len++] = c;
        if (c == '\n') {
            s->buf[s->len] = '\0';
            s->out_str = riven_string_from(s->buf);
            s->done = 1;
            return 2;
        }
    }
}

void *riven_async_stdin_state_take_result(void *state) {
    if (!state) {
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(RIVEN_IO_ERROR_INVALID_INPUT));
    }
    RivenAsyncStdinState *s = (RivenAsyncStdinState *)state;
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

void riven_async_stdin_state_free(void *state) {
    if (!state) return;
    RivenAsyncStdinState *s = (RivenAsyncStdinState *)state;
    /* Restore the original fcntl flags so the parent process sees stdin
     * as it left it. Idempotent: flags_saved==0 means we either didn't
     * change them or already restored. */
    if (s->flags_saved) {
        (void)fcntl(0, F_SETFL, s->saved_flags);
        s->flags_saved = 0;
    }
    if (s->buf) {
        free(s->buf);
        s->buf = NULL;
    }
    free(s);
}
