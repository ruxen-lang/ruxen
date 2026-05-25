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
 * Wire layout (32 bytes — same shape for reader and writer):
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
 * Ownership: the `inner` pointer is borrowed, NOT owned. `into_inner_*`
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
    uint32_t pos;
    uint32_t filled;
    uint8_t *buf;
    void    *inner;
} RuxenBufReader;

_Static_assert(sizeof(RuxenBufReader) == 32,
    "RuxenBufReader wire layout drifted from documented 32-byte form");

typedef struct {
    uint8_t  kind;
    uint8_t  closed;
    uint16_t _pad;
    uint32_t cap;
    uint32_t pos;     /* unused for writer (kept for parallel shape) */
    uint32_t filled;  /* bytes pending in `buf` */
    uint8_t *buf;
    void    *inner;
} RuxenBufWriter;

_Static_assert(sizeof(RuxenBufWriter) == 32,
    "RuxenBufWriter wire layout drifted from documented 32-byte form");

/* Build Result::Err(IoError::InvalidInput) for the guard paths
 * (closed receiver, null inner, etc.). Mirrors `ruxen_file_invalid_input`
 * in io/file.c. */
static void *ruxen_bufio_invalid_input(void) {
    return ruxen_result_err_value(
        (int64_t)ruxen_io_error_unit(RUXEN_IO_ERROR_INVALID_INPUT));
}

static uint32_t ruxen_bufio_round_cap(int64_t cap) {
    if (cap <= 0) return 1;
    if (cap > (int64_t)UINT32_MAX) return UINT32_MAX;
    return (uint32_t)cap;
}

/* Option constructors — inline because the runtime doesn't expose
 * a public `ruxen_option_*_value` family (the regular codegen path
 * allocates Option payloads from MIR `Alloc + SetField` rather than
 * via runtime calls). 16-byte layout: {i32 tag; i32 pad; i64 payload}
 * with tag 0 = None, tag 1 = Some — matches `ruxen_option_unwrap_or`
 * in core/alloc.c and the resolver's variant_idx assignment. */
static void *ruxen_bufio_option_none(void) {
    int64_t *out = (int64_t *)ruxen_alloc(16);
    *(int32_t *)out = 0; /* None */
    out[1] = 0;
    return out;
}

static void *ruxen_bufio_option_some(int64_t payload) {
    int64_t *out = (int64_t *)ruxen_alloc(16);
    *(int32_t *)out = 1; /* Some */
    out[1] = payload;
    return out;
}

/* ── BufReader ─────────────────────────────────────────────────────── */

static RuxenBufReader *ruxen_bufreader_alloc(uint8_t kind, uint32_t cap, void *inner) {
    RuxenBufReader *br = (RuxenBufReader *)ruxen_alloc(sizeof(RuxenBufReader));
    br->kind = kind;
    br->closed = 0;
    br->_pad = 0;
    br->cap = cap;
    br->pos = 0;
    br->filled = 0;
    br->buf = (uint8_t *)malloc(cap);
    if (!br->buf) ruxen_panic("out of memory");
    br->inner = inner;
    return br;
}

RuxenBufReader *ruxen_bufreader_new_file(void *inner) {
    return ruxen_bufreader_alloc(RUXEN_BUFIO_KIND_FILE, RUXEN_BUFIO_DEFAULT_CAP, inner);
}

RuxenBufReader *ruxen_bufreader_new_tcp(void *inner) {
    return ruxen_bufreader_alloc(RUXEN_BUFIO_KIND_TCP, RUXEN_BUFIO_DEFAULT_CAP, inner);
}

RuxenBufReader *ruxen_bufreader_with_capacity_file(int64_t cap, void *inner) {
    return ruxen_bufreader_alloc(RUXEN_BUFIO_KIND_FILE, ruxen_bufio_round_cap(cap), inner);
}

RuxenBufReader *ruxen_bufreader_with_capacity_tcp(int64_t cap, void *inner) {
    return ruxen_bufreader_alloc(RUXEN_BUFIO_KIND_TCP, ruxen_bufio_round_cap(cap), inner);
}

/* Refill `buf` from the inner. Returns:
 *   >0  — bytes loaded (br->filled = n, br->pos = 0)
 *    0  — EOF
 *   <0  — errno (negated so the caller can map via ruxen_io_error_from_errno)
 *
 * Reader-side branch on kind: File reads via the inner fd directly (read(2));
 * TcpStream reads via recv(2). Both retry on EINTR. */
static ssize_t ruxen_bufreader_fill(RuxenBufReader *br) {
    if (br->pos < br->filled) return (ssize_t)(br->filled - br->pos);
    br->pos = 0;
    br->filled = 0;
    if (!br->inner) {
        errno = EBADF;
        return -1;
    }
    int fd;
    if (br->kind == RUXEN_BUFIO_KIND_FILE) {
        fd = ((RuxenFile *)br->inner)->fd;
        if (((RuxenFile *)br->inner)->closed || fd < 0) {
            errno = EBADF;
            return -1;
        }
    } else {
        fd = ((RuxenTcpStream *)br->inner)->fd;
        if (((RuxenTcpStream *)br->inner)->closed || fd < 0) {
            errno = EBADF;
            return -1;
        }
    }
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
    if (!br || br->closed) return ruxen_bufio_invalid_input();
    size_t out_cap = 128;
    size_t out_len = 0;
    char  *out = (char *)malloc(out_cap);
    if (!out) ruxen_panic("out of memory");
    int saw_any = 0;
    int saw_nl = 0;
    while (!saw_nl) {
        ssize_t avail = ruxen_bufreader_fill(br);
        if (avail < 0) {
            int saved = errno;
            free(out);
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
        if (out_len + take + 1 > out_cap) {
            size_t next = out_cap;
            while (next < out_len + take + 1) next *= 2;
            char *grown = (char *)realloc(out, next);
            if (!grown) { free(out); ruxen_panic("out of memory"); }
            out = grown;
            out_cap = next;
        }
        memcpy(out + out_len, br->buf + br->pos, take);
        out_len += take;
        br->pos = take_end;
        if (nl_at < end) saw_nl = 1;
    }
    if (!saw_any) {
        free(out);
        /* Ok(None) — the None variant has no payload; tag 0 in the
         * Option layout. */
        return ruxen_result_ok_value((int64_t)ruxen_bufio_option_none());
    }
    out[out_len] = '\0';
    char *s = ruxen_string_from(out);
    free(out);
    return ruxen_result_ok_value((int64_t)ruxen_bufio_option_some((int64_t)s));
}

/* `BufReader.read(buf: &var Array[U8]) -> Result[Int, IoError]`. Pulls
 * the next chunk from the inner (refilling if empty), then drains the
 * buffered window into `buf` as one int64 slot per byte — same wire
 * shape as `ruxen_file_read` / `ruxen_tcp_read_bytes`. */
void *ruxen_bufreader_read(RuxenBufReader *br, RuxenVec *buf) {
    if (!br || br->closed || !buf) return ruxen_bufio_invalid_input();
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
static void *ruxen_bufreader_into_inner_common(RuxenBufReader *br) {
    if (!br) return NULL;
    void *inner = br->inner;
    if (br->buf) { free(br->buf); br->buf = NULL; }
    br->cap = 0;
    br->pos = 0;
    br->filled = 0;
    br->inner = NULL;
    br->closed = 1;
    return inner;
}

void *ruxen_bufreader_into_inner_file(RuxenBufReader *br) {
    return ruxen_bufreader_into_inner_common(br);
}

void *ruxen_bufreader_into_inner_tcp(RuxenBufReader *br) {
    return ruxen_bufreader_into_inner_common(br);
}

/* Drop helper — frees our buffer only. The inner File / TcpStream has
 * its own drop helper that runs in the same scope-exit pass. Registered
 * in `mir/lower/collect.rs::collect_user_drop_classes`. */
void ruxen_bufreader_drop(RuxenBufReader *br) {
    if (!br) return;
    if (br->closed) return;
    if (br->buf) { free(br->buf); br->buf = NULL; }
    br->cap = 0;
    br->pos = 0;
    br->filled = 0;
    br->inner = NULL;
    br->closed = 1;
}

/* ── BufWriter ─────────────────────────────────────────────────────── */

static RuxenBufWriter *ruxen_bufwriter_alloc(uint8_t kind, uint32_t cap, void *inner) {
    RuxenBufWriter *bw = (RuxenBufWriter *)ruxen_alloc(sizeof(RuxenBufWriter));
    bw->kind = kind;
    bw->closed = 0;
    bw->_pad = 0;
    bw->cap = cap;
    bw->pos = 0;
    bw->filled = 0;
    bw->buf = (uint8_t *)malloc(cap);
    if (!bw->buf) ruxen_panic("out of memory");
    bw->inner = inner;
    return bw;
}

RuxenBufWriter *ruxen_bufwriter_new_file(void *inner) {
    return ruxen_bufwriter_alloc(RUXEN_BUFIO_KIND_FILE, RUXEN_BUFIO_DEFAULT_CAP, inner);
}

RuxenBufWriter *ruxen_bufwriter_new_tcp(void *inner) {
    return ruxen_bufwriter_alloc(RUXEN_BUFIO_KIND_TCP, RUXEN_BUFIO_DEFAULT_CAP, inner);
}

RuxenBufWriter *ruxen_bufwriter_with_capacity_file(int64_t cap, void *inner) {
    return ruxen_bufwriter_alloc(RUXEN_BUFIO_KIND_FILE, ruxen_bufio_round_cap(cap), inner);
}

RuxenBufWriter *ruxen_bufwriter_with_capacity_tcp(int64_t cap, void *inner) {
    return ruxen_bufwriter_alloc(RUXEN_BUFIO_KIND_TCP, ruxen_bufio_round_cap(cap), inner);
}

/* Emit the pending buffer to the inner. Loops on partial writes;
 * retries on EINTR. Returns 0 on success, negative errno on failure
 * (caller maps via ruxen_io_error_from_errno). */
static int ruxen_bufwriter_emit(RuxenBufWriter *bw) {
    if (bw->filled == 0) return 0;
    if (!bw->inner) { errno = EBADF; return -1; }
    int fd;
    if (bw->kind == RUXEN_BUFIO_KIND_FILE) {
        fd = ((RuxenFile *)bw->inner)->fd;
        if (((RuxenFile *)bw->inner)->closed || fd < 0) { errno = EBADF; return -1; }
    } else {
        fd = ((RuxenTcpStream *)bw->inner)->fd;
        if (((RuxenTcpStream *)bw->inner)->closed || fd < 0) { errno = EBADF; return -1; }
    }
    uint32_t off = 0;
    while (off < bw->filled) {
        ssize_t put;
        do {
            if (bw->kind == RUXEN_BUFIO_KIND_FILE) {
                put = write(fd, bw->buf + off, bw->filled - off);
            } else {
                put = send(fd, bw->buf + off, bw->filled - off, MSG_NOSIGNAL);
            }
        } while (put < 0 && errno == EINTR);
        if (put < 0) return -1;
        if (put == 0) { errno = EIO; return -1; }
        off += (uint32_t)put;
    }
    bw->filled = 0;
    return 0;
}

/* Append the bytes in `staged` (length `n`) to `bw->buf`, flushing
 * whenever we'd overflow. Returns 0 on success, negative on emit error
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
        int fd;
        if (bw->kind == RUXEN_BUFIO_KIND_FILE) {
            fd = ((RuxenFile *)bw->inner)->fd;
        } else {
            fd = ((RuxenTcpStream *)bw->inner)->fd;
        }
        size_t off = 0;
        while (off < n) {
            ssize_t put;
            do {
                if (bw->kind == RUXEN_BUFIO_KIND_FILE) {
                    put = write(fd, staged + off, n - off);
                } else {
                    put = send(fd, staged + off, n - off, MSG_NOSIGNAL);
                }
            } while (put < 0 && errno == EINTR);
            if (put < 0) return -1;
            if (put == 0) { errno = EIO; return -1; }
            off += (size_t)put;
        }
        return 0;
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

/* `BufWriter.write(bytes: &Array[U8]) -> Result[Int, IoError]`. Single
 * call → all bytes either accepted into the buffer or partially flushed
 * and the remainder buffered. We return Ok(bytes.len) on success
 * because the user-visible contract is "you handed me N bytes, I took
 * N bytes" (their fate-after-buffering is on flush). Mirrors std::io's
 * BufWriter::write semantics where write returns Ok(buf.len()) when it
 * succeeds in queueing the bytes. */
void *ruxen_bufwriter_write(RuxenBufWriter *bw, RuxenVec *bytes) {
    if (!bw || bw->closed || !bytes) return ruxen_bufio_invalid_input();
    size_t n = (size_t)bytes->len;
    uint8_t *staged = (uint8_t *)malloc(n > 0 ? n : 1);
    if (!staged) ruxen_panic("out of memory");
    for (size_t i = 0; i < n; i++) {
        staged[i] = (uint8_t)(bytes->data[i] & 0xFF);
    }
    int rc = ruxen_bufwriter_append(bw, staged, n);
    if (rc != 0) {
        int saved = errno;
        free(staged);
        return ruxen_io_error_from_errno(saved);
    }
    free(staged);
    return ruxen_result_ok_value((int64_t)n);
}

void *ruxen_bufwriter_write_all(RuxenBufWriter *bw, RuxenVec *bytes) {
    if (!bw || bw->closed || !bytes) return ruxen_bufio_invalid_input();
    size_t n = (size_t)bytes->len;
    uint8_t *staged = (uint8_t *)malloc(n > 0 ? n : 1);
    if (!staged) ruxen_panic("out of memory");
    for (size_t i = 0; i < n; i++) {
        staged[i] = (uint8_t)(bytes->data[i] & 0xFF);
    }
    int rc = ruxen_bufwriter_append(bw, staged, n);
    if (rc != 0) {
        int saved = errno;
        free(staged);
        return ruxen_io_error_from_errno(saved);
    }
    free(staged);
    return ruxen_result_ok_value(0);
}

void *ruxen_bufwriter_write_str(RuxenBufWriter *bw, const char *s) {
    if (!bw || bw->closed) return ruxen_bufio_invalid_input();
    if (!s) s = "";
    size_t n = strlen(s);
    int rc = ruxen_bufwriter_append(bw, (const uint8_t *)s, n);
    if (rc != 0) {
        return ruxen_io_error_from_errno(errno);
    }
    return ruxen_result_ok_value(0);
}

void *ruxen_bufwriter_flush(RuxenBufWriter *bw) {
    if (!bw || bw->closed) return ruxen_bufio_invalid_input();
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
static void *ruxen_bufwriter_into_inner_common(RuxenBufWriter *bw) {
    if (!bw) return ruxen_bufio_invalid_input();
    if (bw->closed) return ruxen_bufio_invalid_input();
    if (ruxen_bufwriter_emit(bw) != 0) {
        return ruxen_io_error_from_errno(errno);
    }
    void *inner = bw->inner;
    if (bw->buf) { free(bw->buf); bw->buf = NULL; }
    bw->cap = 0;
    bw->pos = 0;
    bw->filled = 0;
    bw->inner = NULL;
    bw->closed = 1;
    return ruxen_result_ok_value((int64_t)inner);
}

void *ruxen_bufwriter_into_inner_file(RuxenBufWriter *bw) {
    return ruxen_bufwriter_into_inner_common(bw);
}

void *ruxen_bufwriter_into_inner_tcp(RuxenBufWriter *bw) {
    return ruxen_bufwriter_into_inner_common(bw);
}

/* Drop helper — best-effort flush, then frees our buffer. Errors are
 * swallowed (drop has nobody to surface to). The inner has its own
 * drop helper that runs in the same scope-exit pass. */
void ruxen_bufwriter_drop(RuxenBufWriter *bw) {
    if (!bw) return;
    if (bw->closed) return;
    /* Best-effort flush; swallow errors. */
    (void)ruxen_bufwriter_emit(bw);
    if (bw->buf) { free(bw->buf); bw->buf = NULL; }
    bw->cap = 0;
    bw->pos = 0;
    bw->filled = 0;
    bw->inner = NULL;
    bw->closed = 1;
}
