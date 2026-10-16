#include "../../core/runtime/runtime.h"

/* Phase 2 #06.5 T6: BufReader[R] / BufWriter[W] over File + TcpStream.
 *
 * v1 simplification (per docs/prompts/v1/06_5_phase2_sync_io_completeness.md §2):
 * we parameterize by monomorphization over the closed set {File, TcpStream}
 * only — no formal Read/Write mixin (that lands in v1.5 with the iterator
 * trait work). The runtime carries a 1-byte `kind` tag in the struct and
 * branches on it for every fill / flush; the cost is one branch per 8 KiB
 * transfer, lost in I/O latency.
 *
 * Wire layout (32 bytes — one `RuxenBufIo` struct for reader and writer):
 *
 *   +0   uint8  kind     — 0=File, 1=TcpStream
 *   +1   uint8  closed   — 1 once into_inner / drop has emptied the buffer
 *   +2   uint16 _pad     — reserved (zeroed)
 *   +4   uint32 cap      — capacity in bytes of `buf`
 *   +8   uint32 pos      — reader: next byte to return; writer: unused
 *   +12  uint32 filled   — reader: bytes valid in `buf`; writer: bytes pending
 *   +16  uint8* buf      — 8-byte aligned malloc(cap)
 *   +24  void*  inner    — borrowed RuxenFile* / RuxenTcpStream*; not owned
 *
 * Only construction differs per inner type (it records `kind`); every other
 * entry point, `into_inner` included, is one C symbol that each class's
 * `lib` block aliases.
 *
 * Ownership: the `inner` pointer is borrowed, NOT owned. `into_inner`
 * surrenders the original pointer back to Ruxen (caller resumes
 * responsibility for closing it). `_drop` does NOT close the inner File /
 * TcpStream — the inner has its own drop helper which runs in the same
 * scope-exit pass. We only free our own `buf`.
 *
 * BufWriter auto-flush: `ruxen_bufwriter_drop` calls the kind-appropriate
 * flush before freeing `buf`. Errors are swallowed (drop can't report).
 * Callers who care about flush errors call `.flush()` explicitly.
 *
 * Default capacity: 8 KiB (matches Rust's std::io::BufReader / BufWriter
 * defaults). `with_capacity(0, inner)` rounds up to 1 to keep invariants
 * sane (`cap == 0` would make `fill_buf` an infinite loop).
 */

#define RUXEN_BUFIO_KIND_FILE 0
#define RUXEN_BUFIO_KIND_TCP  1
#define RUXEN_BUFIO_DEFAULT_CAP 8192

typedef struct {
    uint8_t  kind;
    uint8_t  closed;
    uint16_t _pad;
    uint32_t cap;
    uint32_t pos;     /* reader: next byte to return; writer: unused */
    uint32_t filled;  /* reader: bytes valid in `buf`; writer: bytes pending */
    uint8_t *buf;
    void    *inner;
} RuxenBufIo;

_Static_assert(sizeof(RuxenBufIo) == 32,
    "RuxenBufIo wire layout drifted from documented 32-byte form");

typedef RuxenBufIo RuxenBufReader;
typedef RuxenBufIo RuxenBufWriter;

static uint32_t ruxen_bufio_round_cap(int64_t cap) {
    if (cap <= 0) return 1;
    if (cap > (int64_t)UINT32_MAX) return UINT32_MAX;
    return (uint32_t)cap;
}

static RuxenBufIo *ruxen_bufio_alloc(uint8_t kind, uint32_t cap, void *inner) {
    RuxenBufIo *b = (RuxenBufIo *)ruxen_alloc(sizeof(RuxenBufIo));
    b->kind = kind;
    b->closed = 0;
    b->_pad = 0;
    b->cap = cap;
    b->pos = 0;
    b->filled = 0;
    b->buf = (uint8_t *)ruxen_xmalloc(cap);
    b->inner = inner;
    return b;
}

/* The inner's fd, or -1 with errno = EBADF when the inner is missing or
 * closed. File and TcpStream share the `RuxenFdHandle` layout. */
static int ruxen_bufio_inner_fd(RuxenBufIo *b) {
    RuxenFdHandle *h = (RuxenFdHandle *)b->inner;
    if (!h || h->closed || h->fd < 0) {
        errno = EBADF;
        return -1;
    }
    return h->fd;
}

/* Free our buffer and mark closed. Shared by `into_inner` and drop. */
static void ruxen_bufio_release(RuxenBufIo *b) {
    if (b->buf) { free(b->buf); b->buf = NULL; }
    b->cap = 0;
    b->pos = 0;
    b->filled = 0;
    b->inner = NULL;
    b->closed = 1;
}

/* Write all `n` bytes to `fd`: write(2) for a File, send(2) with
 * MSG_NOSIGNAL for a TcpStream. Retries EINTR and partial writes.
 * Returns 0, or -1 with errno set. */
static int ruxen_bufio_write_fd(uint8_t kind, int fd, const uint8_t *data, size_t n) {
    size_t off = 0;
    while (off < n) {
        ssize_t put;
        do {
            if (kind == RUXEN_BUFIO_KIND_FILE) {
                put = write(fd, data + off, n - off);
            } else {
                put = send(fd, data + off, n - off, MSG_NOSIGNAL);
            }
        } while (put < 0 && errno == EINTR);
        if (put < 0) return -1;
        if (put == 0) { errno = EIO; return -1; }
        off += (size_t)put;
    }
    return 0;
}

/* ── BufReader ─────────────────────────────────────────────────────── */

RuxenBufReader *ruxen_bufreader_new_file(void *inner) {
    return ruxen_bufio_alloc(RUXEN_BUFIO_KIND_FILE, RUXEN_BUFIO_DEFAULT_CAP, inner);
}

RuxenBufReader *ruxen_bufreader_new_tcp(void *inner) {
    return ruxen_bufio_alloc(RUXEN_BUFIO_KIND_TCP, RUXEN_BUFIO_DEFAULT_CAP, inner);
}

RuxenBufReader *ruxen_bufreader_with_capacity_file(int64_t cap, void *inner) {
    return ruxen_bufio_alloc(RUXEN_BUFIO_KIND_FILE, ruxen_bufio_round_cap(cap), inner);
}

RuxenBufReader *ruxen_bufreader_with_capacity_tcp(int64_t cap, void *inner) {
    return ruxen_bufio_alloc(RUXEN_BUFIO_KIND_TCP, ruxen_bufio_round_cap(cap), inner);
}

/* Refill `buf` from the inner. Returns:
 *   >0  — bytes loaded (br->filled = n, br->pos = 0)
 *    0  — EOF
 *   <0  — failure, errno set (the caller maps via ruxen_io_error_from_errno)
 *
 * File reads via read(2); TcpStream reads via recv(2). Both retry on
 * EINTR. */
static ssize_t ruxen_bufreader_fill(RuxenBufReader *br) {
    if (br->pos < br->filled) return (ssize_t)(br->filled - br->pos);
    br->pos = 0;
    br->filled = 0;
    int fd = ruxen_bufio_inner_fd(br);
    if (fd < 0) return -1;
    ssize_t got;
    do {
        if (br->kind == RUXEN_BUFIO_KIND_FILE) {
            got = read(fd, br->buf, br->cap);
        } else {
            got = recv(fd, br->buf, br->cap, 0);
        }
    } while (got < 0 && errno == EINTR);
    if (got < 0) return -1;
    br->filled = (uint32_t)got;
    return got;
}

/* `BufReader.read_line() -> Result[Option[String], IoError]`.
 * Returns Ok(Some(line)) where `line` includes the trailing '\n' if one
 * was seen, or just the remaining bytes at EOF. Returns Ok(None) at
 * true EOF (no bytes read). Err on real I/O failure.
 *
 * v1 simplification: we materialize the line into a single malloc'd
 * buffer that grows by doubling. For typical 80-200 char lines this is
 * one or two reallocs. */
void *ruxen_bufreader_read_line(RuxenBufReader *br) {
    if (!br || br->closed) return ruxen_io_error_invalid_input();
    RuxenByteBuf out;
    ruxen_byte_buf_init(&out, 128);
    int saw_any = 0;
    int saw_nl = 0;
    while (!saw_nl) {
        ssize_t avail = ruxen_bufreader_fill(br);
        if (avail < 0) {
            int saved = errno;
            free(out.data);
            return ruxen_io_error_from_errno(saved);
        }
        if (avail == 0) break; /* EOF */
        saw_any = 1;
        /* Scan the buffered window for '\n'. */
        uint32_t i = br->pos;
        uint32_t end = br->filled;
        uint32_t nl_at = end;
        while (i < end) {
            if (br->buf[i] == (uint8_t)'\n') { nl_at = i; break; }
            i++;
        }
        uint32_t take_end = (nl_at < end) ? (nl_at + 1) : end;
        size_t take = (size_t)(take_end - br->pos);
        ruxen_byte_buf_reserve(&out, take);
        memcpy(out.data + out.len, br->buf + br->pos, take);
        out.len += take;
        br->pos = take_end;
        if (nl_at < end) saw_nl = 1;
    }
    if (!saw_any) {
        free(out.data);
        /* Ok(None) — the None variant has no payload; tag 0 in the
         * Option layout. */
        return ruxen_result_ok_value((int64_t)ruxen_option_none());
    }
    char *line = ruxen_byte_buf_finish(&out);
    char *s = ruxen_string_from(line);
    free(line);
    return ruxen_result_ok_value((int64_t)ruxen_option_some((int64_t)s));
}

/* `BufReader.read(buf: &var Array[U8]) -> Result[Int, IoError]`. Pulls
 * the next chunk from the inner (refilling if empty), then drains the
 * buffered window into `buf` as one int64 slot per byte — same wire
 * shape as `ruxen_file_read` / `ruxen_tcp_read_bytes`. */
void *ruxen_bufreader_read(RuxenBufReader *br, RuxenVec *buf) {
    if (!br || br->closed || !buf) return ruxen_io_error_invalid_input();
    ssize_t avail = ruxen_bufreader_fill(br);
    if (avail < 0) {
        return ruxen_io_error_from_errno(errno);
    }
    if (avail == 0) {
        return ruxen_result_ok_value(0);
    }
    uint32_t take = br->filled - br->pos;
    for (uint32_t i = 0; i < take; i++) {
        ruxen_vec_push(buf, (int64_t)br->buf[br->pos + i]);
    }
    br->pos = br->filled;
    return ruxen_result_ok_value((int64_t)take);
}

/* `BufReader.into_inner() -> R`. Surrenders the inner pointer back to
 * the caller (whose ownership we'd only borrowed) and marks ourselves
 * closed so the drop helper doesn't double-free our buffer. The buffer
 * is freed here, immediately. */
void *ruxen_bufreader_into_inner(RuxenBufReader *br) {
    if (!br) return NULL;
    void *inner = br->inner;
    ruxen_bufio_release(br);
    return inner;
}

/* Drop helper — frees our buffer only. The inner File / TcpStream has
 * its own drop helper that runs in the same scope-exit pass. Registered
 * in `mir/lower/collect.rs::collect_user_drop_classes`. */
void ruxen_bufreader_drop(RuxenBufReader *br) {
    if (!br) return;
    if (br->closed) return;
    ruxen_bufio_release(br);
}

/* ── BufWriter ─────────────────────────────────────────────────────── */

RuxenBufWriter *ruxen_bufwriter_new_file(void *inner) {
    return ruxen_bufio_alloc(RUXEN_BUFIO_KIND_FILE, RUXEN_BUFIO_DEFAULT_CAP, inner);
}

RuxenBufWriter *ruxen_bufwriter_new_tcp(void *inner) {
    return ruxen_bufio_alloc(RUXEN_BUFIO_KIND_TCP, RUXEN_BUFIO_DEFAULT_CAP, inner);
}

RuxenBufWriter *ruxen_bufwriter_with_capacity_file(int64_t cap, void *inner) {
    return ruxen_bufio_alloc(RUXEN_BUFIO_KIND_FILE, ruxen_bufio_round_cap(cap), inner);
}

RuxenBufWriter *ruxen_bufwriter_with_capacity_tcp(int64_t cap, void *inner) {
    return ruxen_bufio_alloc(RUXEN_BUFIO_KIND_TCP, ruxen_bufio_round_cap(cap), inner);
}

/* Emit the pending buffer to the inner. Returns 0 on success, -1 with
 * errno set on failure (caller maps via ruxen_io_error_from_errno). */
static int ruxen_bufwriter_emit(RuxenBufWriter *bw) {
    if (bw->filled == 0) return 0;
    int fd = ruxen_bufio_inner_fd(bw);
    if (fd < 0) return -1;
    if (ruxen_bufio_write_fd(bw->kind, fd, bw->buf, bw->filled) != 0) return -1;
    bw->filled = 0;
    return 0;
}

/* Append the bytes in `staged` (length `n`) to `bw->buf`, flushing
 * whenever we'd overflow. Returns 0 on success, -1 on emit error
 * (errno set). Splits one user-call into 0+ emits + one residual copy.
 *
 * The "byte larger than cap" case flushes the empty buffer first and
 * writes the user's bytes straight through the inner — same shape as
 * Rust's BufWriter::write_all fast path. */
static int ruxen_bufwriter_append(RuxenBufWriter *bw, const uint8_t *staged, size_t n) {
    if (n == 0) return 0;
    /* If the incoming chunk wouldn't fit even into an empty buffer,
     * flush whatever is pending then write straight through. */
    if (n >= bw->cap) {
        if (ruxen_bufwriter_emit(bw) != 0) return -1;
        int fd = ruxen_bufio_inner_fd(bw);
        if (fd < 0) return -1;
        return ruxen_bufio_write_fd(bw->kind, fd, staged, n);
    }
    /* Normal path: copy into the residual buffer, flushing when full. */
    size_t off = 0;
    while (off < n) {
        size_t space = (size_t)(bw->cap - bw->filled);
        if (space == 0) {
            if (ruxen_bufwriter_emit(bw) != 0) return -1;
            space = bw->cap;
        }
        size_t chunk = (n - off < space) ? (n - off) : space;
        memcpy(bw->buf + bw->filled, staged + off, chunk);
        bw->filled += (uint32_t)chunk;
        off += chunk;
    }
    return 0;
}

/* Narrow `bytes` (one int64 slot per byte) and append them. Returns 0,
 * or -1 with errno set. Shared by `write` and `write_all`. */
static int ruxen_bufwriter_append_vec(RuxenBufWriter *bw, RuxenVec *bytes) {
    size_t n = (size_t)bytes->len;
    uint8_t *staged = (uint8_t *)ruxen_xmalloc(n > 0 ? n : 1);
    for (size_t i = 0; i < n; i++) {
        staged[i] = (uint8_t)(bytes->data[i] & 0xFF);
    }
    int rc = ruxen_bufwriter_append(bw, staged, n);
    int saved = errno;
    free(staged);
    errno = saved;
    return rc;
}

/* `BufWriter.write(bytes: &Array[U8]) -> Result[Int, IoError]`. Single
 * call → all bytes either accepted into the buffer or partially flushed
 * and the remainder buffered. We return Ok(bytes.len) on success
 * because the user-visible contract is "you handed me N bytes, I took
 * N bytes" (their fate-after-buffering is on flush). Mirrors std::io's
 * BufWriter::write semantics where write returns Ok(buf.len()) when it
 * succeeds in queueing the bytes. */
void *ruxen_bufwriter_write(RuxenBufWriter *bw, RuxenVec *bytes) {
    if (!bw || bw->closed || !bytes) return ruxen_io_error_invalid_input();
    if (ruxen_bufwriter_append_vec(bw, bytes) != 0) {
        return ruxen_io_error_from_errno(errno);
    }
    return ruxen_result_ok_value((int64_t)bytes->len);
}

void *ruxen_bufwriter_write_all(RuxenBufWriter *bw, RuxenVec *bytes) {
    if (!bw || bw->closed || !bytes) return ruxen_io_error_invalid_input();
    if (ruxen_bufwriter_append_vec(bw, bytes) != 0) {
        return ruxen_io_error_from_errno(errno);
    }
    return ruxen_result_ok_value(0);
}

void *ruxen_bufwriter_write_str(RuxenBufWriter *bw, const char *s) {
    if (!bw || bw->closed) return ruxen_io_error_invalid_input();
    if (!s) s = "";
    size_t n = strlen(s);
    int rc = ruxen_bufwriter_append(bw, (const uint8_t *)s, n);
    if (rc != 0) {
        return ruxen_io_error_from_errno(errno);
    }
    return ruxen_result_ok_value(0);
}

void *ruxen_bufwriter_flush(RuxenBufWriter *bw) {
    if (!bw || bw->closed) return ruxen_io_error_invalid_input();
    if (ruxen_bufwriter_emit(bw) != 0) {
        return ruxen_io_error_from_errno(errno);
    }
    return ruxen_result_ok_value(0);
}

/* `BufWriter.into_inner() -> Result[W, IoError]`. Flushes the residual
 * first; on flush failure returns Err (and the inner is NOT surrendered
 * — Rust's `IntoInnerError` carries both the error and the BufWriter,
 * but the v1 surface returns just the error). On success surrenders
 * the inner and marks closed. The returned value is the raw pointer
 * (caller's Result[W, IoError] payload). */
void *ruxen_bufwriter_into_inner(RuxenBufWriter *bw) {
    if (!bw) return ruxen_io_error_invalid_input();
    if (bw->closed) return ruxen_io_error_invalid_input();
    if (ruxen_bufwriter_emit(bw) != 0) {
        return ruxen_io_error_from_errno(errno);
    }
    void *inner = bw->inner;
    ruxen_bufio_release(bw);
    return ruxen_result_ok_value((int64_t)inner);
}

/* Drop helper — best-effort flush, then frees our buffer. Errors are
 * swallowed (drop has nobody to surface to). The inner has its own
 * drop helper that runs in the same scope-exit pass. */
void ruxen_bufwriter_drop(RuxenBufWriter *bw) {
    if (!bw) return;
    if (bw->closed) return;
    /* Best-effort flush; swallow errors. */
    (void)ruxen_bufwriter_emit(bw);
    ruxen_bufio_release(bw);
}
