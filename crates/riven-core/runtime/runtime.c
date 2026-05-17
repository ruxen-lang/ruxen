/* Riven Language Runtime Library
 *
 * Provides basic I/O, string operations, and memory management.
 * Linked into every Riven executable.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <inttypes.h>
#include <stdbool.h>
#include <errno.h>
#include <time.h>
#include <unistd.h>
#include <sched.h>
#include <dirent.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <netdb.h>
#include <signal.h>
/* Phase 2 #06.5 T2: <fcntl.h> for O_RDONLY/O_WRONLY/O_CREAT/O_APPEND/
 * O_TRUNC/O_EXCL/SEEK_SET/SEEK_CUR/SEEK_END consumed by the File class. */
#include <fcntl.h>

/* Linux-only send() flag that suppresses SIGPIPE on a closed peer.
 * macOS / *BSD don't define it; on those platforms we set the
 * per-socket SO_NOSIGPIPE option after creating the fd, so the flag
 * being a no-op (0) is safe. */
#ifndef MSG_NOSIGNAL
#  define MSG_NOSIGNAL 0
#endif

/* ── Platform Assertions ──────────────────────────────────────────── */

_Static_assert(sizeof(void *) == sizeof(int64_t),
    "Riven requires a 64-bit platform (sizeof(void*) must equal sizeof(int64_t))");

_Static_assert(sizeof(void *) == 8,
    "Riven requires 64-bit pointers");

/* ── Forward Declarations ─────────────────────────────────────────── */

void riven_panic(const char *message);
char *riven_string_from(const char *s);
void *riven_alloc(uint64_t size);
void riven_dealloc(void *ptr);
char *riven_char_to_string(int64_t codepoint);
/* Simple Vec: { int64_t *data; uint64_t len; uint64_t cap; }.
 * The struct body is hoisted above the first user so the Phase 2 #06
 * `riven_command_args` (and any future early consumer) can access
 * `->len` / `->data[i]` directly. The full implementation
 * (`riven_vec_new` etc.) lives much later in the file at the
 * "── Vec Operations ──" section. */
typedef struct RivenVec {
    int64_t *data;
    uint64_t len;
    uint64_t cap;
} RivenVec;
RivenVec *riven_vec_new(void);
void riven_vec_push(RivenVec *v, int64_t item);
typedef struct RivenHash RivenHash;
RivenHash *riven_hash_new(void);
void riven_hash_insert(RivenHash *h, int64_t key, int64_t value);
/* Forward decls for the free helpers (called from `riven_vec_drop_*`).
 * We use the `_ORIG_FREE` sentinel so the drop_fixtures textual splice
 * (`free(` → `riven_test_free(`) does not mangle these call sites; the
 * actual link symbols are `riven_vec_free` / `riven_string_free`,
 * pinned via `RIVEN_ASM_LABEL` on the definitions below. */
/* The `_ORIG_FREE` C-side identifier is mapped to the public link
 * symbol (`riven_vec_free` / `riven_string_free` / `riven_hash_free`)
 * via `__asm__` labels. The labels MUST be visible on the forward
 * declaration here — macOS clang errors "cannot apply asm label to
 * function after its first use" if any caller (e.g.
 * `riven_fs_read_dir` further down the file) takes the address of
 * the symbol before the labelled declaration is parsed. Linux gcc
 * tolerates the late label; clang does not. See `docs/dev/rss-cap.md`-
 * style note in the splice header below for the textual-rewrite
 * intent of the `_ORIG_FREE` rename. */
#if defined(__APPLE__)
#  define RIVEN_ASM_LABEL(sym) __asm__("_" #sym)
#else
#  define RIVEN_ASM_LABEL(sym) __asm__(#sym)
#endif
void riven_vec_ORIG_FREE(RivenVec *v) RIVEN_ASM_LABEL(riven_vec_free);
void riven_string_ORIG_FREE(char *s) RIVEN_ASM_LABEL(riven_string_free);
/* Phase 2 #06.D2.S0: Formatter forward decl — asm label must appear
 * before any caller to satisfy macOS clang's "asm label after first
 * use" constraint (same pattern as riven_vec/string above). */
typedef struct RivenFormatter RivenFormatter;
void riven_fmt_formatter_ORIG_FREE(RivenFormatter *f) RIVEN_ASM_LABEL(riven_fmt_formatter_free);
static uint64_t riven_hash_str(const char *s);

static void *riven_result_ok_value(int64_t payload) {
    int64_t *result = (int64_t *)riven_alloc(16);
    *(int32_t *)result = 0; /* Ok */
    result[1] = payload;
    return result;
}

static void *riven_result_err_value(int64_t payload) {
    int64_t *result = (int64_t *)riven_alloc(16);
    *(int32_t *)result = 1; /* Err */
    result[1] = payload;
    return result;
}

/* ── IoError (tagged enum, Phase 2 #06.5) ─────────────────────────────
 *
 * Wire format (matches the Riven type-level `enum IoError` registered
 * in resolve/mod.rs — keep these constants in sync):
 *   offset 0: int32 tag
 *   offset 4: 4 bytes padding (payload aligns to 8)
 *   offset 8: variant-specific payload slot (one int64; pointer for
 *             `Other`, unused for unit variants)
 *
 * We allocate a uniform 16 bytes for every variant so the int64 slot
 * in `Result::Err` always points at a layout-stable struct.
 *
 * Variant tag values mirror the order in `resolve::register_builtins`:
 *   0  NotFound
 *   1  PermissionDenied
 *   2  AlreadyExists
 *   3  Interrupted
 *   4  WouldBlock
 *   5  InvalidInput
 *   6  UnexpectedEof
 *   7  BrokenPipe
 *   8  Other(message: String)
 *
 * Phase 2 #06.5 T1 — appended 11 message-carrying variants (idx 9..19)
 * to widen the classifier's coverage of POSIX errno without renumbering
 * the existing tags (callers may match on the numeric tag via
 * `IoError.kind()` and we must not invalidate those matches):
 *   9  ConnectionRefused(message: String)
 *  10  ConnectionReset(message: String)
 *  11  ConnectionAborted(message: String)
 *  12  NotConnected(message: String)
 *  13  AddrInUse(message: String)
 *  14  AddrNotAvailable(message: String)
 *  15  InvalidData(message: String)
 *  16  TimedOut(message: String)
 *  17  WriteZero(message: String)
 *  18  Unsupported(message: String)
 *  19  OutOfMemory(message: String)
 *
 * `InvalidData` and `WriteZero` have no portable errno mapping; they
 * exist for direct construction (parsers, serializers, short-write
 * detection in user code). The other nine are produced by
 * `riven_io_error_classify_errno` from real errno values.
 */
#define RIVEN_IO_ERROR_NOT_FOUND          0
#define RIVEN_IO_ERROR_PERMISSION_DENIED  1
#define RIVEN_IO_ERROR_ALREADY_EXISTS     2
#define RIVEN_IO_ERROR_INTERRUPTED        3
#define RIVEN_IO_ERROR_WOULD_BLOCK        4
#define RIVEN_IO_ERROR_INVALID_INPUT      5
#define RIVEN_IO_ERROR_UNEXPECTED_EOF     6
#define RIVEN_IO_ERROR_BROKEN_PIPE        7
#define RIVEN_IO_ERROR_OTHER              8
#define RIVEN_IO_ERROR_CONNECTION_REFUSED 9
#define RIVEN_IO_ERROR_CONNECTION_RESET   10
#define RIVEN_IO_ERROR_CONNECTION_ABORTED 11
#define RIVEN_IO_ERROR_NOT_CONNECTED      12
#define RIVEN_IO_ERROR_ADDR_IN_USE        13
#define RIVEN_IO_ERROR_ADDR_NOT_AVAILABLE 14
#define RIVEN_IO_ERROR_INVALID_DATA       15
#define RIVEN_IO_ERROR_TIMED_OUT          16
#define RIVEN_IO_ERROR_WRITE_ZERO         17
#define RIVEN_IO_ERROR_UNSUPPORTED        18
#define RIVEN_IO_ERROR_OUT_OF_MEMORY      19

static void *riven_io_error_unit(int32_t tag) {
    int64_t *err = (int64_t *)riven_alloc(16);
    *(int32_t *)err = tag;
    err[1] = 0;
    return err;
}

static void *riven_io_error_other(const char *message) {
    int64_t *err = (int64_t *)riven_alloc(16);
    *(int32_t *)err = RIVEN_IO_ERROR_OTHER;
    err[1] = (int64_t)riven_string_from(message ? message : "io error");
    return err;
}

/* Phase 2 #06.5 T1: allocator for the 11 new message-carrying variants
 * (ConnectionRefused, …, OutOfMemory). Layout is identical to `Other`
 * — int32 tag, 4 pad, char* payload — only the tag varies. Used by
 * the errno classifier path. User-side direct construction goes
 * through the synthesized enum constructor in MIR lowering, which
 * lays out the same 16-byte struct. */
static void *riven_io_error_struct(int32_t tag, const char *message) {
    int64_t *err = (int64_t *)riven_alloc(16);
    *(int32_t *)err = tag;
    err[1] = (int64_t)riven_string_from(message ? message : "io error");
    return err;
}

static int32_t riven_io_error_classify_errno(int saved_errno) {
    switch (saved_errno) {
        case ENOENT:        return RIVEN_IO_ERROR_NOT_FOUND;
        case EACCES:
        case EPERM:         return RIVEN_IO_ERROR_PERMISSION_DENIED;
        case EEXIST:        return RIVEN_IO_ERROR_ALREADY_EXISTS;
        case EINTR:         return RIVEN_IO_ERROR_INTERRUPTED;
#ifdef EAGAIN
        case EAGAIN:        return RIVEN_IO_ERROR_WOULD_BLOCK;
#endif
        case EINVAL:        return RIVEN_IO_ERROR_INVALID_INPUT;
        case EPIPE:         return RIVEN_IO_ERROR_BROKEN_PIPE;
        /* Phase 2 #06.5 T1 — network + resource-exhaustion classes. */
#ifdef ECONNREFUSED
        case ECONNREFUSED:  return RIVEN_IO_ERROR_CONNECTION_REFUSED;
#endif
#ifdef ECONNRESET
        case ECONNRESET:    return RIVEN_IO_ERROR_CONNECTION_RESET;
#endif
#ifdef ECONNABORTED
        case ECONNABORTED:  return RIVEN_IO_ERROR_CONNECTION_ABORTED;
#endif
#ifdef ENOTCONN
        case ENOTCONN:      return RIVEN_IO_ERROR_NOT_CONNECTED;
#endif
#ifdef EADDRINUSE
        case EADDRINUSE:    return RIVEN_IO_ERROR_ADDR_IN_USE;
#endif
#ifdef EADDRNOTAVAIL
        case EADDRNOTAVAIL: return RIVEN_IO_ERROR_ADDR_NOT_AVAILABLE;
#endif
#ifdef ETIMEDOUT
        case ETIMEDOUT:     return RIVEN_IO_ERROR_TIMED_OUT;
#endif
#ifdef ENOMEM
        case ENOMEM:        return RIVEN_IO_ERROR_OUT_OF_MEMORY;
#endif
#ifdef ENOSYS
        case ENOSYS:        return RIVEN_IO_ERROR_UNSUPPORTED;
#endif
        /* On Linux EOPNOTSUPP == ENOTSUP, so we guard the second
         * label to avoid a duplicate-case compile error. macOS keeps
         * them distinct. */
#ifdef ENOTSUP
        case ENOTSUP:       return RIVEN_IO_ERROR_UNSUPPORTED;
#endif
#if defined(EOPNOTSUPP) && (!defined(ENOTSUP) || EOPNOTSUPP != ENOTSUP)
        case EOPNOTSUPP:    return RIVEN_IO_ERROR_UNSUPPORTED;
#endif
        default:            return RIVEN_IO_ERROR_OTHER;
    }
}

/* Build a Result::Err(IoError) from a user-supplied message. The
 * resulting variant is always `Other(message)`. Call sites without a
 * meaningful errno (EOF, env-var-not-found, …) use this helper. */
static void *riven_io_error_message(const char *message) {
    return riven_result_err_value((int64_t)riven_io_error_other(message));
}

/* Returns true when the tag is one of the 11 message-carrying variants
 * added in #06.5 T1. Lets `from_errno` route the strerror payload into
 * the variant value rather than collapsing back to `Other`. */
static int riven_io_error_tag_has_message(int32_t tag) {
    switch (tag) {
        case RIVEN_IO_ERROR_CONNECTION_REFUSED:
        case RIVEN_IO_ERROR_CONNECTION_RESET:
        case RIVEN_IO_ERROR_CONNECTION_ABORTED:
        case RIVEN_IO_ERROR_NOT_CONNECTED:
        case RIVEN_IO_ERROR_ADDR_IN_USE:
        case RIVEN_IO_ERROR_ADDR_NOT_AVAILABLE:
        case RIVEN_IO_ERROR_INVALID_DATA:
        case RIVEN_IO_ERROR_TIMED_OUT:
        case RIVEN_IO_ERROR_WRITE_ZERO:
        case RIVEN_IO_ERROR_UNSUPPORTED:
        case RIVEN_IO_ERROR_OUT_OF_MEMORY:
            return 1;
        default:
            return 0;
    }
}

/* Build a Result::Err(IoError) from a captured errno. Maps the errno
 * onto a curated variant when possible; falls back to
 * `Other(strerror(errno))`. Always capture errno into a local before
 * calling — any subsequent libc call may clobber it. */
static void *riven_io_error_from_errno(int saved_errno) {
    int32_t tag = riven_io_error_classify_errno(saved_errno);
    if (tag == RIVEN_IO_ERROR_OTHER) {
        return riven_io_error_message(strerror(saved_errno));
    }
    if (riven_io_error_tag_has_message(tag)) {
        return riven_result_err_value(
            (int64_t)riven_io_error_struct(tag, strerror(saved_errno)));
    }
    return riven_result_err_value((int64_t)riven_io_error_unit(tag));
}

/* `IoError.message() -> String`. Wired in `codegen/runtime.rs`
 * (`"IoError_message" -> "riven_io_error_get_message"`). Returns a
 * heap-allocated String pointer (interned static for unit variants;
 * the captured payload for variants that carry one). */
char *riven_io_error_get_message(void *err) {
    if (!err) {
        return riven_string_from("io error");
    }
    int32_t tag = *(int32_t *)err;
    switch (tag) {
        case RIVEN_IO_ERROR_NOT_FOUND:
            return riven_string_from("entity not found");
        case RIVEN_IO_ERROR_PERMISSION_DENIED:
            return riven_string_from("permission denied");
        case RIVEN_IO_ERROR_ALREADY_EXISTS:
            return riven_string_from("entity already exists");
        case RIVEN_IO_ERROR_INTERRUPTED:
            return riven_string_from("operation interrupted");
        case RIVEN_IO_ERROR_WOULD_BLOCK:
            return riven_string_from("operation would block");
        case RIVEN_IO_ERROR_INVALID_INPUT:
            return riven_string_from("invalid input");
        case RIVEN_IO_ERROR_UNEXPECTED_EOF:
            return riven_string_from("unexpected end of file");
        case RIVEN_IO_ERROR_BROKEN_PIPE:
            return riven_string_from("broken pipe");
        case RIVEN_IO_ERROR_OTHER:
        case RIVEN_IO_ERROR_CONNECTION_REFUSED:
        case RIVEN_IO_ERROR_CONNECTION_RESET:
        case RIVEN_IO_ERROR_CONNECTION_ABORTED:
        case RIVEN_IO_ERROR_NOT_CONNECTED:
        case RIVEN_IO_ERROR_ADDR_IN_USE:
        case RIVEN_IO_ERROR_ADDR_NOT_AVAILABLE:
        case RIVEN_IO_ERROR_INVALID_DATA:
        case RIVEN_IO_ERROR_TIMED_OUT:
        case RIVEN_IO_ERROR_WRITE_ZERO:
        case RIVEN_IO_ERROR_UNSUPPORTED:
        case RIVEN_IO_ERROR_OUT_OF_MEMORY: {
            char *msg = (char *)((int64_t *)err)[1];
            if (msg) {
                return msg;
            }
            switch (tag) {
                case RIVEN_IO_ERROR_CONNECTION_REFUSED: return riven_string_from("connection refused");
                case RIVEN_IO_ERROR_CONNECTION_RESET:   return riven_string_from("connection reset");
                case RIVEN_IO_ERROR_CONNECTION_ABORTED: return riven_string_from("connection aborted");
                case RIVEN_IO_ERROR_NOT_CONNECTED:      return riven_string_from("not connected");
                case RIVEN_IO_ERROR_ADDR_IN_USE:        return riven_string_from("address in use");
                case RIVEN_IO_ERROR_ADDR_NOT_AVAILABLE: return riven_string_from("address not available");
                case RIVEN_IO_ERROR_INVALID_DATA:       return riven_string_from("invalid data");
                case RIVEN_IO_ERROR_TIMED_OUT:          return riven_string_from("operation timed out");
                case RIVEN_IO_ERROR_WRITE_ZERO:         return riven_string_from("write zero");
                case RIVEN_IO_ERROR_UNSUPPORTED:        return riven_string_from("unsupported");
                case RIVEN_IO_ERROR_OUT_OF_MEMORY:      return riven_string_from("out of memory");
                default:                                return riven_string_from("io error");
            }
        }
        default:
            return riven_string_from("io error");
    }
}

/* `IoError.kind() -> IoErrorKind`. Wired in `codegen/runtime.rs`
 * (`"IoError_kind" -> "riven_io_error_kind"`). `IoErrorKind` is a
 * 20-unit-variant enum (no payload) whose tag values match IoError
 * 1:1. The codegen treats every enum value as a 16-byte boxed
 * pointer, so we keep the wire format uniform with IoError itself
 * — tag at offset 0, payload slot zeroed. */
void *riven_io_error_kind(void *err) {
    int32_t tag = err ? *(int32_t *)err : RIVEN_IO_ERROR_OTHER;
    int64_t *kind = (int64_t *)riven_alloc(16);
    *(int32_t *)kind = tag;
    kind[1] = 0;
    return kind;
}

static void *riven_stream_handle(FILE *stream) {
    FILE **handle = (FILE **)riven_alloc(sizeof(FILE *));
    *handle = stream;
    return handle;
}

static FILE *riven_stream_from_handle(void *handle, FILE *fallback) {
    return handle ? *(FILE **)handle : fallback;
}

static void *riven_stream_read_line(FILE *stream) {
    size_t cap = 128;
    size_t len = 0;
    char *buf = (char *)malloc(cap);
    int ch;

    if (!buf) {
        riven_panic("out of memory");
    }

    while ((ch = fgetc(stream)) != EOF) {
        if (len + 1 >= cap) {
            size_t next_cap = cap * 2;
            char *next = (char *)realloc(buf, next_cap);
            if (!next) {
                free(buf);
                riven_panic("out of memory");
            }
            buf = next;
            cap = next_cap;
        }
        buf[len++] = (char)ch;
        if (ch == '\n') {
            break;
        }
    }

    if (ferror(stream)) {
        int saved_errno = errno;
        free(buf);
        clearerr(stream);
        return riven_io_error_from_errno(saved_errno);
    }

    if (len == 0 && ch == EOF) {
        free(buf);
        return riven_result_err_value(
            (int64_t)riven_io_error_unit(RIVEN_IO_ERROR_UNEXPECTED_EOF));
    }

    buf[len] = '\0';
    return riven_result_ok_value((int64_t)buf);
}

static int riven_saved_argc = 0;
static char **riven_saved_argv = NULL;

static void riven_free_saved_argv(void) {
    if (!riven_saved_argv) {
        return;
    }

    for (int i = 0; i < riven_saved_argc; i++) {
        free(riven_saved_argv[i]);
    }
    free(riven_saved_argv);
    riven_saved_argv = NULL;
    riven_saved_argc = 0;
}

static void *riven_stream_read_to_string(FILE *stream) {
    size_t cap = 128;
    size_t len = 0;
    char *buf = (char *)malloc(cap);
    int ch;

    if (!buf) {
        riven_panic("out of memory");
    }

    while ((ch = fgetc(stream)) != EOF) {
        if (len + 1 >= cap) {
            size_t next_cap = cap * 2;
            char *next = (char *)realloc(buf, next_cap);
            if (!next) {
                free(buf);
                riven_panic("out of memory");
            }
            buf = next;
            cap = next_cap;
        }
        buf[len++] = (char)ch;
    }

    if (ferror(stream)) {
        int saved_errno = errno;
        free(buf);
        clearerr(stream);
        return riven_io_error_from_errno(saved_errno);
    }

    buf[len] = '\0';
    return riven_result_ok_value((int64_t)buf);
}

/* ── Printing ──────────────────────────────────────────────────────── */

void riven_puts(const char *s) {
    if (s) {
        puts(s);
    } else {
        puts("(nil)");
    }
}

void riven_print(const char *s) {
    if (s) {
        fputs(s, stdout);
    }
}

void riven_eputs(const char *s) {
    if (s) {
        fprintf(stderr, "%s\n", s);
    }
}

void *riven_read_line(void) {
    return riven_stream_read_line(stdin);
}

void *riven_stdin(void) {
    return riven_stream_handle(stdin);
}

void *riven_stdout(void) {
    return riven_stream_handle(stdout);
}

void *riven_stderr(void) {
    return riven_stream_handle(stderr);
}

void *riven_stdin_read_line(void *handle) {
    return riven_stream_read_line(riven_stream_from_handle(handle, stdin));
}

void *riven_stdin_read_to_string(void *handle) {
    return riven_stream_read_to_string(riven_stream_from_handle(handle, stdin));
}

/* Phase 2 stdlib (#06.2): `Stdin.lines() -> Vec[Result[String, IoError]]`.
 *
 * v1 simplification of Rust's `BufRead::lines`: read all of stdin to
 * EOF up front, split on '\n', materialise into a `RivenVec*` of
 * `Result::Ok(line)` elements. If the read itself fails the returned
 * vec contains a single `Result::Err(IoError(strerror))`. Newlines are
 * stripped from each line; a trailing '\n' does NOT produce a final
 * empty element (matches Rust). Empty input → empty vec. */
void *riven_stdin_lines(void *handle) {
    FILE *stream = riven_stream_from_handle(handle, stdin);

    size_t cap = 256;
    size_t len = 0;
    char *buf = (char *)malloc(cap);
    int ch;

    if (!buf) {
        riven_panic("out of memory");
    }

    while ((ch = fgetc(stream)) != EOF) {
        if (len + 1 >= cap) {
            size_t next_cap = cap * 2;
            char *next = (char *)realloc(buf, next_cap);
            if (!next) {
                free(buf);
                riven_panic("out of memory");
            }
            buf = next;
            cap = next_cap;
        }
        buf[len++] = (char)ch;
    }

    /* Capture errno BEFORE any other call that might clobber it. The
     * original `errno` from a failed `fgetc` could be overwritten by
     * `riven_vec_new` (allocator path) or `clearerr`, so snapshot it
     * here. */
    int saved_errno = errno;

    RivenVec *v = riven_vec_new();

    if (ferror(stream)) {
        free(buf);
        clearerr(stream);
        riven_vec_push(v, (int64_t)riven_io_error_from_errno(saved_errno));
        return v;
    }

    size_t line_start = 0;
    for (size_t i = 0; i < len; i++) {
        if (buf[i] == '\n') {
            size_t line_len = i - line_start;
            char *line = (char *)riven_alloc(line_len + 1);
            memcpy(line, &buf[line_start], line_len);
            line[line_len] = '\0';
            riven_vec_push(v, (int64_t)riven_result_ok_value((int64_t)line));
            line_start = i + 1;
        }
    }
    if (line_start < len) {
        size_t line_len = len - line_start;
        char *line = (char *)riven_alloc(line_len + 1);
        memcpy(line, &buf[line_start], line_len);
        line[line_len] = '\0';
        riven_vec_push(v, (int64_t)riven_result_ok_value((int64_t)line));
    }

    free(buf);
    return v;
}

void *riven_stdout_write_str(void *handle, const char *s) {
    FILE *stream = riven_stream_from_handle(handle, stdout);
    const char *text = s ? s : "";
    if (fputs(text, stream) == EOF) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(0);
}

void *riven_stdout_flush(void *handle) {
    FILE *stream = riven_stream_from_handle(handle, stdout);
    if (fflush(stream) != 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(0);
}

void *riven_stderr_write_str(void *handle, const char *s) {
    FILE *stream = riven_stream_from_handle(handle, stderr);
    const char *text = s ? s : "";
    if (fputs(text, stream) == EOF) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(0);
}

void *riven_stderr_flush(void *handle) {
    FILE *stream = riven_stream_from_handle(handle, stderr);
    if (fflush(stream) != 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(0);
}

/* ── Phase 2 stdlib (#06.1): Stdout / Stderr convenience methods.
 *
 * `print`, `println`, `eprint`, `eprintln` are the no-Result variants
 * of `write_str`. Failures are silently swallowed (matching the Rust
 * `print!` / `println!` / `eprintln!` macros' "panic on broken pipe"
 * lineage trimmed for v1 simplicity — we just discard the error).
 * The trailing-newline variants emit `\n` after the user-supplied
 * text via a second fputs so the buffer flush behaviour matches what
 * `puts(3)` would do on a stdio stream. */

void riven_stdout_print(void *handle, const char *s) {
    FILE *stream = riven_stream_from_handle(handle, stdout);
    if (s) {
        (void)fputs(s, stream);
    }
}

void riven_stdout_println(void *handle, const char *s) {
    FILE *stream = riven_stream_from_handle(handle, stdout);
    if (s) {
        (void)fputs(s, stream);
    }
    (void)fputc('\n', stream);
}

void riven_stderr_eprint(void *handle, const char *s) {
    FILE *stream = riven_stream_from_handle(handle, stderr);
    if (s) {
        (void)fputs(s, stream);
    }
}

void riven_stderr_eprintln(void *handle, const char *s) {
    FILE *stream = riven_stream_from_handle(handle, stderr);
    if (s) {
        (void)fputs(s, stream);
    }
    (void)fputc('\n', stream);
}

/* ── Env / Process / FS ────────────────────────────────────────────── */

void riven_env_init(int argc, char **argv) {
    static int riven_env_atexit_registered = 0;
    riven_free_saved_argv();

    if (argc <= 0) {
        return;
    }

    riven_saved_argv = (char **)malloc(sizeof(char *) * (size_t)argc);
    if (!riven_saved_argv) {
        riven_panic("out of memory");
    }

    riven_saved_argc = argc;
    for (int i = 0; i < argc; i++) {
        const char *arg = (argv && argv[i]) ? argv[i] : "";
        size_t len = strlen(arg);
        riven_saved_argv[i] = (char *)malloc(len + 1);
        if (!riven_saved_argv[i]) {
            riven_free_saved_argv();
            riven_panic("out of memory");
        }
        memcpy(riven_saved_argv[i], arg, len + 1);
    }

    /* Release the saved argv at process exit so leak-tracking test
       harnesses don't account for it as an outstanding allocation.
       Production: same effect, just keeps the memory tidy at exit. */
    if (!riven_env_atexit_registered) {
        atexit(riven_free_saved_argv);
        riven_env_atexit_registered = 1;
    }
}

int64_t riven_env_args_count(void) {
    return (int64_t)riven_saved_argc;
}

char *riven_env_args_at(int64_t index) {
    if (index < 0 || index >= riven_saved_argc || !riven_saved_argv) {
        return NULL;
    }

    return riven_string_from(riven_saved_argv[index]);
}

void *riven_env_args(void) {
    RivenVec *args = riven_vec_new();

    if (!riven_saved_argv || riven_saved_argc <= 0) {
        return args;
    }

    for (int i = 0; i < riven_saved_argc; i++) {
        riven_vec_push(args, (int64_t)riven_string_from(riven_saved_argv[i]));
    }

    return args;
}

void *riven_env_var(const char *name) {
    const char *value = getenv(name ? name : "");
    if (!value) {
        return riven_io_error_message("environment variable not found");
    }
    return riven_result_ok_value((int64_t)riven_string_from(value));
}

void riven_process_exit(int64_t code) {
    exit((int)code);
}

void *riven_fs_read_to_string(const char *path) {
    FILE *stream = fopen(path, "rb");
    void *result;

    if (!stream) {
        return riven_io_error_from_errno(errno);
    }

    result = riven_stream_read_to_string(stream);
    if (fclose(stream) != 0 && *(int32_t *)result == 0) {
        return riven_io_error_from_errno(errno);
    }

    return result;
}

void *riven_fs_write(const char *path, const char *contents) {
    FILE *stream = fopen(path, "wb");
    const char *text = contents ? contents : "";
    size_t len = strlen(text);

    if (!stream) {
        return riven_io_error_from_errno(errno);
    }

    if (fwrite(text, 1, len, stream) != len) {
        int saved_errno = errno;
        fclose(stream);
        return riven_io_error_from_errno(saved_errno);
    }

    if (fclose(stream) != 0) {
        return riven_io_error_from_errno(errno);
    }

    return riven_result_ok_value(0);
}

int64_t riven_fs_exists(const char *path) {
    if (!path) {
        return 0;
    }
    return access(path, F_OK) == 0 ? 1 : 0;
}

void *riven_fs_remove_file(const char *path) {
    if (!path) {
        return riven_io_error_message("path is null");
    }
    if (unlink(path) != 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(0);
}

void *riven_fs_create_dir(const char *path) {
    if (!path) {
        return riven_io_error_message("path is null");
    }
    if (mkdir(path, 0777) != 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(0);
}

void *riven_fs_create_dir_all(const char *path) {
    char *copy;
    size_t len;

    if (!path || !*path) {
        return riven_io_error_message("path is null");
    }

    len = strlen(path);
    copy = (char *)malloc(len + 1);
    if (!copy) {
        riven_panic("out of memory");
    }
    memcpy(copy, path, len + 1);

    if (len > 1 && copy[len - 1] == '/') {
        copy[len - 1] = '\0';
    }

    for (char *p = copy + 1; *p; p++) {
        if (*p != '/') {
            continue;
        }
        *p = '\0';
        if (mkdir(copy, 0777) != 0 && errno != EEXIST) {
            int saved_errno = errno;
            free(copy);
            return riven_io_error_from_errno(saved_errno);
        }
        *p = '/';
    }

    if (mkdir(copy, 0777) != 0 && errno != EEXIST) {
        int saved_errno = errno;
        free(copy);
        return riven_io_error_from_errno(saved_errno);
    }

    free(copy);
    return riven_result_ok_value(0);
}

void *riven_fs_rename(const char *from, const char *to) {
    if (!from || !to) {
        return riven_io_error_message("path is null");
    }
    if (rename(from, to) != 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(0);
}

/* ── Phase 2 stdlib (#06): env::vars / env::current_dir + fs::is_file
 * / fs::is_dir / fs::read_dir helpers.
 *
 * `riven_env_vars` walks `extern char **environ` and copies each
 * "KEY=VALUE" entry into a fresh RivenHash[String, String]. Both key
 * and value strings are heap-copied via `riven_string_from` so the
 * returned map owns its storage independently of `environ`.
 *
 * `riven_env_current_dir` calls `getcwd` with a growing buffer until
 * the cwd fits, then wraps the result in Result[String, IoError].
 *
 * `riven_fs_read_dir` returns Result[Vec[String], IoError] of the
 * directory entry names (skipping "." and ".."), heap-copied. Order
 * matches the underlying readdir() — caller must sort if needed.
 *
 * `riven_fs_is_file` / `riven_fs_is_dir` consult `stat()` and return
 * 1/0. They mirror `riven_fs_exists`'s convention of returning 0 on
 * any error rather than surfacing IoError, since they are typically
 * used as boolean predicates inside `if`. */

extern char **environ;

void *riven_env_vars(void) {
    RivenHash *h = riven_hash_new();
    if (!environ) {
        return h;
    }
    for (char **p = environ; *p; ++p) {
        const char *entry = *p;
        const char *eq = strchr(entry, '=');
        if (!eq) {
            /* Malformed entry without '='. POSIX says environ entries
               are KEY=VALUE; treat the whole thing as a key with an
               empty value rather than dropping it silently. */
            riven_hash_insert(
                h,
                (int64_t)riven_string_from(entry),
                (int64_t)riven_string_from("")
            );
            continue;
        }
        size_t key_len = (size_t)(eq - entry);
        char *key_buf = (char *)malloc(key_len + 1);
        if (!key_buf) {
            riven_panic("out of memory");
        }
        memcpy(key_buf, entry, key_len);
        key_buf[key_len] = '\0';
        riven_hash_insert(
            h,
            (int64_t)riven_string_from(key_buf),
            (int64_t)riven_string_from(eq + 1)
        );
        free(key_buf);
    }
    return h;
}

void *riven_env_current_dir(void) {
    size_t cap = 256;
    char *buf = (char *)malloc(cap);
    if (!buf) {
        riven_panic("out of memory");
    }
    while (1) {
        if (getcwd(buf, cap) != NULL) {
            void *result = riven_result_ok_value((int64_t)riven_string_from(buf));
            free(buf);
            return result;
        }
        if (errno != ERANGE) {
            int saved_errno = errno;
            free(buf);
            return riven_io_error_from_errno(saved_errno);
        }
        size_t next = cap * 2;
        char *next_buf = (char *)realloc(buf, next);
        if (!next_buf) {
            free(buf);
            riven_panic("out of memory");
        }
        buf = next_buf;
        cap = next;
    }
}

int64_t riven_fs_is_file(const char *path) {
    if (!path) {
        return 0;
    }
    struct stat st;
    if (stat(path, &st) != 0) {
        return 0;
    }
    return S_ISREG(st.st_mode) ? 1 : 0;
}

int64_t riven_fs_is_dir(const char *path) {
    if (!path) {
        return 0;
    }
    struct stat st;
    if (stat(path, &st) != 0) {
        return 0;
    }
    return S_ISDIR(st.st_mode) ? 1 : 0;
}

void *riven_fs_read_dir(const char *path) {
    if (!path) {
        return riven_io_error_message("path is null");
    }
    DIR *dir = opendir(path);
    if (!dir) {
        return riven_io_error_from_errno(errno);
    }
    RivenVec *names = riven_vec_new();
    errno = 0;
    struct dirent *entry;
    while ((entry = readdir(dir)) != NULL) {
        const char *n = entry->d_name;
        /* Skip "." and ".." — callers asking for the directory's
           contents almost never want them, and including them
           complicates downstream sorting / filtering. */
        if (n[0] == '.' && (n[1] == '\0' || (n[1] == '.' && n[2] == '\0'))) {
            continue;
        }
        riven_vec_push(names, (int64_t)riven_string_from(n));
        errno = 0;
    }
    int saved = errno;
    closedir(dir);
    if (saved != 0) {
        /* `_ORIG_FREE` survives the drop_fixtures textual `free(` →
         * `riven_test_free(` splice; the asm-label decl at the top of
         * this file maps it to the public `riven_vec_free` symbol. */
        riven_vec_ORIG_FREE(names);
        return riven_io_error_from_errno(saved);
    }
    return riven_result_ok_value((int64_t)names);
}

/* ── Phase 2 stdlib (#06): fs::metadata ────────────────────────────────
 *
 * `riven_fs_metadata(path) -> Result[Metadata, IoError]` calls `lstat(2)`
 * so symlinks are reported as `Symlink` rather than dereferenced. The
 * returned `Metadata` is a flat heap struct laid out as three int64s:
 *
 *     offset  0:  size          (file size in bytes, signed for parity
 *                                with Riven's `Int`; truncated on
 *                                pathological >2^63 sizes)
 *     offset  8:  modified_secs (UNIX timestamp from st_mtime; seconds
 *                                since 1970-01-01 UTC, ignoring sub-
 *                                second precision)
 *     offset 16:  kind          (0=File, 1=Dir, 2=Symlink, 3=Other —
 *                                covers fifos, sockets, block/char
 *                                devices; the v1 surface is just the
 *                                three predicates `is_file` / `is_dir`
 *                                / `is_symlink`)
 *
 * We pack the fields ourselves so the wire format is independent of
 * libc's `struct stat` ABI (which varies across libc versions and
 * platforms). Accessor helpers read the int64 slots directly. Drop is
 * the generic `riven_dealloc` (no inner heap to free), but we also
 * expose a `riven_metadata_free` alias for symmetry with future
 * accessor wrappers. */
#define RIVEN_METADATA_KIND_FILE    0
#define RIVEN_METADATA_KIND_DIR     1
#define RIVEN_METADATA_KIND_SYMLINK 2
#define RIVEN_METADATA_KIND_OTHER   3

void *riven_fs_metadata(const char *path) {
    if (!path) {
        return riven_io_error_message("path is null");
    }
    struct stat st;
    /* lstat so a symlink reports as Symlink instead of being followed.
     * Callers that want follow-semantics can call `read_to_string` or
     * `is_file` on the path directly, both of which use plain stat. */
    if (lstat(path, &st) != 0) {
        return riven_io_error_from_errno(errno);
    }
    int64_t *meta = (int64_t *)riven_alloc(24);
    meta[0] = (int64_t)st.st_size;
    meta[1] = (int64_t)st.st_mtime;
    int64_t kind;
    if (S_ISREG(st.st_mode)) {
        kind = RIVEN_METADATA_KIND_FILE;
    } else if (S_ISDIR(st.st_mode)) {
        kind = RIVEN_METADATA_KIND_DIR;
    } else if (S_ISLNK(st.st_mode)) {
        kind = RIVEN_METADATA_KIND_SYMLINK;
    } else {
        kind = RIVEN_METADATA_KIND_OTHER;
    }
    meta[2] = kind;
    return riven_result_ok_value((int64_t)meta);
}

int64_t riven_metadata_len(void *meta) {
    if (!meta) return 0;
    return ((int64_t *)meta)[0];
}

int64_t riven_metadata_modified(void *meta) {
    if (!meta) return 0;
    return ((int64_t *)meta)[1];
}

int64_t riven_metadata_is_file(void *meta) {
    if (!meta) return 0;
    return ((int64_t *)meta)[2] == RIVEN_METADATA_KIND_FILE ? 1 : 0;
}

int64_t riven_metadata_is_dir(void *meta) {
    if (!meta) return 0;
    return ((int64_t *)meta)[2] == RIVEN_METADATA_KIND_DIR ? 1 : 0;
}

int64_t riven_metadata_is_symlink(void *meta) {
    if (!meta) return 0;
    return ((int64_t *)meta)[2] == RIVEN_METADATA_KIND_SYMLINK ? 1 : 0;
}

/* Explicit free helper for symmetry with the other built-in Drop
 * surfaces (Formatter, Vec, Hash). The Metadata struct holds no inner
 * heap, so this is just `riven_dealloc` with a typed alias — the
 * scope-exit drop pass uses `riven_dealloc` directly for Class-typed
 * locals, so this is currently unused by codegen but exposed for FFI
 * symmetry and future expansion. */
void riven_metadata_free(void *meta) {
    if (meta) riven_dealloc(meta);
}

/* ── Phase 2 stdlib (#06.5 T3): fs completeness ───────────────────────
 *
 * Six free functions filling the v1 sync-fs surface beyond
 * read/write/exists/remove/create_dir(_all)/rename: copy, recursive
 * remove, canonicalize, atomic write, read_link, symlink. All return
 * Result[T, IoError]. NULL inputs surface `IoError.InvalidInput` (the
 * argument was syntactically wrong, not an I/O failure) — matching the
 * T2 File class's `riven_file_invalid_input` convention rather than the
 * legacy `riven_io_error_message` "Other" path used by earlier fs fns.
 *
 *   `riven_fs_copy(src, dst) -> Result[Int, IoError]`
 *     Read/write loop with a 64 KiB stack buffer. POSIX-portable; we
 *     deliberately do NOT use `copy_file_range(2)` (Linux-only) or
 *     `fcopyfile(3)` (macOS-only) — the loop is fast enough for v1 and
 *     keeps both platforms on the same code path. Returns bytes copied
 *     on success.
 *
 *   `riven_fs_remove_dir_all(path) -> Result[(), IoError]`
 *     Hand-rolled post-order traversal via opendir/readdir + lstat.
 *     `lstat` (not stat) so symlinks-to-directories are unlinked
 *     instead of being recursed into — matches `nftw(FTW_PHYS)` and
 *     std::fs::remove_dir_all semantics. We avoid `<ftw.h>` entirely
 *     to dodge the `_XOPEN_SOURCE` feature-test dance on Linux.
 *
 *   `riven_fs_canonicalize(path) -> Result[String, IoError]`
 *     `realpath(path, NULL)` — POSIX.1-2008, supported on both macOS
 *     and Linux. The returned buffer comes from libc `malloc`, so we
 *     copy it into a Riven String and `free()` (not `riven_dealloc`)
 *     the original.
 *
 *   `riven_fs_write_atomic(path, contents) -> Result[(), IoError]`
 *     Write to "<path>.tmp.<pid>" in the SAME directory as the target,
 *     fsync the data, fsync the containing directory, then rename(2)
 *     over the target. Same-directory placement is mandatory: rename(2)
 *     is only atomic across paths on the same filesystem, and a tmp in
 *     `/tmp` would cross mounts on most systems. On any error we unlink
 *     the temp file before returning Err so partial writes don't leak.
 *
 *   `riven_fs_read_link(path) -> Result[String, IoError]`
 *     Growing-buffer `readlink(2)` — start at 256 bytes, double on
 *     truncation. `readlink` returns the number of bytes written and
 *     does NOT null-terminate, so we always over-allocate by 1.
 *
 *   `riven_fs_symlink(target, linkpath) -> Result[(), IoError]`
 *     Thin wrapper around `symlink(2)`. Argument order matches the
 *     Riven surface (`symlink(target, link)`) which matches libc.
 */

static void *riven_fs_invalid_input(void) {
    return riven_result_err_value(
        (int64_t)riven_io_error_unit(RIVEN_IO_ERROR_INVALID_INPUT));
}

void *riven_fs_copy(const char *src, const char *dst) {
    if (!src || !dst) return riven_fs_invalid_input();
    int in_fd = open(src, O_RDONLY);
    if (in_fd < 0) {
        return riven_io_error_from_errno(errno);
    }
    /* 0644 mirrors File.create. The atomic-vs-non-atomic distinction
     * (truncate first, write into it) matches stdlib `fs::copy` — the
     * destination is left empty if the source open succeeded but the
     * write loop fails partway. */
    int out_fd = open(dst, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (out_fd < 0) {
        int saved = errno;
        close(in_fd);
        return riven_io_error_from_errno(saved);
    }
    char buf[65536];
    int64_t total = 0;
    for (;;) {
        ssize_t n;
        do {
            n = read(in_fd, buf, sizeof(buf));
        } while (n < 0 && errno == EINTR);
        if (n < 0) {
            int saved = errno;
            close(in_fd);
            close(out_fd);
            return riven_io_error_from_errno(saved);
        }
        if (n == 0) break;
        ssize_t written = 0;
        while (written < n) {
            ssize_t w;
            do {
                w = write(out_fd, buf + written, (size_t)(n - written));
            } while (w < 0 && errno == EINTR);
            if (w < 0) {
                int saved = errno;
                close(in_fd);
                close(out_fd);
                return riven_io_error_from_errno(saved);
            }
            written += w;
        }
        total += (int64_t)n;
    }
    close(in_fd);
    if (close(out_fd) != 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(total);
}

/* Recursive helper for remove_dir_all. Returns 0 on success, -1 on
 * failure with errno set. Post-order: empties children before rmdir'ing
 * the directory itself. Uses lstat so symlinks-to-directories are
 * unlinked rather than recursed-into. */
static int riven_fs_remove_dir_all_inner(const char *path) {
    struct stat st;
    if (lstat(path, &st) != 0) {
        return -1;
    }
    if (!S_ISDIR(st.st_mode)) {
        /* Symlink-to-dir or a regular file: unlink, don't recurse. */
        return unlink(path);
    }
    DIR *dir = opendir(path);
    if (!dir) return -1;
    struct dirent *entry;
    size_t path_len = strlen(path);
    int rc = 0;
    errno = 0;
    while ((entry = readdir(dir)) != NULL) {
        const char *n = entry->d_name;
        if (n[0] == '.' && (n[1] == '\0' || (n[1] == '.' && n[2] == '\0'))) {
            continue;
        }
        size_t n_len = strlen(n);
        char *child = (char *)malloc(path_len + 1 + n_len + 1);
        if (!child) {
            errno = ENOMEM;
            rc = -1;
            break;
        }
        memcpy(child, path, path_len);
        child[path_len] = '/';
        memcpy(child + path_len + 1, n, n_len + 1);
        if (riven_fs_remove_dir_all_inner(child) != 0) {
            int saved = errno;
            free(child);
            errno = saved;
            rc = -1;
            break;
        }
        free(child);
        errno = 0;
    }
    int saved = errno;
    closedir(dir);
    if (rc != 0) {
        errno = saved;
        return -1;
    }
    if (rmdir(path) != 0) {
        return -1;
    }
    return 0;
}

void *riven_fs_remove_dir_all(const char *path) {
    if (!path) return riven_fs_invalid_input();
    if (riven_fs_remove_dir_all_inner(path) != 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(0);
}

void *riven_fs_canonicalize(const char *path) {
    if (!path) return riven_fs_invalid_input();
    /* realpath(path, NULL) allocates via libc malloc on POSIX.1-2008
     * (both macOS and Linux). We always copy into a Riven String and
     * free the libc buffer separately. */
    char *resolved = realpath(path, NULL);
    if (!resolved) {
        return riven_io_error_from_errno(errno);
    }
    void *result = riven_result_ok_value((int64_t)riven_string_from(resolved));
    free(resolved);
    return result;
}

void *riven_fs_write_atomic(const char *path, const char *contents) {
    if (!path) return riven_fs_invalid_input();
    const char *text = contents ? contents : "";
    size_t text_len = strlen(text);

    /* Build "<path>.tmp.<pid>" in the SAME directory as `path` — rename(2)
     * is only atomic when source and target are on the same filesystem,
     * and same-directory is the simplest guarantee. */
    size_t path_len = strlen(path);
    /* ".tmp." (5) + 20 digits (max int64 width) + NUL = 26. */
    char *tmp = (char *)malloc(path_len + 32);
    if (!tmp) {
        riven_panic("out of memory");
    }
    int written = snprintf(tmp, path_len + 32, "%s.tmp.%ld",
                           path, (long)getpid());
    if (written < 0 || (size_t)written >= path_len + 32) {
        free(tmp);
        return riven_io_error_from_errno(EINVAL);
    }

    int fd = open(tmp, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        int saved = errno;
        free(tmp);
        return riven_io_error_from_errno(saved);
    }
    size_t off = 0;
    while (off < text_len) {
        ssize_t w;
        do {
            w = write(fd, text + off, text_len - off);
        } while (w < 0 && errno == EINTR);
        if (w < 0) {
            int saved = errno;
            close(fd);
            unlink(tmp);
            free(tmp);
            return riven_io_error_from_errno(saved);
        }
        off += (size_t)w;
    }
    /* fsync the file's data before rename. The directory entry change
     * (rename) plus an fsync on the file is the canonical Unix recipe
     * for "data hits disk before the new name is visible". A directory
     * fsync would also be needed to guarantee the rename itself is
     * durable across crash; we skip that for v1 — atomicity within a
     * single boot is the only guarantee we promise. */
    if (fsync(fd) != 0) {
        /* fsync failure on some FS / pseudo-fs (e.g. /tmp on certain
         * tmpfs configs) returns EINVAL; do not treat as fatal — the
         * fallback is rename without the durability promise. */
        int saved = errno;
        if (saved != EINVAL && saved != ENOTSUP) {
            close(fd);
            unlink(tmp);
            free(tmp);
            return riven_io_error_from_errno(saved);
        }
    }
    if (close(fd) != 0) {
        int saved = errno;
        unlink(tmp);
        free(tmp);
        return riven_io_error_from_errno(saved);
    }
    if (rename(tmp, path) != 0) {
        int saved = errno;
        unlink(tmp);
        free(tmp);
        return riven_io_error_from_errno(saved);
    }
    free(tmp);
    return riven_result_ok_value(0);
}

void *riven_fs_read_link(const char *path) {
    if (!path) return riven_fs_invalid_input();
    /* readlink does not null-terminate, returns count; on truncation
     * we cannot tell whether the buffer was exactly right or short.
     * Grow until the result is strictly less than the buffer size. */
    size_t cap = 256;
    char *buf = (char *)malloc(cap);
    if (!buf) {
        riven_panic("out of memory");
    }
    for (;;) {
        ssize_t n = readlink(path, buf, cap - 1);
        if (n < 0) {
            int saved = errno;
            free(buf);
            return riven_io_error_from_errno(saved);
        }
        if ((size_t)n < cap - 1) {
            buf[n] = '\0';
            void *result = riven_result_ok_value((int64_t)riven_string_from(buf));
            free(buf);
            return result;
        }
        /* Possibly truncated — grow and retry. */
        size_t next = cap * 2;
        char *next_buf = (char *)realloc(buf, next);
        if (!next_buf) {
            free(buf);
            riven_panic("out of memory");
        }
        buf = next_buf;
        cap = next;
    }
}

void *riven_fs_symlink(const char *target, const char *linkpath) {
    if (!target || !linkpath) return riven_fs_invalid_input();
    if (symlink(target, linkpath) != 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(0);
}

/* ── Phase 2 stdlib (#06): std.process.Command builder ────────────────
 *
 * Mirrors the `fs.metadata` "flat heap struct + accessors" pattern. The
 * surface is:
 *
 *     Command.new(prog).arg("a").args(["b","c"])
 *       .env("K","V").current_dir("/tmp")
 *       .status()  -> Result[ExitStatus, IoError]
 *       .output()  -> Result[Output, IoError]
 *
 * `.arg/.args/.env/.current_dir` mutate-in-place and return the same
 * Command pointer so chained calls compose. `.status` / `.output`
 * fork+execve the child, wait, then free the Command (consume-self
 * semantics — calling status then output on the same handle is UB and
 * documented as such; v1 has no linear-type checker to enforce it).
 *
 * Wire layouts (all int64-aligned; allocated via riven_alloc):
 *
 *   RivenCommand  (64 bytes)
 *     +0   char*    program          — strdup'd
 *     +8   int64    args_count
 *     +16  int64    args_cap
 *     +24  char**   args             — each entry strdup'd
 *     +32  int64    envs_count
 *     +40  int64    envs_cap
 *     +48  char**   envs             — each entry "KEY=VAL", strdup'd
 *     +56  char*    cwd              — strdup'd or NULL
 *
 *   RivenExitStatus (8 bytes)
 *     +0   int64    code             — POSIX-shell convention:
 *                                      0..=255 = normal exit;
 *                                      128+signal = signal termination;
 *                                      127 = fork/exec/waitpid failure
 *
 *   RivenOutput (24 bytes)
 *     +0   RivenExitStatus*  status  — sub-allocation, freed by Output_free
 *     +8   char*             stdout  — heap String (riven_string_from)
 *     +16  char*             stderr  — heap String
 *
 * `.spawn -> Child` (async-style handle with `.wait/.kill/.try_wait`)
 * is explicitly DEFERRED to v2 per the prompt — v1 ships the blocking
 * `.status` / `.output` terminals only. The runtime intentionally does
 * NOT include a Child struct so a v1 program cannot accidentally
 * reference the un-implemented surface.
 *
 * macOS / Linux notes:
 *   - `extern char **environ` is unavailable in dylib code on macOS
 *     (link errors); we read environ via `*_NSGetEnviron()` on Apple
 *     and direct extern elsewhere — same approach `riven_env_vars`
 *     already uses (line ~723). Since this runtime is statically
 *     linked into the produced binary the direct `extern` is fine on
 *     both platforms; we reuse the existing module-level declaration.
 *   - posix_spawn would be more portable but adds a second code path
 *     for env/cwd plumbing; we use fork+execve for parity with the
 *     existing `riven_process_run` and to keep the implementation
 *     focused. vfork is intentionally avoided (deprecated on macOS,
 *     fragile around any allocation between fork and exec).
 *   - PATH lookup: `execvp` does PATH lookup, but `execve` (which we
 *     need for explicit envp) does NOT. We therefore require an
 *     absolute path in `program` — callers can compute it via
 *     `path::join` or use `/usr/bin/env <name>` for PATH lookup. Tests
 *     use absolute paths (`/usr/bin/true`, `/bin/sh`, `/usr/bin/echo`).
 */

typedef struct {
    char *program;          /* +0  */
    int64_t args_count;     /* +8  */
    int64_t args_cap;       /* +16 */
    char **args;            /* +24 */
    int64_t envs_count;     /* +32 */
    int64_t envs_cap;       /* +40 */
    char **envs;            /* +48 */
    char *cwd;              /* +56 */
} RivenCommand;

_Static_assert(sizeof(RivenCommand) == 64,
    "RivenCommand wire layout drifted from documented 64-byte form");

typedef struct {
    int64_t code;
} RivenExitStatus;

typedef struct {
    RivenExitStatus *status;
    char *stdout_buf;
    char *stderr_buf;
} RivenOutput;

/* Helper: grow a (count, cap, ptr*) trio by one slot. Returns 0 on
 * success, -1 on allocation failure (callers panic). The strdup of
 * the new entry happens in the caller. */
static int riven_command_grow(int64_t *count, int64_t *cap, char ***arr) {
    if (*count + 1 > *cap) {
        int64_t new_cap = *cap == 0 ? 4 : (*cap) * 2;
        char **next = (char **)realloc(*arr, sizeof(char *) * (size_t)new_cap);
        if (!next) return -1;
        *arr = next;
        *cap = new_cap;
    }
    return 0;
}

RivenCommand *riven_command_new(const char *program) {
    RivenCommand *c = (RivenCommand *)riven_alloc(sizeof(RivenCommand));
    /* riven_alloc panics on OOM, so c is non-null here.
     * We use plain malloc+memcpy (via `riven_string_from`-style copy)
     * for the inner strings rather than `riven_string_from` itself —
     * inner strings are freed with `free(...)` in `riven_command_free_inner`,
     * which is the same allocator pairing. Keeping the Command's
     * internal strings out of the riven-managed String pool means
     * they don't need to participate in the Riven drop tracker. */
    const char *src = program ? program : "";
    size_t len = strlen(src);
    c->program = (char *)malloc(len + 1);
    if (!c->program) riven_panic("out of memory");
    memcpy(c->program, src, len + 1);
    c->args_count = 0;
    c->args_cap = 0;
    c->args = NULL;
    c->envs_count = 0;
    c->envs_cap = 0;
    c->envs = NULL;
    c->cwd = NULL;
    return c;
}

RivenCommand *riven_command_arg(RivenCommand *c, const char *arg) {
    if (!c) return c;
    if (riven_command_grow(&c->args_count, &c->args_cap, &c->args) != 0) {
        riven_panic("out of memory");
    }
    const char *src = arg ? arg : "";
    size_t len = strlen(src);
    char *copy = (char *)malloc(len + 1);
    if (!copy) riven_panic("out of memory");
    memcpy(copy, src, len + 1);
    c->args[c->args_count] = copy;
    c->args_count++;
    return c;
}

/* `.args(Array[String])` — bulk append. `args` is a Vec[String] whose
 * slots hold heap `char*` pointers (see "Vec Operations"). We strdup
 * each entry into the Command's owned storage so the source Vec can be
 * dropped independently. */
RivenCommand *riven_command_args(RivenCommand *c, RivenVec *args) {
    if (!c) return c;
    if (!args) return c;
    for (uint64_t i = 0; i < args->len; i++) {
        const char *src = (const char *)(uintptr_t)args->data[i];
        riven_command_arg(c, src);
    }
    return c;
}

RivenCommand *riven_command_env(RivenCommand *c, const char *key, const char *value) {
    if (!c) return c;
    const char *k = key ? key : "";
    const char *v = value ? value : "";
    size_t klen = strlen(k);
    size_t vlen = strlen(v);
    /* "KEY=VAL\0" */
    char *entry = (char *)malloc(klen + 1 + vlen + 1);
    if (!entry) riven_panic("out of memory");
    memcpy(entry, k, klen);
    entry[klen] = '=';
    memcpy(entry + klen + 1, v, vlen);
    entry[klen + 1 + vlen] = '\0';
    if (riven_command_grow(&c->envs_count, &c->envs_cap, &c->envs) != 0) {
        free(entry);
        riven_panic("out of memory");
    }
    c->envs[c->envs_count] = entry;
    c->envs_count++;
    return c;
}

RivenCommand *riven_command_current_dir(RivenCommand *c, const char *path) {
    if (!c) return c;
    if (c->cwd) {
        free(c->cwd);
        c->cwd = NULL;
    }
    if (path) {
        size_t len = strlen(path);
        c->cwd = (char *)malloc(len + 1);
        if (!c->cwd) riven_panic("out of memory");
        memcpy(c->cwd, path, len + 1);
    }
    return c;
}

/* Free a Command's inner heap. Used both as the scope-exit drop helper
 * (for the never-terminal case) and immediately before returning from
 * `.status` / `.output`. Tolerates NULL. */
static void riven_command_free_inner(RivenCommand *c) {
    if (!c) return;
    if (c->program) { free(c->program); c->program = NULL; }
    if (c->args) {
        for (int64_t i = 0; i < c->args_count; i++) {
            if (c->args[i]) free(c->args[i]);
        }
        free(c->args);
        c->args = NULL;
    }
    c->args_count = 0;
    c->args_cap = 0;
    if (c->envs) {
        for (int64_t i = 0; i < c->envs_count; i++) {
            if (c->envs[i]) free(c->envs[i]);
        }
        free(c->envs);
        c->envs = NULL;
    }
    c->envs_count = 0;
    c->envs_cap = 0;
    if (c->cwd) { free(c->cwd); c->cwd = NULL; }
}

/* Explicit Command free for the drop pass. Mirrors `riven_metadata_free`
 * — exposed so the scope-exit drop on a Command local that never reached
 * `.status` / `.output` releases the inner allocations rather than
 * leaking via the generic `riven_dealloc`. The MIR drop wiring (see
 * `insert_drops` in mir/lower.rs) maps Class types to `riven_dealloc`
 * by default, so we add `Command` to the `user_drop_classes` set so
 * `Command_drop(self)` is emitted before the spine dealloc. */
void riven_command_drop(RivenCommand *c) {
    if (!c) return;
    riven_command_free_inner(c);
    /* Caller's drop pass will emit riven_dealloc(c) next. */
}

/* Build a NULL-terminated char** envp for execve. Layout:
 *   [c->envs[0], c->envs[1], ..., NULL]
 * The returned array is malloc'd; entries are aliased into the
 * Command's storage (we do NOT strdup, because the child runs execve
 * which copies envp into the new image). Caller frees the array
 * spine; entries belong to the Command. */
static char **riven_command_build_envp(RivenCommand *c) {
    if (!c) return NULL;
    char **envp = (char **)malloc(sizeof(char *) * (size_t)(c->envs_count + 1));
    if (!envp) return NULL;
    for (int64_t i = 0; i < c->envs_count; i++) {
        envp[i] = c->envs[i];
    }
    envp[c->envs_count] = NULL;
    return envp;
}

/* Build the NULL-terminated argv (program followed by args). Caller
 * frees the array spine; entries belong to the Command. */
static char **riven_command_build_argv(RivenCommand *c) {
    if (!c) return NULL;
    char **argv = (char **)malloc(sizeof(char *) * (size_t)(c->args_count + 2));
    if (!argv) return NULL;
    argv[0] = c->program;
    for (int64_t i = 0; i < c->args_count; i++) {
        argv[i + 1] = c->args[i];
    }
    argv[c->args_count + 1] = NULL;
    return argv;
}

/* Pack a POSIX `waitpid` status into the v1 ExitStatus convention:
 *   normal exit:    0..=255
 *   signal:         128 + signum (matches `riven_process_run`)
 *   anything else:  127 (lump under the fork/exec failure code)
 */
static int64_t riven_command_pack_status(int wstatus) {
    if (WIFEXITED(wstatus))   return (int64_t)WEXITSTATUS(wstatus);
    if (WIFSIGNALED(wstatus)) return (int64_t)(128 + WTERMSIG(wstatus));
    return 127;
}

static RivenExitStatus *riven_exit_status_alloc(int64_t code) {
    RivenExitStatus *st = (RivenExitStatus *)riven_alloc(sizeof(RivenExitStatus));
    st->code = code;
    return st;
}

/* `Command.status() -> Result[ExitStatus, IoError]` — fork+execve a
 * child inheriting parent stdio, wait, return the packed exit code.
 *
 * Borrow semantics: the runtime does NOT free the Command — the MIR
 * drop pass emits `Command_drop(c) + riven_dealloc(c)` at scope exit
 * because `Command` is registered in `user_drop_classes`. To preserve
 * that invariant `Command_status` is also listed in the borrow-helper
 * exception in `compute_dealloc_safe_locals` so the receiver local
 * stays alloc-rooted across the call. Calling `.status` twice on the
 * same Command is therefore well-defined (it re-runs the child); the
 * documented "consume" semantic is enforced by typeck shape (`.status`
 * returning a Result, callers naturally let-bind and stop using the
 * Command afterwards) rather than by linear types — those land in v2
 * alongside `spawn -> Child`.
 *
 * fork failure / execve failure / waitpid failure all surface as
 * Result.Err(IoError::from_errno). The child reports execve failure
 * by exiting 127 with a diagnostic on inherited stderr, matching
 * `riven_process_run`'s contract.
 */
void *riven_command_status(RivenCommand *c) {
    if (!c) {
        return riven_io_error_message("command is null");
    }
    if (!c->program || !*c->program) {
        return riven_io_error_message("command program is empty");
    }
    /* Pre-flight existence check so a typo'd binary path surfaces as
     * Result::Err(IoError::NotFound) instead of Ok(ExitStatus(127)).
     * Without this, callers cannot distinguish "exec failed" from
     * "exec succeeded and the child happened to exit 127". The
     * `access(F_OK)` is a stat-only call — the actual permission /
     * exec failure path still gets caught by execve in the child and
     * surfaces as Ok(127) (the conventional shell encoding). */
    if (access(c->program, F_OK) != 0) {
        return riven_io_error_from_errno(errno);
    }

    char **argv = riven_command_build_argv(c);
    char **envp = riven_command_build_envp(c);
    if (!argv || !envp) {
        if (argv) free(argv);
        if (envp) free(envp);
        return riven_io_error_message("out of memory");
    }

    pid_t pid = fork();
    if (pid < 0) {
        int saved = errno;
        free(argv);
        free(envp);
        return riven_io_error_from_errno(saved);
    }

    if (pid == 0) {
        /* Child: chdir if requested, then execve. On any failure write
         * to inherited stderr and _exit(127). */
        if (c->cwd && chdir(c->cwd) != 0) {
            int saved = errno;
            fprintf(stderr,
                    "riven_command_status: chdir(\"%s\") failed: %s (errno=%d)\n",
                    c->cwd, strerror(saved), saved);
            _exit(127);
        }
        execve(c->program, argv, envp);
        int saved = errno;
        fprintf(stderr,
                "riven_command_status: execve(\"%s\") failed: %s (errno=%d)\n",
                c->program, strerror(saved), saved);
        _exit(127);
    }

    /* Parent: free argv/envp spines (entries belong to the Command),
     * wait. The Command itself stays live until scope-exit drop. */
    free(argv);
    free(envp);

    int wstatus = 0;
    while (waitpid(pid, &wstatus, 0) < 0) {
        if (errno == EINTR) continue;
        int saved = errno;
        return riven_io_error_from_errno(saved);
    }

    int64_t code = riven_command_pack_status(wstatus);
    RivenExitStatus *st = riven_exit_status_alloc(code);

    return riven_result_ok_value((int64_t)st);
}

/* Drain a pipe fd into a freshly-allocated, NUL-terminated buffer.
 * Returns the buffer (caller owns) on success; on read error returns
 * NULL with `*err_out` set to errno. Empty pipe yields a non-NULL
 * zero-length string. */
static char *riven_command_drain_fd(int fd, int *err_out) {
    size_t cap = 256;
    size_t len = 0;
    char *buf = (char *)malloc(cap);
    if (!buf) {
        *err_out = ENOMEM;
        return NULL;
    }
    for (;;) {
        if (len + 1 >= cap) {
            size_t next_cap = cap * 2;
            char *next = (char *)realloc(buf, next_cap);
            if (!next) {
                free(buf);
                *err_out = ENOMEM;
                return NULL;
            }
            buf = next;
            cap = next_cap;
        }
        ssize_t n = read(fd, buf + len, cap - len - 1);
        if (n > 0) {
            len += (size_t)n;
            continue;
        }
        if (n == 0) break;     /* EOF */
        if (errno == EINTR) continue;
        int saved = errno;
        free(buf);
        *err_out = saved;
        return NULL;
    }
    buf[len] = '\0';
    *err_out = 0;
    return buf;
}

/* `Command.output() -> Result[Output, IoError]` — like status, but
 * captures the child's stdout and stderr via pipes. Output is returned
 * as String (UTF-8 assumed; raw-bytes API deferred to v2).
 *
 * Borrow semantics same as `Command_status` — see the comment block
 * there. The Command stays live until the caller's scope-exit drop.
 */
void *riven_command_output(RivenCommand *c) {
    if (!c) {
        return riven_io_error_message("command is null");
    }
    if (!c->program || !*c->program) {
        return riven_io_error_message("command program is empty");
    }
    /* Pre-flight existence check — see `riven_command_status` for the
     * rationale. Typo'd binaries surface as Err(NotFound). */
    if (access(c->program, F_OK) != 0) {
        return riven_io_error_from_errno(errno);
    }

    int out_pipe[2] = {-1, -1};
    int err_pipe[2] = {-1, -1};
    if (pipe(out_pipe) != 0) {
        int saved = errno;
        return riven_io_error_from_errno(saved);
    }
    if (pipe(err_pipe) != 0) {
        int saved = errno;
        close(out_pipe[0]); close(out_pipe[1]);
        return riven_io_error_from_errno(saved);
    }

    char **argv = riven_command_build_argv(c);
    char **envp = riven_command_build_envp(c);
    if (!argv || !envp) {
        if (argv) free(argv);
        if (envp) free(envp);
        close(out_pipe[0]); close(out_pipe[1]);
        close(err_pipe[0]); close(err_pipe[1]);
        return riven_io_error_message("out of memory");
    }

    pid_t pid = fork();
    if (pid < 0) {
        int saved = errno;
        free(argv); free(envp);
        close(out_pipe[0]); close(out_pipe[1]);
        close(err_pipe[0]); close(err_pipe[1]);
        return riven_io_error_from_errno(saved);
    }

    if (pid == 0) {
        /* Child: rewire stdout/stderr onto the pipe write ends, close
         * the read ends, chdir, exec. */
        close(out_pipe[0]);
        close(err_pipe[0]);
        if (dup2(out_pipe[1], STDOUT_FILENO) < 0) _exit(127);
        if (dup2(err_pipe[1], STDERR_FILENO) < 0) _exit(127);
        close(out_pipe[1]);
        close(err_pipe[1]);
        if (c->cwd && chdir(c->cwd) != 0) {
            /* stderr is already piped — write goes back to parent. */
            int saved = errno;
            fprintf(stderr,
                    "riven_command_output: chdir(\"%s\") failed: %s (errno=%d)\n",
                    c->cwd, strerror(saved), saved);
            _exit(127);
        }
        execve(c->program, argv, envp);
        int saved = errno;
        fprintf(stderr,
                "riven_command_output: execve(\"%s\") failed: %s (errno=%d)\n",
                c->program, strerror(saved), saved);
        _exit(127);
    }

    /* Parent: close the write ends so EOF arrives when the child exits;
     * drain both pipes; wait. The naive sequential drain (stdout fully
     * then stderr fully) can deadlock if the child writes > pipe-buf
     * bytes to stderr while we're blocked reading stdout. For v1 we
     * accept that risk — typical CLI output is well under the 64 KiB
     * pipe buffer on Linux / 16 KiB on macOS, and the test surface
     * doesn't approach those limits. A non-blocking poll/select loop
     * is deferred to v2 alongside spawn/Child. */
    free(argv);
    free(envp);
    close(out_pipe[1]);
    close(err_pipe[1]);

    int err_n = 0;
    char *stdout_buf = riven_command_drain_fd(out_pipe[0], &err_n);
    close(out_pipe[0]);
    if (!stdout_buf) {
        close(err_pipe[0]);
        /* Reap to avoid zombie. */
        int junk = 0;
        while (waitpid(pid, &junk, 0) < 0 && errno == EINTR) {}
        return riven_io_error_from_errno(err_n);
    }
    char *stderr_buf = riven_command_drain_fd(err_pipe[0], &err_n);
    close(err_pipe[0]);
    if (!stderr_buf) {
        free(stdout_buf);
        int junk = 0;
        while (waitpid(pid, &junk, 0) < 0 && errno == EINTR) {}
        return riven_io_error_from_errno(err_n);
    }

    int wstatus = 0;
    while (waitpid(pid, &wstatus, 0) < 0) {
        if (errno == EINTR) continue;
        int saved = errno;
        free(stdout_buf);
        free(stderr_buf);
        return riven_io_error_from_errno(saved);
    }

    /* Re-wrap the raw read buffers into riven-managed Strings so the
     * Riven-side drop pass on the Output's stdout / stderr fields can
     * call `riven_string_free` uniformly. `riven_string_from` allocates
     * a fresh buffer and copies — we then free the read buffer. */
    char *stdout_riv = riven_string_from(stdout_buf);
    char *stderr_riv = riven_string_from(stderr_buf);
    free(stdout_buf);
    free(stderr_buf);

    RivenOutput *out = (RivenOutput *)riven_alloc(sizeof(RivenOutput));
    out->status = riven_exit_status_alloc(riven_command_pack_status(wstatus));
    out->stdout_buf = stdout_riv;
    out->stderr_buf = stderr_riv;

    return riven_result_ok_value((int64_t)out);
}

/* ── ExitStatus accessors ────────────────────────────────────────────── */

int64_t riven_exit_status_code(RivenExitStatus *st) {
    if (!st) return 127;
    return st->code;
}

int64_t riven_exit_status_success(RivenExitStatus *st) {
    if (!st) return 0;
    return st->code == 0 ? 1 : 0;
}

void riven_exit_status_free(RivenExitStatus *st) {
    if (st) riven_dealloc(st);
}

/* ── Output accessors ────────────────────────────────────────────────── */

/* `.stdout()` / `.stderr()` return a CLONE of the captured String so the
 * Output can be dropped (or another accessor called) without
 * invalidating the returned String. The cloned buffer is alloc-rooted
 * on the caller; the original stays with the Output until its own drop.
 */
char *riven_output_stdout(RivenOutput *o) {
    if (!o || !o->stdout_buf) return riven_string_from("");
    return riven_string_from(o->stdout_buf);
}

char *riven_output_stderr(RivenOutput *o) {
    if (!o || !o->stderr_buf) return riven_string_from("");
    return riven_string_from(o->stderr_buf);
}

/* `.status()` returns a FRESH ExitStatus copy so the caller can drop
 * the Output without dangling pointers. */
RivenExitStatus *riven_output_status(RivenOutput *o) {
    if (!o) return riven_exit_status_alloc(127);
    int64_t code = o->status ? o->status->code : 127;
    return riven_exit_status_alloc(code);
}

/* Drop helper for Output — frees the sub-status, the two String
 * buffers, then the spine. Mirrors `riven_command_drop`. */
void riven_output_drop(RivenOutput *o) {
    if (!o) return;
    if (o->status) {
        riven_dealloc(o->status);
        o->status = NULL;
    }
    if (o->stdout_buf) {
        free(o->stdout_buf);
        o->stdout_buf = NULL;
    }
    if (o->stderr_buf) {
        free(o->stderr_buf);
        o->stderr_buf = NULL;
    }
}

/* ── Phase 2 stdlib (#06.5 T2): File / OpenOptions / SeekFrom ─────────
 *
 * The `File` class is a thin owning wrapper over a POSIX fd. Surface:
 *
 *     File.open(path)         -> Result[File, IoError]   (O_RDONLY)
 *     File.create(path)       -> Result[File, IoError]   (O_WRONLY|O_CREAT|O_TRUNC, 0644)
 *     File.append(path)       -> Result[File, IoError]   (O_WRONLY|O_CREAT|O_APPEND, 0644)
 *     File.open_options(p, o) -> Result[File, IoError]
 *
 *     f.read(buf)             -> Result[Int, IoError]    (bytes read; 0 = EOF)
 *     f.read_to_string()      -> Result[String, IoError]
 *     f.read_all()            -> Result[Array[U8], IoError]
 *     f.write(bytes)          -> Result[Int, IoError]    (bytes written)
 *     f.write_all(bytes)      -> Result[(), IoError]     (loops on partial)
 *     f.write_str(s)          -> Result[(), IoError]
 *     f.flush()               -> Result[(), IoError]     (raw File: no-op)
 *     f.seek(pos)             -> Result[Int, IoError]    (new file position)
 *     f.metadata()            -> Result[Metadata, IoError]  (fstat)
 *     f.close()               -> Result[(), IoError]     (also runs on drop)
 *
 * `OpenOptions` is the builder companion for `File.open_options`. The
 * builder methods mutate-in-place and return the same pointer (mirrors
 * `Command.arg/.args/...`). One terminal call (`File.open_options(p, o)`)
 * consumes the configuration; the OpenOptions value itself stays alive
 * for any further use and is freed by the scope-exit drop pipeline via
 * the generic `riven_dealloc` (no inner heap — 8-byte POD).
 *
 * `SeekFrom` is a tagged enum with three struct-variants, each carrying
 * a single `offset: Int`. The codegen lays every Riven enum out as a
 * 16-byte boxed pointer with `{i32 tag; i32 pad; i64 payload}` — for
 * a single-field struct variant the payload slot IS the field. So
 * `riven_file_seek` reads tag at +0 and offset at +8 directly.
 *
 *     SeekFrom.Start(n)   -> tag 0,  whence SEEK_SET
 *     SeekFrom.End(n)     -> tag 1,  whence SEEK_END
 *     SeekFrom.Current(n) -> tag 2,  whence SEEK_CUR
 *
 * Drop semantics: `File` participates in the user_drop_classes pipeline
 * (see mir/lower/collect.rs::collect_user_drop_classes). At scope exit
 * the MIR emits `File_drop(f) + riven_dealloc(f)`. `riven_file_drop`
 * closes the fd iff `closed == 0` and then returns; the dealloc that
 * follows releases the 8-byte spine.
 *
 * Wire layouts:
 *
 *   RivenFile  (8 bytes)
 *     +0   int32  fd       — open file descriptor (or -1 once closed)
 *     +4   int32  closed   — 1 once `close` has been called; idempotent
 *
 *   RivenOpenOptions  (8 bytes)
 *     +0   u8     read
 *     +1   u8     write
 *     +2   u8     append
 *     +3   u8     truncate
 *     +4   u8     create
 *     +5   u8     create_new       (maps to O_EXCL alongside O_CREAT)
 *     +6   u8     _pad[2]          (zeroed; reserved)
 *     +7   u8
 */

#define RIVEN_SEEK_FROM_START   0
#define RIVEN_SEEK_FROM_END     1
#define RIVEN_SEEK_FROM_CURRENT 2

typedef struct {
    int32_t fd;
    int32_t closed;
} RivenFile;

_Static_assert(sizeof(RivenFile) == 8,
    "RivenFile wire layout drifted from documented 8-byte form");

typedef struct {
    uint8_t read;
    uint8_t write;
    uint8_t append;
    uint8_t truncate;
    uint8_t create;
    uint8_t create_new;
    uint8_t _pad[2];
} RivenOpenOptions;

_Static_assert(sizeof(RivenOpenOptions) == 8,
    "RivenOpenOptions wire layout drifted from documented 8-byte form");

/* Wrap an existing fd in a Result::Ok(File). */
static void *riven_file_wrap_ok(int fd) {
    RivenFile *f = (RivenFile *)riven_alloc(sizeof(RivenFile));
    f->fd = fd;
    f->closed = 0;
    return riven_result_ok_value((int64_t)f);
}

/* Build a Result::Err(IoError::InvalidInput(<msg>)) for the static
 * detection paths (E0711/E0712). The runtime InvalidInput variant has
 * no payload in the current 8-tag layout — message routing for unit
 * variants happens via `IoError.message()` returning the canonical
 * static string. We therefore use the runtime helper that allocates a
 * unit-variant value with the canonical message. */
static void *riven_file_invalid_input(void) {
    return riven_result_err_value(
        (int64_t)riven_io_error_unit(RIVEN_IO_ERROR_INVALID_INPUT));
}

void *riven_file_open(const char *path) {
    if (!path) return riven_file_invalid_input();
    int fd = open(path, O_RDONLY);
    if (fd < 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_file_wrap_ok(fd);
}

void *riven_file_create(const char *path) {
    if (!path) return riven_file_invalid_input();
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fd < 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_file_wrap_ok(fd);
}

void *riven_file_append(const char *path) {
    if (!path) return riven_file_invalid_input();
    int fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0644);
    if (fd < 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_file_wrap_ok(fd);
}

/* Derive `open(2)` flags from an OpenOptions value. E0711:
 * `OpenOptions requires at least one of read/write/append`. We surface
 * this as Result::Err(IoError::InvalidInput) at the runtime layer —
 * static detection is deferred (the OpenOptions builder is a value-
 * level chain so a single-AST-pass static analysis cannot in general
 * see the final flag set).
 *
 * Flag matrix (mirrors Rust's std::fs::OpenOptions):
 *   read=1, write=0, append=0           -> O_RDONLY
 *   read=0, write=1, append=0           -> O_WRONLY
 *   read=1, write=1, append=0           -> O_RDWR
 *   read=0/1, write=*, append=1         -> O_WRONLY|O_APPEND (or O_RDWR|O_APPEND)
 *   truncate=1 (only with write)        -> O_TRUNC
 *   create=1                            -> O_CREAT (mode 0644)
 *   create_new=1                        -> O_CREAT|O_EXCL (overrides create)
 */
void *riven_file_open_options(const char *path, RivenOpenOptions *opts) {
    if (!path || !opts) return riven_file_invalid_input();
    if (!opts->read && !opts->write && !opts->append) {
        /* E0711 at runtime — see comment above for the deferral note. */
        return riven_file_invalid_input();
    }
    int flags = 0;
    if (opts->read && (opts->write || opts->append)) {
        flags |= O_RDWR;
    } else if (opts->write || opts->append) {
        flags |= O_WRONLY;
    } else {
        flags |= O_RDONLY;
    }
    if (opts->append) flags |= O_APPEND;
    if (opts->truncate && (opts->write || opts->append)) flags |= O_TRUNC;
    if (opts->create_new) {
        flags |= O_CREAT | O_EXCL;
    } else if (opts->create) {
        flags |= O_CREAT;
    }
    int fd;
    if (flags & O_CREAT) {
        fd = open(path, flags, 0644);
    } else {
        fd = open(path, flags);
    }
    if (fd < 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_file_wrap_ok(fd);
}

/* `File.read(buf: &mut Array[U8]) -> Result[Int, IoError]`.
 *
 * v1 simplification: buf is a `RivenVec` whose element slots are
 * int64_t (the uniform Vec representation). We read into a stack
 * staging buffer of `min(remaining_cap, 4096)` bytes and push each
 * byte as a single int64 slot. This keeps the wire-level contract
 * identical to other byte-Array surfaces (process Output.stdout
 * materializes as a String, not a Vec[U8] today — that bridge lands
 * with the BufReader work in T6).
 *
 * Returns Ok(0) on EOF, Ok(n) on n>0 bytes, Err on real failure.
 * Retries on EINTR. */
void *riven_file_read(RivenFile *f, RivenVec *buf) {
    if (!f || f->closed || !buf) return riven_file_invalid_input();
    unsigned char stage[4096];
    ssize_t got;
    do {
        got = read(f->fd, stage, sizeof(stage));
    } while (got < 0 && errno == EINTR);
    if (got < 0) {
        return riven_io_error_from_errno(errno);
    }
    for (ssize_t i = 0; i < got; i++) {
        riven_vec_push(buf, (int64_t)stage[i]);
    }
    return riven_result_ok_value(got);
}

void *riven_file_read_to_string(RivenFile *f) {
    if (!f || f->closed) return riven_file_invalid_input();
    size_t cap = 256;
    size_t len = 0;
    char *out = (char *)malloc(cap);
    if (!out) riven_panic("out of memory");
    for (;;) {
        if (len + 1 >= cap) {
            size_t next_cap = cap * 2;
            char *next = (char *)realloc(out, next_cap);
            if (!next) { free(out); riven_panic("out of memory"); }
            out = next;
            cap = next_cap;
        }
        ssize_t got;
        do {
            got = read(f->fd, out + len, cap - 1 - len);
        } while (got < 0 && errno == EINTR);
        if (got < 0) {
            int saved = errno;
            free(out);
            return riven_io_error_from_errno(saved);
        }
        if (got == 0) break;
        len += (size_t)got;
    }
    out[len] = '\0';
    /* `riven_string_from` performs the canonical copy into the String
     * pool; free our staging buffer afterwards. */
    char *s = riven_string_from(out);
    free(out);
    return riven_result_ok_value((int64_t)s);
}

void *riven_file_read_all(RivenFile *f) {
    if (!f || f->closed) return riven_file_invalid_input();
    RivenVec *v = riven_vec_new();
    unsigned char stage[4096];
    for (;;) {
        ssize_t got;
        do {
            got = read(f->fd, stage, sizeof(stage));
        } while (got < 0 && errno == EINTR);
        if (got < 0) {
            int saved = errno;
            /* Caller will not see the partial Vec on error; free it. */
            riven_dealloc(v);
            return riven_io_error_from_errno(saved);
        }
        if (got == 0) break;
        for (ssize_t i = 0; i < got; i++) {
            riven_vec_push(v, (int64_t)stage[i]);
        }
    }
    return riven_result_ok_value((int64_t)v);
}

/* `File.write(bytes: &Array[U8]) -> Result[Int, IoError]`. Single
 * `write(2)` call. Partial writes are possible and surface as
 * Ok(n) where n < bytes.len; the caller chooses how to retry. Use
 * `write_all` if a loop-until-complete contract is wanted. */
void *riven_file_write(RivenFile *f, RivenVec *bytes) {
    if (!f || f->closed || !bytes) return riven_file_invalid_input();
    /* Stage the int64-slot bytes into a tight buffer for `write(2)`. */
    size_t n = (size_t)bytes->len;
    unsigned char *stage = (unsigned char *)malloc(n > 0 ? n : 1);
    if (!stage) riven_panic("out of memory");
    for (size_t i = 0; i < n; i++) {
        stage[i] = (unsigned char)(bytes->data[i] & 0xFF);
    }
    ssize_t put;
    do {
        put = write(f->fd, stage, n);
    } while (put < 0 && errno == EINTR);
    if (put < 0) {
        int saved = errno;
        free(stage);
        return riven_io_error_from_errno(saved);
    }
    free(stage);
    return riven_result_ok_value((int64_t)put);
}

void *riven_file_write_all(RivenFile *f, RivenVec *bytes) {
    if (!f || f->closed || !bytes) return riven_file_invalid_input();
    size_t n = (size_t)bytes->len;
    unsigned char *stage = (unsigned char *)malloc(n > 0 ? n : 1);
    if (!stage) riven_panic("out of memory");
    for (size_t i = 0; i < n; i++) {
        stage[i] = (unsigned char)(bytes->data[i] & 0xFF);
    }
    size_t off = 0;
    while (off < n) {
        ssize_t put;
        do {
            put = write(f->fd, stage + off, n - off);
        } while (put < 0 && errno == EINTR);
        if (put < 0) {
            int saved = errno;
            free(stage);
            return riven_io_error_from_errno(saved);
        }
        if (put == 0) {
            /* WriteZero per std::io contract: write returned 0 with
             * bytes remaining and no signaled error. */
            free(stage);
            return riven_result_err_value(
                (int64_t)riven_io_error_struct(
                    RIVEN_IO_ERROR_WRITE_ZERO,
                    "write returned 0 with bytes remaining"));
        }
        off += (size_t)put;
    }
    free(stage);
    return riven_result_ok_value(0);
}

void *riven_file_write_str(RivenFile *f, const char *s) {
    if (!f || f->closed) return riven_file_invalid_input();
    if (!s) s = "";
    size_t n = strlen(s);
    size_t off = 0;
    while (off < n) {
        ssize_t put;
        do {
            put = write(f->fd, s + off, n - off);
        } while (put < 0 && errno == EINTR);
        if (put < 0) {
            return riven_io_error_from_errno(errno);
        }
        if (put == 0) {
            return riven_result_err_value(
                (int64_t)riven_io_error_struct(
                    RIVEN_IO_ERROR_WRITE_ZERO,
                    "write returned 0 with bytes remaining"));
        }
        off += (size_t)put;
    }
    return riven_result_ok_value(0);
}

/* `File.flush()` — POSIX `write(2)` has no userspace buffer to flush.
 * BufWriter (T6) overrides this with the buffer-emit path. For raw
 * File the contract is just Ok(()). We deliberately do NOT fsync()
 * here — that would be a much heavier semantic and is what
 * `fs.write_atomic` covers (durability) when added in T3. */
void *riven_file_flush(RivenFile *f) {
    if (!f || f->closed) return riven_file_invalid_input();
    return riven_result_ok_value(0);
}

/* `File.seek(pos: SeekFrom) -> Result[Int, IoError]`. `pos` is a
 * 16-byte tagged enum value (see SeekFrom comment block at the top of
 * this section). E0712 is "negative offset on SeekFrom::Start" — a
 * file position before byte 0 is meaningless. We surface it as
 * Result::Err(IoError::InvalidInput) at runtime, matching the prompt's
 * "runtime check ok if not statically detectable" fallback. */
void *riven_file_seek(RivenFile *f, void *pos) {
    if (!f || f->closed || !pos) return riven_file_invalid_input();
    int32_t tag = *(int32_t *)pos;
    int64_t offset = ((int64_t *)pos)[1];
    int whence;
    switch (tag) {
        case RIVEN_SEEK_FROM_START:
            if (offset < 0) {
                /* E0712: negative offset on Start. */
                return riven_file_invalid_input();
            }
            whence = SEEK_SET;
            break;
        case RIVEN_SEEK_FROM_END:
            whence = SEEK_END;
            break;
        case RIVEN_SEEK_FROM_CURRENT:
            whence = SEEK_CUR;
            break;
        default:
            return riven_file_invalid_input();
    }
    off_t pos_new = lseek(f->fd, (off_t)offset, whence);
    if (pos_new == (off_t)-1) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value((int64_t)pos_new);
}

/* `File.metadata() -> Result[Metadata, IoError]`. Reuses the
 * `RivenMetadata` wire format produced by `riven_fs_metadata` — 3
 * int64s {size, modified_secs, kind}. fstat(2) so we report the
 * underlying file's identity, not whatever path created the fd. */
void *riven_file_metadata(RivenFile *f) {
    if (!f || f->closed) return riven_file_invalid_input();
    struct stat st;
    if (fstat(f->fd, &st) != 0) {
        return riven_io_error_from_errno(errno);
    }
    int64_t *meta = (int64_t *)riven_alloc(24);
    meta[0] = (int64_t)st.st_size;
    meta[1] = (int64_t)st.st_mtime;
    int64_t kind;
    if (S_ISREG(st.st_mode)) {
        kind = RIVEN_METADATA_KIND_FILE;
    } else if (S_ISDIR(st.st_mode)) {
        kind = RIVEN_METADATA_KIND_DIR;
    } else if (S_ISLNK(st.st_mode)) {
        kind = RIVEN_METADATA_KIND_SYMLINK;
    } else {
        kind = RIVEN_METADATA_KIND_OTHER;
    }
    meta[2] = kind;
    return riven_result_ok_value((int64_t)meta);
}

/* `File.close()` — idempotent. Returns Ok(()) on first successful
 * close, Ok(()) on subsequent calls (no-op), Err on close(2) failure
 * (rare: EBADF, EIO). */
void *riven_file_close(RivenFile *f) {
    if (!f) return riven_file_invalid_input();
    if (f->closed) return riven_result_ok_value(0);
    int rc;
    do {
        rc = close(f->fd);
    } while (rc < 0 && errno == EINTR);
    f->closed = 1;
    f->fd = -1;
    if (rc < 0) {
        return riven_io_error_from_errno(errno);
    }
    return riven_result_ok_value(0);
}

/* Drop helper for File — closes the fd if still open. Registered in
 * `mir/lower/collect.rs::collect_user_drop_classes` so the MIR emits
 * `File_drop(f) + riven_dealloc(f)` at scope exit. The dealloc that
 * follows releases the 8-byte spine. */
void riven_file_drop(RivenFile *f) {
    if (!f) return;
    if (!f->closed && f->fd >= 0) {
        int rc;
        do {
            rc = close(f->fd);
        } while (rc < 0 && errno == EINTR);
        (void)rc; /* drop swallows errors — there's nobody to surface them to. */
        f->closed = 1;
        f->fd = -1;
    }
}

/* ── OpenOptions builder ──────────────────────────────────────────── */

RivenOpenOptions *riven_open_options_new(void) {
    RivenOpenOptions *o = (RivenOpenOptions *)riven_alloc(sizeof(RivenOpenOptions));
    o->read = 0;
    o->write = 0;
    o->append = 0;
    o->truncate = 0;
    o->create = 0;
    o->create_new = 0;
    o->_pad[0] = 0;
    o->_pad[1] = 0;
    return o;
}

RivenOpenOptions *riven_open_options_read(RivenOpenOptions *o, int64_t v) {
    if (!o) return o;
    o->read = v ? 1 : 0;
    return o;
}

RivenOpenOptions *riven_open_options_write(RivenOpenOptions *o, int64_t v) {
    if (!o) return o;
    o->write = v ? 1 : 0;
    return o;
}

RivenOpenOptions *riven_open_options_append(RivenOpenOptions *o, int64_t v) {
    if (!o) return o;
    o->append = v ? 1 : 0;
    return o;
}

RivenOpenOptions *riven_open_options_truncate(RivenOpenOptions *o, int64_t v) {
    if (!o) return o;
    o->truncate = v ? 1 : 0;
    return o;
}

RivenOpenOptions *riven_open_options_create(RivenOpenOptions *o, int64_t v) {
    if (!o) return o;
    o->create = v ? 1 : 0;
    return o;
}

RivenOpenOptions *riven_open_options_create_new(RivenOpenOptions *o, int64_t v) {
    if (!o) return o;
    o->create_new = v ? 1 : 0;
    return o;
}

void riven_print_int(int64_t n) {
    printf("%" PRId64 "\n", n);
}

void riven_print_float(double f) {
    printf("%g\n", f);
}

/* ── To-String Conversions ─────────────────────────────────────────── */

char *riven_int_to_string(int64_t n) {
    char buf[32];
    snprintf(buf, sizeof(buf), "%" PRId64, n);
    size_t len = strlen(buf);
    char *result = (char *)malloc(len + 1);
    if (!result) {
        riven_panic("out of memory");
    }
    memcpy(result, buf, len + 1);
    return result;
}

char *riven_float_to_string(double f) {
    char buf[64];
    snprintf(buf, sizeof(buf), "%g", f);
    size_t len = strlen(buf);
    char *result = (char *)malloc(len + 1);
    if (!result) {
        riven_panic("out of memory");
    }
    memcpy(result, buf, len + 1);
    return result;
}

/* Phase 2 #06.D4: format a float with an explicit decimal precision.
 * `prec < 0` falls back to `%g` (matches `riven_float_to_string`).
 * `prec >= 0` uses `%.<prec>f`. */
char *riven_float_to_string_prec(double f, int64_t prec) {
    if (prec < 0) return riven_float_to_string(f);
    if (prec > 60) prec = 60; /* snprintf safety bound */
    char buf[96];
    snprintf(buf, sizeof(buf), "%.*f", (int)prec, f);
    size_t len = strlen(buf);
    char *result = (char *)malloc(len + 1);
    if (!result) {
        riven_panic("out of memory");
    }
    memcpy(result, buf, len + 1);
    return result;
}

/* Convert a Unicode codepoint (passed widened to i64) into a heap-allocated
   UTF-8 string. Used for `"#{c}"` interpolation on values of type `Char`. */
char *riven_char_to_string(int64_t codepoint) {
    uint32_t cp = (uint32_t)codepoint;
    char buf[5];
    size_t len;
    if (cp < 0x80) {
        buf[0] = (char)cp;
        len = 1;
    } else if (cp < 0x800) {
        buf[0] = (char)(0xC0 | (cp >> 6));
        buf[1] = (char)(0x80 | (cp & 0x3F));
        len = 2;
    } else if (cp < 0x10000) {
        buf[0] = (char)(0xE0 | (cp >> 12));
        buf[1] = (char)(0x80 | ((cp >> 6) & 0x3F));
        buf[2] = (char)(0x80 | (cp & 0x3F));
        len = 3;
    } else {
        buf[0] = (char)(0xF0 | (cp >> 18));
        buf[1] = (char)(0x80 | ((cp >> 12) & 0x3F));
        buf[2] = (char)(0x80 | ((cp >> 6) & 0x3F));
        buf[3] = (char)(0x80 | (cp & 0x3F));
        len = 4;
    }
    char *result = (char *)malloc(len + 1);
    if (!result) {
        riven_panic("out of memory");
    }
    memcpy(result, buf, len);
    result[len] = '\0';
    return result;
}

char *riven_bool_to_string(int64_t b) {
    const char *s = b ? "true" : "false";
    size_t len = strlen(s);
    char *result = (char *)malloc(len + 1);
    if (!result) {
        riven_panic("out of memory");
    }
    memcpy(result, s, len + 1);
    return result;
}

/* ── String Operations ─────────────────────────────────────────────── */

/* ── String Comparison ─────────────────────────────────────────── */

int64_t riven_string_eq(const char *a, const char *b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    return strcmp(a, b) == 0 ? 1 : 0;
}

int64_t riven_string_cmp(const char *a, const char *b) {
    if (!a && !b) return 0;
    if (!a) return -1;
    if (!b) return 1;
    return (int64_t)strcmp(a, b);
}

int64_t riven_string_hash(const char *s) {
    return (int64_t)riven_hash_str(s);
}

void riven_thread_sleep_ns(int64_t ns) {
    if (ns <= 0) {
        return;
    }

    struct timespec req;
    req.tv_sec = (time_t)(ns / 1000000000LL);
    req.tv_nsec = (long)(ns % 1000000000LL);

    while (nanosleep(&req, &req) == -1 && errno == EINTR) {
    }
}

void riven_thread_yield(void) {
    sched_yield();
}

/* ---------------------------------------------------------------------
 * Signal handling — minimal cooperative graceful-shutdown surface.
 *
 * The v1 model is intentionally simple: install a SIGINT handler that
 * flips a flag, then let the program poll that flag from its main
 * loop and shut down cleanly when set.  No SA_RESTART, so a blocking
 * accept() / read() / write() that's interrupted by SIGINT returns
 * with errno=EINTR — the loop sees the flag on its next iteration and
 * exits.  No re-entrant handler, no multi-signal coverage, no signal
 * masks — server scripts get the basic shape and v2 can layer SIGTERM
 * / SIGHUP / per-signal handlers on top.
 * ------------------------------------------------------------------ */

static volatile sig_atomic_t riven_sigint_received = 0;

static void riven_sigint_handler(int signo) {
    (void)signo;
    riven_sigint_received = 1;
}

void riven_signal_install_sigint(void) {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = riven_sigint_handler;
    sigemptyset(&sa.sa_mask);
    /* Intentionally no SA_RESTART: we want blocking syscalls to
     * return EINTR so cooperative loops can notice the flag. */
    sa.sa_flags = 0;
    sigaction(SIGINT, &sa, NULL);
}

int64_t riven_signal_received_sigint(void) {
    return riven_sigint_received ? 1 : 0;
}

/* ---------------------------------------------------------------------
 * std::time
 *
 * Two clock sources, both returned as nanoseconds in an int64_t.
 *
 *   - `riven_time_now_ns`  → monotonic clock, suitable for measuring
 *     elapsed time. Never moves backwards. Anchor is unspecified — only
 *     differences are meaningful.
 *
 *   - `riven_time_unix_ns` → realtime clock, nanoseconds since the Unix
 *     epoch (1970-01-01 UTC). Subject to NTP adjustments and manual
 *     clock changes; do not use for measuring elapsed time.
 *
 * Both clamp to 0 on clock_gettime failure (which is effectively never
 * on Linux/macOS but the contract has to say something). Returning 0
 * keeps the type signature simple — no Result wrapping for a syscall
 * that has no failure mode in practice.
 * --------------------------------------------------------------------- */
int64_t riven_time_now_ns(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
        return 0;
    }
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}

int64_t riven_time_unix_ns(void) {
    struct timespec ts;
    if (clock_gettime(CLOCK_REALTIME, &ts) != 0) {
        return 0;
    }
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}

/* ---------------------------------------------------------------------
 * std::path
 *
 * Linux/Unix-style path manipulation. Windows backslash is a non-goal
 * for v1 (see docs/requirements/tier4_02_cross_compilation.md).
 *
 * For lookups that may fail (parent, file_name, extension), v1 returns
 * an empty string as the absent-value sentinel rather than wiring a
 * tagged Option[String] return. Callers can branch on `.is_empty()`.
 * Promoting to Option[String] is a follow-up once a runtime-side
 * Option-of-heap helper exists (the current `riven_option_*` family
 * only handles primitive payloads).
 * --------------------------------------------------------------------- */
char *riven_path_join(const char *a, const char *b) {
    const char *base = a ? a : "";
    const char *tail = b ? b : "";

    /* If the second component is absolute it overrides the first. */
    if (tail[0] == '/') {
        return riven_string_from(tail);
    }

    size_t la = strlen(base);
    size_t lb = strlen(tail);

    /* Empty base → just clone tail; empty tail → just clone base. */
    if (la == 0) return riven_string_from(tail);
    if (lb == 0) return riven_string_from(base);

    int needsep = base[la - 1] != '/';
    size_t total = la + (needsep ? 1 : 0) + lb;
    char *out = (char *)malloc(total + 1);
    if (!out) return riven_string_from("");
    memcpy(out, base, la);
    size_t i = la;
    if (needsep) {
        out[i++] = '/';
    }
    memcpy(out + i, tail, lb);
    out[total] = '\0';
    return out;
}

char *riven_path_parent(const char *p) {
    if (!p || !*p) return riven_string_from("");
    size_t n = strlen(p);

    /* Strip trailing slashes (but keep leading "/" itself). */
    while (n > 1 && p[n - 1] == '/') {
        n--;
    }

    /* Find the last separator in the trimmed range. */
    ssize_t i = (ssize_t)n - 1;
    while (i >= 0 && p[i] != '/') {
        i--;
    }

    if (i < 0) {
        /* No separator → no parent. */
        return riven_string_from("");
    }
    if (i == 0) {
        /* Parent is root. */
        return riven_string_from("/");
    }

    char *out = (char *)malloc((size_t)i + 1);
    if (!out) return riven_string_from("");
    memcpy(out, p, (size_t)i);
    out[i] = '\0';
    return out;
}

char *riven_path_file_name(const char *p) {
    if (!p || !*p) return riven_string_from("");
    size_t n = strlen(p);

    /* Trailing slashes mean the path designates a directory whose name
     * we still want — `/foo/bar/` → `bar`. Strip them first. */
    while (n > 0 && p[n - 1] == '/') {
        n--;
    }
    if (n == 0) return riven_string_from("");

    ssize_t start = (ssize_t)n - 1;
    while (start >= 0 && p[start] != '/') {
        start--;
    }
    start++; /* point at first char of file name */

    size_t len = n - (size_t)start;
    char *out = (char *)malloc(len + 1);
    if (!out) return riven_string_from("");
    memcpy(out, p + start, len);
    out[len] = '\0';
    return out;
}

char *riven_path_extension(const char *p) {
    char *name = riven_path_file_name(p);
    if (!name || !*name) {
        return name ? name : riven_string_from("");
    }

    size_t n = strlen(name);
    /* Find last '.'. Leading dot is NOT an extension ("/foo/.hidden" has
     * no extension), matching Rust's std::path::Path::extension. */
    ssize_t dot = (ssize_t)n - 1;
    while (dot > 0 && name[dot] != '.') {
        dot--;
    }

    if (dot <= 0 || name[dot] != '.') {
        free(name);
        return riven_string_from("");
    }

    size_t ext_len = n - (size_t)dot - 1;
    char *out = (char *)malloc(ext_len + 1);
    if (!out) {
        free(name);
        return riven_string_from("");
    }
    memcpy(out, name + dot + 1, ext_len);
    out[ext_len] = '\0';
    free(name);
    return out;
}

int64_t riven_path_is_absolute(const char *p) {
    return (p && p[0] == '/') ? 1 : 0;
}

/* ── std::net (Phase 3) ────────────────────────────────────────────────
 *
 * Minimum-viable TCP API exposed as fd-based free functions. The Riven
 * surface is intentionally raw — fds are returned as `Int`, with -1
 * signalling failure (mirroring POSIX). Class wrappers
 * (TcpStream/TcpListener) are a follow-up.
 *
 * Address parsing: we split the input on the *last* ':' so IPv6 literals
 * with embedded colons can be handled later (v1: IPv4/hostname only,
 * still uses last-colon split which is correct for `host:port`).
 *
 * SIGPIPE suppression: on Linux we pass `MSG_NOSIGNAL` to `send()` so a
 * write to a peer that closed its end raises `EPIPE` rather than killing
 * us with SIGPIPE. macOS/BSD lack `MSG_NOSIGNAL` (collapsed to 0 above)
 * but have the per-socket `SO_NOSIGPIPE` option, which we set via
 * `riven_tcp_set_nosigpipe()` after every `socket()` / `accept()`.
 *
 * v1 binary-safety caveat: `tcp_read` returns a NUL-terminated Riven
 * String. The buffer is malloc'd at `max+1` and the byte after the last
 * received byte is set to 0. If the *received* bytes contain embedded
 * NULs, callers will observe truncation at the first NUL when treating
 * the result as a C string. Full binary safety requires a `Bytes`
 * type — tracked as a follow-up.
 */

/* Suppress SIGPIPE on this socket when the peer goes away. On Linux
 * we rely on MSG_NOSIGNAL at send() time, so this is a no-op there.
 * On macOS / *BSD MSG_NOSIGNAL doesn't exist (defined to 0 above), so
 * SO_NOSIGPIPE on the socket is what actually keeps a write-after-close
 * from killing us with SIGPIPE. */
static void riven_tcp_set_nosigpipe(int fd) {
#ifdef SO_NOSIGPIPE
    int one = 1;
    (void)setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, sizeof one);
#else
    (void)fd;
#endif
}

/* Internal: split "host:port" on the last colon. Returns 0 on success,
 * -1 on malformed input. `host` may end up empty (means INADDR_ANY for
 * listen, or fail-loudly for connect via getaddrinfo). */
static int riven_tcp_split_addr(const char *addr, char *host, size_t host_cap,
                                char *port, size_t port_cap) {
    if (!addr) return -1;
    const char *colon = strrchr(addr, ':');
    if (!colon) return -1;
    size_t host_len = (size_t)(colon - addr);
    size_t port_len = strlen(colon + 1);
    if (host_len + 1 > host_cap || port_len + 1 > port_cap) return -1;
    memcpy(host, addr, host_len);
    host[host_len] = '\0';
    memcpy(port, colon + 1, port_len + 1);
    return 0;
}

int64_t riven_tcp_connect(const char *addr) {
    char host[256];
    char port[16];
    if (riven_tcp_split_addr(addr, host, sizeof host, port, sizeof port) != 0) {
        return -1;
    }

    struct addrinfo hints;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;

    struct addrinfo *res = NULL;
    const char *node = (host[0] == '\0') ? "127.0.0.1" : host;
    if (getaddrinfo(node, port, &hints, &res) != 0 || !res) {
        return -1;
    }

    int fd = -1;
    for (struct addrinfo *ai = res; ai; ai = ai->ai_next) {
        fd = socket(ai->ai_family, ai->ai_socktype, ai->ai_protocol);
        if (fd < 0) continue;
        riven_tcp_set_nosigpipe(fd);
        if (connect(fd, ai->ai_addr, ai->ai_addrlen) == 0) {
            break;
        }
        close(fd);
        fd = -1;
    }

    freeaddrinfo(res);
    return (int64_t)fd;
}

int64_t riven_tcp_listen(const char *addr) {
    char host[256];
    char port[16];
    if (riven_tcp_split_addr(addr, host, sizeof host, port, sizeof port) != 0) {
        return -1;
    }

    int port_num = atoi(port);
    if (port_num < 0 || port_num > 65535) {
        return -1;
    }

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) return -1;
    riven_tcp_set_nosigpipe(fd);

    int one = 1;
    /* SO_REUSEADDR so test runs can re-bind without TIME_WAIT delay. */
    (void)setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof one);

    struct sockaddr_in sa;
    memset(&sa, 0, sizeof sa);
    sa.sin_family = AF_INET;
    sa.sin_port = htons((uint16_t)port_num);

    if (host[0] == '\0' || strcmp(host, "0.0.0.0") == 0) {
        sa.sin_addr.s_addr = htonl(INADDR_ANY);
    } else if (inet_pton(AF_INET, host, &sa.sin_addr) != 1) {
        /* Fall back to getaddrinfo for hostnames like "localhost". */
        struct addrinfo hints, *res = NULL;
        memset(&hints, 0, sizeof hints);
        hints.ai_family = AF_INET;
        hints.ai_socktype = SOCK_STREAM;
        if (getaddrinfo(host, NULL, &hints, &res) != 0 || !res) {
            close(fd);
            return -1;
        }
        sa.sin_addr = ((struct sockaddr_in *)res->ai_addr)->sin_addr;
        freeaddrinfo(res);
    }

    if (bind(fd, (struct sockaddr *)&sa, sizeof sa) != 0) {
        close(fd);
        return -1;
    }
    if (listen(fd, 128) != 0) {
        close(fd);
        return -1;
    }
    return (int64_t)fd;
}

int64_t riven_tcp_accept(int64_t fd) {
    if (fd < 0) return -1;
    /* EINTR is propagated to the caller as `-1` rather than
     * auto-retried internally — this lets cooperative shutdown
     * loops notice a SIGINT and break out of their accept loop.
     * Callers that want auto-retry can wrap with `while fd < 0
     * { ... }` in Riven and check `signal_received_sigint()` on
     * each iteration. */
    int accepted = accept((int)fd, NULL, NULL);
    if (accepted >= 0) {
        riven_tcp_set_nosigpipe(accepted);
    }
    return (int64_t)accepted;
}

/* Reads up to `max` bytes; returns a malloc'd NUL-terminated Riven
 * String. Empty string on EOF / error. See module-level comment about
 * v1 binary-safety: embedded NULs in the received bytes will look like
 * a short read when the buffer is treated as a C string. */
char *riven_tcp_read(int64_t fd, int64_t max) {
    if (fd < 0 || max <= 0) {
        return riven_string_from("");
    }
    size_t cap = (size_t)max;
    char *buf = (char *)malloc(cap + 1);
    if (!buf) {
        riven_panic("out of memory");
    }
    ssize_t n;
    do {
        n = recv((int)fd, buf, cap, 0);
    } while (n < 0 && errno == EINTR);

    if (n <= 0) {
        free(buf);
        return riven_string_from("");
    }
    buf[n] = '\0';
    return buf;
}

int64_t riven_tcp_write(int64_t fd, const char *data) {
    if (fd < 0 || !data) return -1;
    size_t total = strlen(data);
    size_t sent = 0;
    while (sent < total) {
        ssize_t n = send((int)fd, data + sent, total - sent, MSG_NOSIGNAL);
        if (n < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        sent += (size_t)n;
    }
    return (int64_t)sent;
}

void riven_tcp_close(int64_t fd) {
    if (fd < 0) return;
    /* Best-effort close — ignore EINTR/EBADF. */
    (void)close((int)fd);
}

char *riven_string_concat(const char *a, const char *b) {
    if (!a && !b) return NULL;
    if (!a) return riven_string_from(b);
    if (!b) return riven_string_from(a);
    size_t len_a = strlen(a);
    size_t len_b = strlen(b);
    size_t total;
    if (__builtin_add_overflow(len_a, len_b, &total) ||
        __builtin_add_overflow(total, 1, &total)) {
        riven_panic("string size overflow");
    }
    char *result = (char *)malloc(total);
    if (!result) {
        riven_panic("out of memory");
    }
    memcpy(result, a, len_a);
    memcpy(result + len_a, b, len_b + 1);
    return result;
}

char *riven_string_from(const char *s) {
    if (!s) return NULL;
    size_t len = strlen(s);
    char *result = (char *)malloc(len + 1);
    if (!result) {
        riven_panic("out of memory");
    }
    memcpy(result, s, len + 1);
    return result;
}

/* Phase 2 #06.D4: return a fresh string truncated to at most `max_chars`
 * UTF-8 codepoints.  `max_chars < 0` returns a copy of the input
 * unchanged (used when the precision spec is unset).  Truncation
 * respects UTF-8 boundaries — the returned string is always valid
 * UTF-8.  Returns a freshly-allocated heap string the caller owns.
 *
 * Distinct from the in-place `riven_string_truncate(char*, n)` used by
 * `String.truncate(n)` — this variant takes char count (not bytes) and
 * returns a fresh string so the source remains usable. */
char *riven_string_truncate_chars(const char *s, int64_t max_chars) {
    if (!s) return riven_string_from("");
    if (max_chars < 0) return riven_string_from(s);
    size_t kept_bytes = 0;
    int64_t kept_chars = 0;
    while (s[kept_bytes] != '\0' && kept_chars < max_chars) {
        unsigned char c = (unsigned char) s[kept_bytes];
        size_t step = 1;
        if      ((c & 0x80) == 0x00) step = 1;
        else if ((c & 0xE0) == 0xC0) step = 2;
        else if ((c & 0xF0) == 0xE0) step = 3;
        else if ((c & 0xF8) == 0xF0) step = 4;
        kept_bytes += step;
        kept_chars += 1;
    }
    char *result = (char *) malloc(kept_bytes + 1);
    if (!result) riven_panic("out of memory");
    memcpy(result, s, kept_bytes);
    result[kept_bytes] = '\0';
    return result;
}

/* ── std::fmt::Formatter ─────────────────────────────────────────────
 *
 * Phase 2 #06.D2.S0: six runtime helpers that Phase A's CHANGELOG
 * claimed but never landed. Codegen in cranelift.rs and llvm/ already
 * has the signatures wired; these definitions close the link gap.
 *
 * Layout: heap-owned grow-able byte buffer plus reserved spec slots
 * (width/precision/align/fill) for Phase D4.
 */

typedef struct RivenFormatter {
    char    *buf;
    size_t   len;
    size_t   cap;
    /* Reserved spec slots — populated by Phase D4. */
    int32_t  width;
    int32_t  precision;
    int8_t   align;    /* 0 = default, 1 = left, 2 = center, 3 = right */
    int32_t  fill_cp;  /* UTF-32 codepoint; -1 = default ' ' */
} RivenFormatter;

/* Grow the buffer so at least `additional` more bytes can be appended. */
static void riven_fmt_formatter_reserve(RivenFormatter *f, size_t additional) {
    size_t needed = f->len + additional + 1; /* +1 for NUL */
    if (needed <= f->cap) return;
    size_t newcap = f->cap == 0 ? 16 : f->cap;
    while (newcap < needed) newcap *= 2;
    char *nb = (char *) realloc(f->buf, newcap);
    if (!nb) { fprintf(stderr, "riven: formatter alloc failed\n"); exit(101); }
    f->buf = nb;
    f->cap = newcap;
}

/* Allocate and initialise a fresh Formatter with an empty buffer. */
RivenFormatter *riven_fmt_formatter_new(void) {
    RivenFormatter *f = (RivenFormatter *) calloc(1, sizeof(RivenFormatter));
    if (!f) { fprintf(stderr, "riven: formatter alloc failed\n"); exit(101); }
    f->precision = -1;
    f->fill_cp   = -1;
    riven_fmt_formatter_reserve(f, 0);
    f->buf[0] = '\0';
    return f;
}

/* Phase 2 #06.D4: spec-aware constructor.  Used by interpolation sites
 * that carry a non-default `FormatSpec` (width / precision / align /
 * fill).  Sentinel encoding mirrors the MIR lowerer:
 *   width      == 0  → unset
 *   precision  == -1 → unset
 *   align      == 0  → default (right for numerics, left for strings);
 *                       1 = left, 2 = center, 3 = right
 *   fill_cp    == -1 → default ' ' (space)
 */
RivenFormatter *riven_fmt_formatter_new_with_spec(
    int64_t width, int64_t precision, int64_t align, int64_t fill
) {
    RivenFormatter *f = riven_fmt_formatter_new();
    f->width     = (int32_t) width;
    f->precision = (int32_t) precision;
    f->align     = (int8_t)  align;
    f->fill_cp   = (int32_t) fill;
    return f;
}

/* Phase 2 #06.D4: precision accessor.  Synth `Float_fmt` / `String_fmt`
 * read this to know whether to truncate / round.  Returns -1 when the
 * formatter was constructed without a precision (the default). */
int64_t riven_fmt_formatter_precision(const RivenFormatter *f) {
    return f ? (int64_t) f->precision : -1;
}

/* Free the Formatter and its buffer.
 * Uses the _ORIG_FREE sentinel + RIVEN_ASM_LABEL rebind so the
 * drop_fixtures textual `free(` → `riven_test_free(` rewrite does not
 * mangle this call site (same pattern as riven_string_free /
 * riven_vec_free).  Forward decl + asm label are at the top of the
 * file to satisfy macOS clang's "asm label after first use" rule. */
void riven_fmt_formatter_ORIG_FREE(RivenFormatter *f) {
    if (!f) return;
    if (f->buf) free(f->buf);
    free(f);
}

/* Append the NUL-terminated string `s` to the buffer.
 * Returns 0 (tag-0 = Ok(())) on success, 1 (tag-1 = FmtError) on
 * null input (v1 simplification — buffer overflow is not surfaced). */
int64_t riven_fmt_formatter_write_str(RivenFormatter *f, const char *s) {
    if (!f || !s) return 1;
    size_t n = strlen(s);
    riven_fmt_formatter_reserve(f, n);
    memcpy(f->buf + f->len, s, n);
    f->len += n;
    f->buf[f->len] = '\0';
    return 0;
}

/* Append a single Unicode codepoint.
 * v1: ASCII codepoints (0–0x7F) are stored directly; non-ASCII
 * codepoints emit '?' (Phase D4 will add full UTF-8 encoding). */
int64_t riven_fmt_formatter_write_char(RivenFormatter *f, int64_t codepoint) {
    if (!f) return 1;
    if (codepoint >= 0 && codepoint <= 0x7f) {
        riven_fmt_formatter_reserve(f, 1);
        f->buf[f->len++] = (char) codepoint;
        f->buf[f->len]   = '\0';
    } else {
        /* Non-ASCII placeholder until Phase D4 UTF-8 encoding lands. */
        riven_fmt_formatter_reserve(f, 1);
        f->buf[f->len++] = '?';
        f->buf[f->len]   = '\0';
    }
    return 0;
}

/* Phase 2 #06.D4: apply width / align / fill to `taken` in place.
 * Returns a freshly allocated buffer when padding is needed; otherwise
 * returns `taken` untouched.  When a new buffer is allocated the input
 * is freed.  `len` is the byte length of `taken` (must not include the
 * trailing NUL).  The padded buffer is NUL-terminated. */
static char *riven_fmt_apply_pad(
    char *taken, size_t len,
    int32_t width, int8_t align, int32_t fill_cp
) {
    if (width <= 0 || (size_t) width <= len) return taken;
    size_t pad = (size_t) width - len;
    /* v1: ASCII fill only.  Non-ASCII codepoints fall back to ' '. */
    char fill_ch = ' ';
    if (fill_cp > 0 && fill_cp <= 0x7f) fill_ch = (char) fill_cp;
    /* align: 0 default = right for numerics; the lowerer defaults to
     * align=0 when the spec omits it, so we treat 0 as right-align here.
     * 1 = left, 2 = center, 3 = right. */
    size_t left_pad  = 0;
    size_t right_pad = 0;
    if (align == 1) {              /* left  */
        right_pad = pad;
    } else if (align == 2) {       /* center: prefer extra on right */
        left_pad  = pad / 2;
        right_pad = pad - left_pad;
    } else {                       /* default + 3 = right */
        left_pad  = pad;
    }
    char *out = (char *) malloc((size_t) width + 1);
    if (!out) { fprintf(stderr, "riven: pad alloc failed\n"); exit(101); }
    memset(out, fill_ch, left_pad);
    if (taken && len > 0) memcpy(out + left_pad, taken, len);
    memset(out + left_pad + len, fill_ch, right_pad);
    out[(size_t) width] = '\0';
    if (taken) free(taken);
    return out;
}

/* Transfer buffer ownership to a Riven String and free the Formatter.
 * The accumulated `buf` is taken directly (no copy unless padding is
 * required by the spec) and returned as a heap `char*` that satisfies
 * the Riven String ABI.  Codegen must not emit a follow-up `_free` call
 * on the Formatter after this.
 *
 * Phase 2 #06.D4: when the formatter was constructed with a width spec,
 * width/align/fill are applied here.  Precision is type-specific and is
 * consumed earlier by the synth `Float_fmt` / `String_fmt` bodies, not
 * here. */
const char *riven_fmt_formatter_buffer(RivenFormatter *f) {
    if (!f) return riven_string_from("");
    char  *taken = f->buf;
    size_t len   = f->len;
    int32_t width   = f->width;
    int8_t  align   = f->align;
    int32_t fill_cp = f->fill_cp;
    /* Disown the buffer before freeing the struct so the struct's
     * destructor cannot double-free it. */
    f->buf = NULL;
    f->len = 0;
    f->cap = 0;
    riven_fmt_formatter_ORIG_FREE(f);
    if (!taken) return riven_string_from("");
    taken = riven_fmt_apply_pad(taken, len, width, align, fill_cp);
    return taken;
}

/* Return the number of bytes currently accumulated in the buffer. */
int64_t riven_fmt_formatter_len(const RivenFormatter *f) {
    return f ? (int64_t) f->len : 0;
}

/* ── Memory Management ─────────────────────────────────────────────── */

void *riven_alloc(uint64_t size) {
    void *ptr = malloc((size_t)size);
    if (!ptr && size > 0) {
        riven_panic("out of memory");
    }
    memset(ptr, 0, (size_t)size);
    return ptr;
}

void riven_dealloc(void *ptr) {
    free(ptr);
}

/* ── Heap-owned built-in drops (P0.7) ──────────────────────────────────
 *
 * The drop-elaboration pass emits `Call { callee: "riven_string_free" }`
 * (and `riven_vec_free` / `riven_hash_free`) at scope exit for owning
 * locals of those built-in types. Each helper here frees the spine of
 * its argument and tolerates NULL.
 *
 * Implementation note (test scaffolding): the `drop_fixtures` test
 * harness rewrites every `free(` call site in this file via a textual
 * `String::replace`. That rewrite mangles any function header whose
 * name ends in `free(`, so we cannot write `void riven_string_free(`
 * directly — the rewrite would turn it into
 * `void riven_string_riven_test_free(` and break the link.
 *
 * The harness exposes a sentinel `ORIG_FREE(` that survives the
 * blanket `free(` rewrite (case-sensitive `_FREE(` ≠ `free(`) and is
 * substituted back to `free(` immediately afterwards. We use that
 * sentinel for the C-level identifier — `riven_string_ORIG_FREE` —
 * and pin the link symbol to the canonical `riven_string_free` via
 * the GCC/Clang `__asm__` label syntax. Mach-O requires a leading
 * underscore on the asm label (the platform's C-name → symbol
 * convention), so the macro `RIVEN_ASM_LABEL` adds one on Darwin.
 *
 * In production, the source goes straight to the C compiler:
 *   void riven_string_ORIG_FREE(char *s) __asm__("_riven_string_free");
 * defines a function whose link symbol is `riven_string_free`.
 *
 * In the test runtime, step 4 of the splice rewrites `ORIG_FREE(` →
 * `free(`, so both the C identifier and the asm label string become
 * `riven_string_free` (a self-referent label that is a no-op rename).
 * The `inject_helper_counter` splice then matches the post-rewrite
 * header and decorates the body with a per-kind counter increment.
 */
/* `RIVEN_ASM_LABEL` and the labelled forward decls live near the top
 * of the file (around line ~50) — they have to be in scope before the
 * first caller takes the address of `_ORIG_FREE` or macOS clang
 * rejects with "asm label after first use". */
void riven_string_ORIG_FREE(char *s) {
    if (s) free(s);
}

void *riven_realloc(void *ptr, uint64_t new_size) {
    void *new_ptr = realloc(ptr, (size_t)new_size);
    if (!new_ptr && new_size > 0) {
        riven_panic("out of memory");
    }
    return new_ptr;
}

/* ── String Extended Operations ────────────────────────────────────── */

uint64_t riven_string_len(const char *s) {
    return s ? (uint64_t)strlen(s) : 0;
}

int8_t riven_string_is_empty(const char *s) {
    return (!s || s[0] == '\0') ? 1 : 0;
}

/* push_str: Two calling conventions are supported.
   (1) Caller has a local `char*` (not an address) — the codegen passes the
       value directly and reassigns the returned buffer.
   (2) Caller has a `&mut String` pointer (`char**`) — the codegen derefs it
       to read the current buffer, calls this helper, then stores the result
       back through the pointer via `riven_store_ptr`.
   Either way this helper itself just takes two `char*` and returns a fresh
   concatenated buffer. */
char *riven_string_push_str(const char *dst, const char *src) {
    if (!dst && !src) return NULL;
    return riven_string_concat(dst, src);
}

/* Dereference a pointer-to-pointer: `*p` where `p` is a `char**`.
   Used by the codegen to read the current value of a `&mut String` local
   before calling a mutating method like `push` or `push_str`. */
char *riven_deref_ptr(char **p) {
    return p ? *p : NULL;
}

/* Store through a pointer-to-pointer: `*p = v` where `p` is a `char**`.
   Used by the codegen to write back a new buffer into a `&mut String`
   local so that the caller observes the reassignment after a mutating
   method returns. */
void riven_store_ptr(char **p, char *v) {
    if (p) *p = v;
}

char *riven_string_trim(const char *s) {
    if (!s) return riven_string_from("");
    /* Skip leading whitespace */
    while (*s == ' ' || *s == '\t' || *s == '\n' || *s == '\r') s++;
    size_t len = strlen(s);
    /* Skip trailing whitespace */
    while (len > 0 && (s[len-1] == ' ' || s[len-1] == '\t' ||
           s[len-1] == '\n' || s[len-1] == '\r')) len--;
    char *result = (char *)malloc(len + 1);
    if (!result) {
        riven_panic("out of memory");
    }
    memcpy(result, s, len);
    result[len] = '\0';
    return result;
}

char *riven_string_to_lower(const char *s) {
    if (!s) return riven_string_from("");
    size_t len = strlen(s);
    char *result = (char *)malloc(len + 1);
    if (!result) {
        riven_panic("out of memory");
    }
    for (size_t i = 0; i < len; i++) {
        char c = s[i];
        if (c >= 'A' && c <= 'Z') c = c + ('a' - 'A');
        result[i] = c;
    }
    result[len] = '\0';
    return result;
}

char *riven_string_to_upper(const char *s) {
    if (!s) return riven_string_from("");
    size_t len = strlen(s);
    char *result = (char *)malloc(len + 1);
    if (!result) {
        riven_panic("out of memory");
    }
    for (size_t i = 0; i < len; i++) {
        char c = s[i];
        if (c >= 'a' && c <= 'z') c = c - ('a' - 'A');
        result[i] = c;
    }
    result[len] = '\0';
    return result;
}

/* ── String constructors (#02) ────────────────────────────────────── */

/* String.new — fresh empty owned string. */
char *riven_string_new(void) {
    return riven_string_from("");
}

/* String.with_capacity(n) — empty string with at least `n` bytes
   pre-allocated. v1 stores strings as plain malloc'd char* with no
   inline capacity field; we honour the request by allocating a buffer
   of size max(n+1, 1) but the user-visible length is still 0. */
char *riven_string_with_capacity(int64_t cap) {
    size_t want = cap > 0 ? (size_t)cap : 0;
    char *out = (char *)malloc(want + 1);
    if (!out) {
        riven_panic("out of memory");
    }
    out[0] = '\0';
    return out;
}

/* String.as_str / String.to_string — identity at the runtime layer.
   `to_string` returns an owned clone so the user can mutate it. */
const char *riven_string_as_str(const char *s) {
    return s ? s : "";
}

char *riven_string_to_string(const char *s) {
    return riven_string_from(s ? s : "");
}

/* String.bytes / String.into_bytes — return a Vec[U8] (i.e. RivenVec
   with one slot per byte). Currently runtime Vec slots are 64-bit so
   each byte gets widened on the way in; the typecker promises the
   user-visible element type is U8. */
RivenVec *riven_string_bytes(const char *s) {
    RivenVec *result = riven_vec_new();
    if (!s) return result;
    while (*s) {
        riven_vec_push(result, (int64_t)(uint8_t)*s);
        s++;
    }
    return result;
}

/* trim_start / trim_end — like trim but only one side. Returns owned. */
char *riven_string_trim_start(const char *s) {
    if (!s) return riven_string_from("");
    while (*s == ' ' || *s == '\t' || *s == '\n' || *s == '\r') s++;
    return riven_string_from(s);
}

char *riven_string_trim_end(const char *s) {
    if (!s) return riven_string_from("");
    size_t len = strlen(s);
    while (len > 0 && (s[len-1] == ' ' || s[len-1] == '\t' ||
           s[len-1] == '\n' || s[len-1] == '\r')) len--;
    char *result = (char *)malloc(len + 1);
    if (!result) {
        riven_panic("out of memory");
    }
    memcpy(result, s, len);
    result[len] = '\0';
    return result;
}

/* String.find(&str) — return Option[USize] of the first byte index of
   the needle, or None if not found / empty needle / null receiver. */
void *riven_string_find(const char *s, const char *needle) {
    int64_t *out = (int64_t *)riven_alloc(16);
    if (!s || !needle || needle[0] == '\0') {
        *(int32_t *)out = 0; /* None */
        return out;
    }
    const char *hit = strstr(s, needle);
    if (!hit) {
        *(int32_t *)out = 0; /* None */
    } else {
        *(int32_t *)out = 1; /* Some */
        out[1] = (int64_t)(hit - s);
    }
    return out;
}

/* String.splitn(n, &str) — split into at most `n` parts. Returns a
   Vec of owned strings. n <= 0 produces an empty Vec. n == 1 yields
   a Vec containing the entire string. */
RivenVec *riven_string_splitn(const char *s, int64_t n, const char *delimiter) {
    RivenVec *result = riven_vec_new();
    if (!s || n <= 0) return result;
    if (!delimiter || delimiter[0] == '\0' || n == 1) {
        riven_vec_push(result, (int64_t)riven_string_from(s));
        return result;
    }
    size_t dlen = strlen(delimiter);
    const char *start = s;
    int64_t produced = 0;
    while (produced < n - 1) {
        const char *found = strstr(start, delimiter);
        if (!found) break;
        size_t part_len = (size_t)(found - start);
        char *part = (char *)malloc(part_len + 1);
        if (!part) {
            riven_panic("out of memory");
        }
        memcpy(part, start, part_len);
        part[part_len] = '\0';
        riven_vec_push(result, (int64_t)part);
        start = found + dlen;
        produced++;
    }
    riven_vec_push(result, (int64_t)riven_string_from(start));
    return result;
}

/* String.clear / truncate(n) — mutating, in place. The caller must
   ensure the buffer is owned (i.e. came from riven_string_from /
   _new / _with_capacity). For `&mut String` parameters the codegen
   reads the buffer pointer via `riven_deref_ptr` and passes it here;
   we just rewrite the bytes in place. truncate(n) keeps at most `n`
   leading bytes; n >= len is a no-op. Negative n is a no-op. */
void riven_string_clear(char *s) {
    if (s) s[0] = '\0';
}

void riven_string_truncate(char *s, int64_t n) {
    if (!s) return;
    if (n < 0) return;
    size_t len = strlen(s);
    size_t cap = (size_t)n;
    if (cap >= len) return;
    s[cap] = '\0';
}

/* String.insert(i, char) — return a new owned string with the char
   widened to UTF-8 inserted at byte index `i`. Out-of-range index
   panics. */
char *riven_string_insert(const char *s, int64_t i, int64_t codepoint) {
    const char *base = s ? s : "";
    size_t len = strlen(base);
    if (i < 0 || (size_t)i > len) {
        riven_panic("string insert index out of bounds");
    }
    char *one = riven_char_to_string(codepoint);
    size_t one_len = strlen(one);
    char *out = (char *)malloc(len + one_len + 1);
    if (!out) {
        riven_panic("out of memory");
    }
    memcpy(out, base, (size_t)i);
    memcpy(out + i, one, one_len);
    memcpy(out + i + one_len, base + i, len - (size_t)i);
    out[len + one_len] = '\0';
    free(one);
    return out;
}

/* String.insert_str(i, &str) — same idea but for a borrowed slice. */
char *riven_string_insert_str(const char *s, int64_t i, const char *part) {
    const char *base = s ? s : "";
    size_t len = strlen(base);
    if (i < 0 || (size_t)i > len) {
        riven_panic("string insert_str index out of bounds");
    }
    const char *src = part ? part : "";
    size_t plen = strlen(src);
    char *out = (char *)malloc(len + plen + 1);
    if (!out) {
        riven_panic("out of memory");
    }
    memcpy(out, base, (size_t)i);
    memcpy(out + i, src, plen);
    memcpy(out + i + plen, base + i, len - (size_t)i);
    out[len + plen] = '\0';
    return out;
}

/* String.remove(i) — for v1 we operate on a single byte at a time
   (proper UTF-8 codepoint removal lands with full Char support, see
   §9 of the stdlib brief). Returns the byte at `i` (widened to i64)
   and an out-parameter that points at the new buffer. The codegen
   pairs this with a `riven_store_ptr` to update the &mut String. */
struct RivenStringRemove {
    int64_t removed;   /* Char codepoint (the byte for v1) */
    char *new_buffer;  /* Caller stores this back through &mut String */
};

void *riven_string_remove(const char *s, int64_t i) {
    /* Lay the result out so the codegen can read .removed (i64 at
       offset 0) and .new_buffer (ptr at offset 8) from one alloc. */
    int64_t *out = (int64_t *)riven_alloc(16);
    if (!s) {
        riven_panic("string remove on null");
    }
    size_t len = strlen(s);
    if (i < 0 || (size_t)i >= len) {
        riven_panic("string remove index out of bounds");
    }
    out[0] = (int64_t)(uint8_t)s[i];
    char *new_buf = (char *)malloc(len);  /* one byte shorter + NUL */
    if (!new_buf) {
        riven_panic("out of memory");
    }
    memcpy(new_buf, s, (size_t)i);
    memcpy(new_buf + i, s + i + 1, len - (size_t)i - 1);
    new_buf[len - 1] = '\0';
    out[1] = (int64_t)new_buf;
    return out;
}

/* String.parse_int / parse_float — full surface, returning the same
   tagged Result the rest of the runtime uses. The Err payload is a
   ParseIntError / ParseFloatError box (newtype around a String message). */
static void *riven_parse_error_box(const char *msg) {
    char **box_ = (char **)riven_alloc(8);
    *box_ = riven_string_from(msg ? msg : "parse error");
    return box_;
}

void *riven_string_parse_int(const char *s) {
    int64_t *out = (int64_t *)riven_alloc(16);
    if (!s || *s == '\0') {
        *(int32_t *)out = 1; /* Err */
        out[1] = (int64_t)riven_parse_error_box("empty string");
        return out;
    }
    /* Skip leading whitespace — matches Rust's i64::from_str_radix
       which does *not* skip whitespace, but matches the user expectation
       that "  -42" parses. We follow Rust here: no whitespace stripping. */
    char *end = NULL;
    errno = 0;
    long long val = strtoll(s, &end, 10);
    if (end == s || *end != '\0') {
        *(int32_t *)out = 1; /* Err */
        out[1] = (int64_t)riven_parse_error_box("invalid integer");
    } else if (errno == ERANGE) {
        *(int32_t *)out = 1; /* Err */
        out[1] = (int64_t)riven_parse_error_box("integer out of range");
    } else {
        *(int32_t *)out = 0; /* Ok */
        out[1] = (int64_t)val;
    }
    return out;
}

void *riven_string_parse_float(const char *s) {
    int64_t *out = (int64_t *)riven_alloc(16);
    if (!s || *s == '\0') {
        *(int32_t *)out = 1; /* Err */
        out[1] = (int64_t)riven_parse_error_box("empty string");
        return out;
    }
    char *end = NULL;
    errno = 0;
    double val = strtod(s, &end);
    if (end == s || *end != '\0') {
        *(int32_t *)out = 1; /* Err */
        out[1] = (int64_t)riven_parse_error_box("invalid float");
    } else if (errno == ERANGE) {
        *(int32_t *)out = 1; /* Err */
        out[1] = (int64_t)riven_parse_error_box("float out of range");
    } else {
        *(int32_t *)out = 0; /* Ok */
        /* Bit-cast double into the 64-bit payload slot. */
        union { double d; int64_t i; } u;
        u.d = val;
        out[1] = u.i;
    }
    return out;
}

/* ParseIntError / ParseFloatError accessors. The runtime stores both
   error variants as a newtype around a heap String — a `char**` whose
   pointee is the message buffer. The .message accessor returns a
   reference (just the inner pointer) so the caller doesn't free it. */
const char *riven_parse_error_message(char **box_) {
    if (!box_ || !*box_) return "";
    return *box_;
}

/* String.split(&str) — split at every occurrence of the delimiter.
   Returns a Vec of owned strings (heap-allocated `char*` per part).
   Differs from `riven_string_splitn`, which caps the number of parts.
   Empty delimiter or null receiver yields a Vec containing the
   original string (matches Rust's surprising `"abc".split("")`-like
   convention only at the empty-receiver edge; for our v1 surface we
   treat empty delimiter as "no split"). */
RivenVec *riven_string_split(const char *s, const char *delimiter) {
    RivenVec *result = riven_vec_new();
    if (!s) return result;
    if (!delimiter || delimiter[0] == '\0') {
        riven_vec_push(result, (int64_t)riven_string_from(s));
        return result;
    }
    size_t dlen = strlen(delimiter);
    const char *start = s;
    while (1) {
        const char *found = strstr(start, delimiter);
        if (!found) {
            riven_vec_push(result, (int64_t)riven_string_from(start));
            break;
        }
        size_t part_len = (size_t)(found - start);
        char *part = (char *)malloc(part_len + 1);
        if (!part) {
            riven_panic("out of memory");
        }
        memcpy(part, start, part_len);
        part[part_len] = '\0';
        riven_vec_push(result, (int64_t)part);
        start = found + dlen;
    }
    return result;
}

/* String.push(Char) — append a single Char (UTF-8 codepoint widened to
   i64) onto the receiver, returning a freshly allocated buffer. The
   codegen wraps this with the &mut String deref/store helpers when
   the receiver is a parameter; for an owned local it rebinds the
   variable. The receiver buffer is consumed: the codegen's reassign
   path arranges the prior buffer to be freed via the existing
   reassignment-drop machinery. */
char *riven_string_push(const char *s, int64_t codepoint) {
    const char *base = s ? s : "";
    size_t len = strlen(base);
    char *one = riven_char_to_string(codepoint);
    size_t one_len = strlen(one);
    char *out = (char *)malloc(len + one_len + 1);
    if (!out) {
        riven_panic("out of memory");
    }
    memcpy(out, base, len);
    memcpy(out + len, one, one_len + 1);
    free(one);
    return out;
}

/* String.into_bytes — consuming variant of `bytes`. Takes ownership of
   the receiver: returns a Vec[U8] holding the UTF-8 bytes and frees
   the source `char*` spine. The MIR call site is responsible for
   suppressing the regular scope-exit drop on the source local so we
   don't double-free. The Vec returned holds widened i64 byte values
   per the existing v1 element-slot convention. */
RivenVec *riven_string_into_bytes(char *s) {
    RivenVec *result = riven_vec_new();
    if (!s) return result;
    const char *p = s;
    while (*p) {
        riven_vec_push(result, (int64_t)(uint8_t)*p);
        p++;
    }
    /* Free the source buffer — the caller has handed us ownership and
       relies on us draining it. The drop pass above the call site must
       not also emit a `riven_string_free` for the same local, or this
       becomes a double-free. */
    free(s);
    return result;
}

/* ── Vec Operations ───────────────────────────────────────────────── */

/* Vec struct body is hoisted near the top of the file (immediately
 * after the typedef forward decl) so earlier code — including the
 * Phase 2 #06 std.process.Command helpers that walk a Vec[String]
 * passed as `args` — can access `->len` / `->data[i]` directly. The
 * field layout `{ int64_t *data; uint64_t len; uint64_t cap; }`
 * matches the hoisted definition exactly. */

RivenVec *riven_vec_new(void) {
    RivenVec *v = (RivenVec *)malloc(sizeof(RivenVec));
    if (!v) {
        riven_panic("out of memory");
    }
    v->data = NULL;
    v->len = 0;
    v->cap = 0;
    return v;
}

/* Vec drop — see the "Heap-owned built-in drops" comment in the
   memory-management section above for the dual-name pattern. Element
   heap (e.g. for `Vec[String]`) is a known v1 limitation: only the
   data buffer and outer struct are released; recursive element
   drops are codegen-driven and not yet emitted. */
/* asm label declared at the top of the file alongside the matching
 * forward decl — see the `RIVEN_ASM_LABEL` block there. */
void riven_vec_ORIG_FREE(RivenVec *v) {
    if (!v) return;
    if (v->data) free(v->data);
    free(v);
}

/* Internal: grow a Vec's capacity to at least `needed` slots.
   Kept available for future Vec operations even if `riven_vec_push`
   currently inlines its own grow logic. */
__attribute__((unused))
static void riven_vec_grow(RivenVec *v, uint64_t needed) {
    uint64_t new_cap = v->cap == 0 ? 4 : v->cap * 2;
    while (new_cap < needed) {
        uint64_t doubled = new_cap * 2;
        if (doubled < new_cap) {
            riven_panic("vector capacity overflow");
        }
        new_cap = doubled;
    }
    size_t alloc_size;
    if (__builtin_mul_overflow(new_cap, sizeof(int64_t), &alloc_size)) {
        riven_panic("vector allocation size overflow");
    }
    int64_t *new_data = (int64_t *)realloc(v->data, alloc_size);
    if (!new_data) {
        riven_panic("out of memory");
    }
    v->data = new_data;
    v->cap = new_cap;
}

void riven_vec_push(RivenVec *v, int64_t item) {
    if (!v) return;
    if (v->len >= v->cap) {
        uint64_t new_cap = v->cap == 0 ? 4 : v->cap * 2;
        /* Overflow check on capacity doubling */
        if (new_cap < v->cap) {
            riven_panic("vector capacity overflow");
        }
        /* Overflow check on allocation size */
        size_t alloc_size;
        if (__builtin_mul_overflow(new_cap, sizeof(int64_t), &alloc_size)) {
            riven_panic("vector allocation size overflow");
        }
        /* Preserve original pointer in case realloc fails */
        int64_t *new_data = (int64_t *)realloc(v->data, alloc_size);
        if (!new_data) {
            riven_panic("out of memory");
        }
        v->data = new_data;
        v->cap = new_cap;
    }
    v->data[v->len++] = item;
}

/* Pop the last element off a Vec, returning an Option tagged union:
   [tag:i32 pad:i32 payload:i64]. tag=0 → None, tag=1 → Some(value). */
void *riven_vec_pop(RivenVec *v) {
    int64_t *result = (int64_t *)riven_alloc(16);
    if (!v || v->len == 0) {
        *(int32_t *)result = 0; /* None */
    } else {
        v->len -= 1;
        *(int32_t *)result = 1; /* Some */
        result[1] = v->data[v->len];
    }
    return result;
}

uint64_t riven_vec_len(RivenVec *v) {
    return v ? v->len : 0;
}

/* Decode a UTF-8 string into a Vec of codepoints (widened to i64) so that
   the existing Vec-iteration machinery can drive `for ch in s.chars`.
   Malformed bytes are passed through as single-byte codepoints. */
/* String predicates / search / repeat — quick helpers around libc. */

int8_t riven_string_contains(const char *s, const char *needle) {
    if (!s || !needle) return 0;
    return strstr(s, needle) ? 1 : 0;
}

int8_t riven_string_starts_with(const char *s, const char *prefix) {
    if (!s || !prefix) return 0;
    size_t plen = strlen(prefix);
    if (plen == 0) return 1;
    if (strlen(s) < plen) return 0;
    return strncmp(s, prefix, plen) == 0 ? 1 : 0;
}

int8_t riven_string_ends_with(const char *s, const char *suffix) {
    if (!s || !suffix) return 0;
    size_t slen = strlen(s);
    size_t xlen = strlen(suffix);
    if (xlen == 0) return 1;
    if (slen < xlen) return 0;
    return strncmp(s + slen - xlen, suffix, xlen) == 0 ? 1 : 0;
}

char *riven_string_repeat(const char *s, int64_t count) {
    if (!s || count <= 0) return riven_string_from("");
    size_t slen = strlen(s);
    if (slen == 0) return riven_string_from("");
    size_t total = slen * (size_t)count;
    char *out = (char *)malloc(total + 1);
    if (!out) {
        riven_panic("out of memory");
    }
    for (int64_t i = 0; i < count; i++) {
        memcpy(out + (size_t)i * slen, s, slen);
    }
    out[total] = '\0';
    return out;
}

RivenVec *riven_string_chars(const char *s) {
    RivenVec *result = riven_vec_new();
    if (!s) return result;
    const unsigned char *p = (const unsigned char *)s;
    while (*p) {
        uint32_t cp;
        size_t n;
        unsigned char b0 = *p;
        if (b0 < 0x80) {
            cp = b0;
            n = 1;
        } else if ((b0 & 0xE0) == 0xC0 && (p[1] & 0xC0) == 0x80) {
            cp = ((uint32_t)(b0 & 0x1F) << 6)
               |  (uint32_t)(p[1] & 0x3F);
            n = 2;
        } else if ((b0 & 0xF0) == 0xE0
                   && (p[1] & 0xC0) == 0x80 && (p[2] & 0xC0) == 0x80) {
            cp = ((uint32_t)(b0 & 0x0F) << 12)
               | ((uint32_t)(p[1] & 0x3F) << 6)
               |  (uint32_t)(p[2] & 0x3F);
            n = 3;
        } else if ((b0 & 0xF8) == 0xF0
                   && (p[1] & 0xC0) == 0x80 && (p[2] & 0xC0) == 0x80
                   && (p[3] & 0xC0) == 0x80) {
            cp = ((uint32_t)(b0 & 0x07) << 18)
               | ((uint32_t)(p[1] & 0x3F) << 12)
               | ((uint32_t)(p[2] & 0x3F) << 6)
               |  (uint32_t)(p[3] & 0x3F);
            n = 4;
        } else {
            cp = b0;
            n = 1;
        }
        riven_vec_push(result, (int64_t)cp);
        p += n;
    }
    return result;
}

int64_t riven_vec_get(RivenVec *v, uint64_t index) {
    if (!v || index >= v->len) {
        riven_panic("index out of bounds");
    }
    return v->data[index];
}

/* get_mut: returns a POINTER to the element in the Vec's buffer.
   This allows mutations through the returned reference to modify
   the actual element in the Vec. Panics if out of bounds. */
int64_t *riven_vec_get_mut(RivenVec *v, uint64_t index) {
    if (!v || index >= v->len) {
        riven_panic("index out of bounds");
    }
    return &v->data[index];
}

/* get_opt: returns a proper Option tagged union (16 bytes):
   [tag: i32] [pad: i32] [payload: i64]
   tag=0 → None, tag=1 → Some(value) */
void *riven_vec_get_opt(RivenVec *v, uint64_t index) {
    int64_t *result = (int64_t *)riven_alloc(16);
    if (!v || index >= v->len) {
        *(int32_t *)result = 0; /* None */
    } else {
        *(int32_t *)result = 1; /* Some */
        result[1] = v->data[index];
    }
    return result;
}

/* get_mut_opt: like get_opt but returns a pointer to the element
   instead of a copy, enabling mutation through the reference. */
void *riven_vec_get_mut_opt(RivenVec *v, uint64_t index) {
    int64_t *result = (int64_t *)riven_alloc(16);
    if (!v || index >= v->len) {
        *(int32_t *)result = 0; /* None */
    } else {
        *(int32_t *)result = 1; /* Some */
        /* Store pointer to element, not copy of element */
        result[1] = (int64_t)&v->data[index];
    }
    return result;
}

int8_t riven_vec_is_empty(RivenVec *v) {
    return (!v || v->len == 0) ? 1 : 0;
}

void riven_vec_each(RivenVec *v, void (*callback)(int64_t)) {
    /* In the v1 runtime, closures/blocks are not yet supported.
       Just iterate and call the callback if non-null. */
    if (!v || !callback) return;
    for (uint64_t i = 0; i < v->len; i++) {
        callback(v->data[i]);
    }
}

/* Sum the elements of an Int Vec. The callee is responsible for ensuring
   the elements are i64 — non-Int Vecs are typeck-rejected upstream. */
int64_t riven_vec_sum(RivenVec *v) {
    if (!v) return 0;
    int64_t total = 0;
    for (uint64_t i = 0; i < v->len; i++) {
        total += v->data[i];
    }
    return total;
}

/* `Vec::count` — alias for length. Kept as a separate symbol so the
   codegen can dispatch without a special case. */
int64_t riven_vec_count(RivenVec *v) {
    return v ? (int64_t)v->len : 0;
}

/* Reverse a Vec in place. Returns the same pointer for fluent chaining. */
RivenVec *riven_vec_reverse(RivenVec *v) {
    if (!v || v->len < 2) return v;
    uint64_t i = 0, j = v->len - 1;
    while (i < j) {
        int64_t tmp = v->data[i];
        v->data[i] = v->data[j];
        v->data[j] = tmp;
        i++;
        j--;
    }
    return v;
}

/* Vec::first / Vec::last — return Option-tagged 16-byte pair.
   Some(v) is tag=1, payload=v; None is tag=0. */
void *riven_vec_first(RivenVec *v) {
    int64_t *out = (int64_t *)riven_alloc(16);
    if (v && v->len > 0) {
        *(int32_t *)out = 1; /* Some */
        out[1] = v->data[0];
    } else {
        *(int32_t *)out = 0; /* None */
    }
    return out;
}

void *riven_vec_last(RivenVec *v) {
    int64_t *out = (int64_t *)riven_alloc(16);
    if (v && v->len > 0) {
        *(int32_t *)out = 1; /* Some */
        out[1] = v->data[v->len - 1];
    } else {
        *(int32_t *)out = 0; /* None */
    }
    return out;
}

/* Vec::clone — shallow copy: a new Vec with the same elements.
   Element types that own heap data (String, Class) keep aliasing
   that storage; v1 only guarantees structural duplication. */
RivenVec *riven_vec_clone(RivenVec *v) {
    RivenVec *out = riven_vec_new();
    if (!v || v->len == 0) return out;
    for (uint64_t i = 0; i < v->len; i++) {
        riven_vec_push(out, v->data[i]);
    }
    return out;
}

/* `Vec::take(n)` — eager-materialise the first `n` elements as a
   fresh `RivenVec*`. Phase 2 stdlib (#05 batch 2): we do NOT yet
   model lazy iterators; `vec.iter.take(n)` returns a copy, which
   keeps the chain trivially composable with downstream eager
   terminators (`sum`, `count`, etc.) without a per-call iterator
   struct. Bounds: `n` is clamped to `[0, len]`; a negative `n`
   yields an empty Vec. Element ownership is shallow-copied (same
   contract as `riven_vec_clone`). */
RivenVec *riven_vec_take(RivenVec *v, int64_t n) {
    RivenVec *out = riven_vec_new();
    if (!v || n <= 0) return out;
    uint64_t bound = (uint64_t)n;
    if (bound > v->len) bound = v->len;
    for (uint64_t i = 0; i < bound; i++) {
        riven_vec_push(out, v->data[i]);
    }
    return out;
}

/* `Vec::skip(n)` — eager-materialise the tail starting from index `n`
   as a fresh `RivenVec*`. Mirrors `riven_vec_take` but copies from
   `n..len`. `n >= len` yields an empty Vec; `n <= 0` yields a full
   shallow clone. */
RivenVec *riven_vec_skip(RivenVec *v, int64_t n) {
    RivenVec *out = riven_vec_new();
    if (!v) return out;
    uint64_t start = n <= 0 ? 0 : (uint64_t)n;
    if (start >= v->len) return out;
    for (uint64_t i = start; i < v->len; i++) {
        riven_vec_push(out, v->data[i]);
    }
    return out;
}

/* `Iter::chain(other)` — eager-materialise the concatenation of two
   iterators as a fresh `RivenVec*`. Phase 2 stdlib (#05 batch 3): we
   keep the v1 "every iter is a Vec" invariant; chain copies each
   source's slots into a new Vec so downstream terminators (`sum`,
   `count`, `fold`, …) keep using the same `RivenVec*` shape. Element
   ownership is shallow-copied — the same contract as
   `riven_vec_clone` / `riven_vec_take` / `riven_vec_skip`. The two
   source iters remain owned by their respective scope frames. */
RivenVec *riven_vec_chain(RivenVec *a, RivenVec *b) {
    RivenVec *out = riven_vec_new();
    if (a) {
        for (uint64_t i = 0; i < a->len; i++) {
            riven_vec_push(out, a->data[i]);
        }
    }
    if (b) {
        for (uint64_t i = 0; i < b->len; i++) {
            riven_vec_push(out, b->data[i]);
        }
    }
    return out;
}

/* `Iter::zip(other)` — eager-materialise pairs as a fresh
   `RivenVec*`, where each slot holds a pointer to a freshly
   allocated 2-tuple `{a[i], b[i]}` (16 bytes, layout: field0 at +0,
   field1 at +8). Stops at the shorter source's length. The tuple
   payloads are shallow-copied — for `Vec[Int]`/`Vec[&str]` this is a
   plain bit copy; for `Vec[String]`/`Vec[Vec[_]]` the heap
   ownership is aliased (same shallow-copy contract as `clone`).

   v1 ships eager-materialisation rather than a lazy `ZipIter`
   struct; once trait-driven dispatch lands (#09) the inner cell can
   become a tagged-pair iterator over two cursors. */
RivenVec *riven_vec_zip(RivenVec *a, RivenVec *b) {
    RivenVec *out = riven_vec_new();
    if (!a || !b) return out;
    uint64_t bound = a->len < b->len ? a->len : b->len;
    for (uint64_t i = 0; i < bound; i++) {
        int64_t *pair = (int64_t *)riven_alloc(16);
        pair[0] = a->data[i];
        pair[1] = b->data[i];
        riven_vec_push(out, (int64_t)pair);
    }
    return out;
}

/* Vec::contains (Int) — linear scan for value equality.
   Element type is opaque at runtime; we treat each slot as int64
   bitwise-equal. For Vec[String]/Vec[&str] callers should use a
   different path once a polymorphic `contains` lands. */
int8_t riven_vec_contains_int(RivenVec *v, int64_t needle) {
    if (!v) return 0;
    for (uint64_t i = 0; i < v->len; i++) {
        if (v->data[i] == needle) return 1;
    }
    return 0;
}

static int riven_vec_int_cmp(const void *a, const void *b) {
    int64_t x = *(const int64_t *)a;
    int64_t y = *(const int64_t *)b;
    return x < y ? -1 : (x > y ? 1 : 0);
}

/* Vec::sort (Int) — in-place ascending qsort. Returns the same
   pointer for chaining. Non-Int Vecs are typeck-rejected upstream. */
RivenVec *riven_vec_sort(RivenVec *v) {
    if (!v || v->len < 2) return v;
    qsort(v->data, (size_t)v->len, sizeof(int64_t), riven_vec_int_cmp);
    return v;
}

/* ── Vec[T] surface — Phase 2 stdlib batch 1 (#03) ──────────────────
 * Constructors / inspectors / mutators that the v1 surface promises
 * but the runtime did not yet implement. All operate on the same
 * 64-bit-slot element layout; the type system enforces correctness
 * upstream so e.g. `riven_vec_swap` on Vec[String] is safe — the
 * slot bits are pointer-typed but bitwise-swappable.
 */

/* Vec.with_capacity(cap) — pre-allocate the backing array. */
RivenVec *riven_vec_with_capacity(uint64_t cap) {
    RivenVec *v = riven_vec_new();
    if (cap == 0) return v;
    size_t alloc_size;
    if (__builtin_mul_overflow(cap, sizeof(int64_t), &alloc_size)) {
        riven_panic("vector allocation size overflow");
    }
    int64_t *data = (int64_t *)malloc(alloc_size);
    if (!data) {
        riven_panic("out of memory");
    }
    v->data = data;
    v->cap = cap;
    return v;
}

/* Vec.capacity — total slot count of the backing array. */
uint64_t riven_vec_capacity(RivenVec *v) {
    return v ? v->cap : 0;
}

/* Vec.clear — reset length to zero without freeing the backing
   array. Element-owned heap (e.g. Vec[String]) is NOT walked here;
   the v1 contract is that `clear` is a *bulk forget* — callers who
   need element-drop should use `truncate(0)` once that helper learns
   per-type drop, or drain explicitly. */
void riven_vec_clear(RivenVec *v) {
    if (!v) return;
    v->len = 0;
}

/* Vec.truncate(n) — drop trailing elements past index `n`. No-op if
   `n >= len`. */
void riven_vec_truncate(RivenVec *v, uint64_t n) {
    if (!v) return;
    if (n < v->len) {
        v->len = n;
    }
}

/* Vec.swap(i, j) — swap two elements in place. Panics on OOB. */
void riven_vec_swap(RivenVec *v, uint64_t i, uint64_t j) {
    if (!v || i >= v->len || j >= v->len) {
        riven_panic("vec swap: index out of bounds");
    }
    if (i == j) return;
    int64_t tmp = v->data[i];
    v->data[i] = v->data[j];
    v->data[j] = tmp;
}

/* Vec.insert(i, item) — shift elements at >= i one slot right and
   place `item` at position `i`. Panics if i > len. */
void riven_vec_insert(RivenVec *v, uint64_t i, int64_t item) {
    if (!v) {
        riven_panic("vec insert: null receiver");
    }
    if (i > v->len) {
        riven_panic("vec insert: index out of bounds");
    }
    /* Reuse push's grow path by appending then rotating. */
    riven_vec_push(v, 0);
    for (uint64_t k = v->len - 1; k > i; k--) {
        v->data[k] = v->data[k - 1];
    }
    v->data[i] = item;
}

/* Vec.remove(i) — remove and return the element at index `i`,
   shifting subsequent elements one slot left. Panics on OOB. */
int64_t riven_vec_remove(RivenVec *v, uint64_t i) {
    if (!v || i >= v->len) {
        riven_panic("vec remove: index out of bounds");
    }
    int64_t out = v->data[i];
    for (uint64_t k = i; k + 1 < v->len; k++) {
        v->data[k] = v->data[k + 1];
    }
    v->len -= 1;
    return out;
}

/* Vec.extend(other) — append every element of `other` to `self`.
   The element slots are copied bitwise; for Vec[String] this means
   the destination aliases the source's owned strings. The caller is
   responsible for ensuring `other` is forgotten/cleared before drop
   to avoid double-free; v1 documents this as a known sharp edge
   for heap-element types and the borrow checker treats `extend`'s
   second arg as a borrow only — full move semantics land in 05. */
void riven_vec_extend(RivenVec *v, RivenVec *other) {
    if (!v || !other) return;
    for (uint64_t k = 0; k < other->len; k++) {
        riven_vec_push(v, other->data[k]);
    }
}

/* Vec[i] — panicking indexed read used by IndexOp lowering for Vec
   receivers. Mirrors Rust's `v[i]`: OOB → panic with a descriptive
   message including both `i` and `len`. */
int64_t riven_vec_get_or_panic(RivenVec *v, uint64_t i) {
    if (!v || i >= (v ? v->len : 0)) {
        char buf[96];
        uint64_t len = v ? v->len : 0;
        snprintf(buf, sizeof(buf), "index %llu out of range, len %llu",
                 (unsigned long long)i, (unsigned long long)len);
        riven_panic(buf);
    }
    return v->data[i];
}

/* Vec[T] == Vec[T] — pairwise equality of slots. For primitive T
   this is bitwise; for Vec[String] the typechecker will require
   `T: PartialEq` and we still bitwise-compare slot pointers — true
   structural string equality on Vec[String] will land alongside the
   PartialEq trait dispatch in 05. v1 ships the integer-correct
   version that the existing fixtures exercise. */
int8_t riven_vec_eq(RivenVec *a, RivenVec *b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    if (a->len != b->len) return 0;
    for (uint64_t i = 0; i < a->len; i++) {
        if (a->data[i] != b->data[i]) return 0;
    }
    return 1;
}

/* Vec[String] / Vec[Vec[T]] element-aware drop. The existing
   `riven_vec_free` releases only the spine + backing array; for
   element types that own heap (String, nested Vec, HashMap), we
   need to walk the elements first. Drop elaboration in MIR picks
   the right per-element variant based on the static element type.
   These helpers are idempotent on null and never re-enter on
   already-freed slots (we zero before falling into the spine
   free). */
/* NOTE on free-helper call sites in this file: the drop_fixtures test
 * harness textually rewrites every `free(` substring to
 * `riven_test_free(`. To call `riven_string_free` / `riven_vec_free`
 * from C without losing them to that rewrite we go through the
 * `ORIG_FREE(` sentinel — see the long comment above
 * `riven_string_ORIG_FREE`. In the production build the sentinel
 * resolves directly to the canonical helper via the asm-label
 * declarations; under the leak-tracking splice it is restored to
 * `free(` and folded into the wrapper that bumps the per-kind
 * counters.
 */
void riven_vec_drop_string(RivenVec *v) {
    if (!v) return;
    for (uint64_t i = 0; i < v->len; i++) {
        char *s = (char *)v->data[i];
        if (s) riven_string_ORIG_FREE(s);
        v->data[i] = 0;
    }
    riven_vec_ORIG_FREE(v);
}

void riven_vec_drop_vec(RivenVec *v) {
    if (!v) return;
    for (uint64_t i = 0; i < v->len; i++) {
        RivenVec *inner = (RivenVec *)v->data[i];
        if (inner) {
            /* Recurse: inner Vecs of integer slots are spine-only. The
               compiler currently only emits this for Vec[Vec[Int]]; for
               Vec[Vec[String]] the per-type drop selector will pick a
               nested-string variant once parser surfaces that. */
            riven_vec_ORIG_FREE(inner);
        }
        v->data[i] = 0;
    }
    riven_vec_ORIG_FREE(v);
}

/* ---------------------------------------------------------------------
 * std::process::run (Phase 3)
 *
 * Fork+execvp a child process inheriting stdin/stdout/stderr from the
 * parent and return its exit code. Output capture is intentionally out
 * of scope for v1 — that's a follow-up that needs a richer return type
 * (ProcessOutput { exit_code, stdout, stderr }) and pipe plumbing.
 *
 * `args` is a `Vec[String]` whose slots hold heap `char *` pointers
 * cast to int64 (see "Vec Operations" — every Vec slot is an int64,
 * the type is dictated by codegen). We borrow those pointers directly
 * into argv; we do NOT take ownership, so the caller's drop logic for
 * the Vec[String] still runs as expected.
 *
 * Return value:
 *   - normal child exit:   the exit code (0..=255)
 *   - signal termination:  128 + signal number (matches POSIX shells)
 *   - fork() failure:      127
 *   - malloc() failure:    127
 *   - execvp() failure:    127 (reported from child via _exit, see below)
 *
 * 127 is the conventional "command not found / could not exec" code
 * used by POSIX shells, so callers that just check `!= 0` will treat
 * exec failures the same as a missing binary.
 * --------------------------------------------------------------------- */
int64_t riven_process_run(const char *cmd, RivenVec *args) {
    if (!cmd) {
        return 127;
    }

    uint64_t arg_count = args ? args->len : 0;
    /* argv layout: [cmd, arg0, arg1, ..., NULL] — so argc + 2 slots. */
    size_t argv_slots;
    if (__builtin_add_overflow(arg_count, (uint64_t)2, &argv_slots)) {
        return 127;
    }
    char **argv = (char **)malloc(sizeof(char *) * argv_slots);
    if (!argv) {
        return 127;
    }

    /* execvp's prototype takes `char *const argv[]`, but POSIX guarantees
       it does not modify the strings — the const-cast is the standard
       idiom (see `man 3p execvp`). */
    argv[0] = (char *)cmd;
    for (uint64_t i = 0; i < arg_count; i++) {
        argv[i + 1] = (char *)(uintptr_t)args->data[i];
    }
    argv[arg_count + 1] = NULL;

    pid_t pid = fork();
    if (pid < 0) {
        int saved = errno;
        fprintf(stderr, "riven_process_run: fork failed: %s (errno=%d)\n",
                strerror(saved), saved);
        free(argv);
        return 127;
    }

    if (pid == 0) {
        /* Child: replace image. On success, execvp does not return.
           On failure, write the cause to stderr (the parent's stderr
           is inherited) so CI logs surface *why* exec failed instead
           of just "child exited 127". Then `_exit` (not `exit`) so
           the parent's atexit/stdio cleanup is not duplicated. */
        execvp(cmd, argv);
        int saved = errno;
        fprintf(stderr,
                "riven_process_run: execvp(\"%s\") failed: %s (errno=%d)\n",
                cmd ? cmd : "(null)",
                strerror(saved),
                saved);
        _exit(127);
    }

    /* Parent: free argv (the strings still belong to the Vec) and wait. */
    free(argv);

    int status = 0;
    while (waitpid(pid, &status, 0) < 0) {
        if (errno == EINTR) {
            continue;
        }
        int saved = errno;
        fprintf(stderr, "riven_process_run: waitpid failed: %s (errno=%d)\n",
                strerror(saved), saved);
        return 127;
    }

    if (WIFEXITED(status)) {
        return (int64_t)WEXITSTATUS(status);
    }
    if (WIFSIGNALED(status)) {
        return (int64_t)(128 + WTERMSIG(status));
    }
    /* Stopped/continued — shouldn't happen with the default waitpid
       options. Treat as a generic failure. */
    return 127;
}

/* ── Vec[T] surface — Phase 2 stdlib batch 2 (#03) ──────────────────
 * `from_iter`, `dedup`, plus the consume-style `into_iter` whose
 * runtime side is identity (the v1 iterator representation IS a
 * RivenVec*, so `iter` / `into_iter` / `iter_mut` are all the same
 * passthrough at the C layer). The closure-takers `sort_by` and
 * `retain` are inlined at the MIR layer and do not appear here.
 */

/* Vec.from_iter(iter) — currently identity passthrough since every
   "iterator" in the v1 runtime is already a RivenVec*. The MIR-level
   drop-elaboration treats this as a fresh allocation (see
   FRESH_ALLOC_CALLEES in mir/lower.rs) so the destination local
   inherits the spine-free responsibility. */
RivenVec *riven_vec_from_iter(RivenVec *iter) {
    /* The source iter is consumed (its Vec spine is what the new Vec
       owns now). The drop pass already taints the source local via
       the consume-helper match. Nothing else to do. */
    return iter;
}

/* String.from_iter(iter[String]) / iter.collect[String] — concatenate
   the owned string (or &str-like) elements in iteration order into one
   fresh owned string. v1 keeps this narrow: typeck only accepts
   String/&str items, not arbitrary Display/ToString conversions. */
char *riven_string_from_iter(RivenVec *iter) {
    char *out = riven_string_from("");
    if (!iter) return out;
    for (uint64_t i = 0; i < iter->len; i++) {
        const char *part = (const char *)iter->data[i];
        char *next = riven_string_concat(out, part ? part : "");
        riven_string_ORIG_FREE(out);
        out = next;
    }
    return out;
}

/* Vec slot store — used by the inlined `retain` lowering to compact
   surviving elements into the prefix. Panics on OOB so a buggy
   inliner (or a future user-callable use that escapes) doesn't
   silently corrupt out-of-bounds memory. */
void riven_vec_set(RivenVec *v, uint64_t index, int64_t value) {
    if (!v || index >= v->len) {
        riven_panic("vec set: index out of bounds");
    }
    v->data[index] = value;
}

/* Vec.dedup — remove consecutive duplicates. Mirrors Rust's
   `Vec::dedup` for primitive slot equality (PartialEq required at the
   type level; the runtime uses bitwise 64-bit slot compare). */
void riven_vec_dedup(RivenVec *v) {
    if (!v || v->len < 2) return;
    uint64_t write = 1;
    for (uint64_t read = 1; read < v->len; read++) {
        if (v->data[read] != v->data[write - 1]) {
            v->data[write] = v->data[read];
            write++;
        }
    }
    v->len = write;
}

/* Vec::join — concatenate string elements with a separator.
   Treats each Vec slot as a `const char *`. Caller is responsible
   for ensuring the elements are strings; non-String Vecs are
   typeck-rejected upstream. */
char *riven_vec_join(RivenVec *v, const char *sep) {
    if (!v || v->len == 0) return riven_string_from("");
    const char *separator = sep ? sep : "";
    size_t sep_len = strlen(separator);
    size_t total = 0;
    for (uint64_t i = 0; i < v->len; i++) {
        const char *s = (const char *)v->data[i];
        if (s) total += strlen(s);
    }
    if (v->len > 1) {
        total += sep_len * (v->len - 1);
    }
    char *out = (char *)malloc(total + 1);
    if (!out) {
        riven_panic("out of memory");
    }
    char *cursor = out;
    for (uint64_t i = 0; i < v->len; i++) {
        const char *s = (const char *)v->data[i];
        if (i > 0 && sep_len > 0) {
            memcpy(cursor, separator, sep_len);
            cursor += sep_len;
        }
        if (s) {
            size_t slen = strlen(s);
            memcpy(cursor, s, slen);
            cursor += slen;
        }
    }
    *cursor = '\0';
    return out;
}

/* String::lines — split on '\n'. The trailing empty element after
   a terminating newline is dropped (matches Rust's `str::lines`).
   '\r' immediately before a '\n' is also dropped. */
RivenVec *riven_string_lines(const char *s) {
    RivenVec *result = riven_vec_new();
    if (!s) return result;
    const char *start = s;
    while (1) {
        const char *nl = strchr(start, '\n');
        size_t len = nl ? (size_t)(nl - start) : strlen(start);
        if (len > 0 && start[len - 1] == '\r') {
            len--;
        }
        char *line = (char *)malloc(len + 1);
        if (!line) {
            riven_panic("out of memory");
        }
        memcpy(line, start, len);
        line[len] = '\0';
        riven_vec_push(result, (int64_t)line);
        if (!nl) break;
        start = nl + 1;
        if (*start == '\0') {
            /* Drop the trailing empty element after a final '\n'. */
            break;
        }
    }
    return result;
}

/* String::replace — non-overlapping substring replace. Returns a
   newly allocated owned string; the receiver is left intact. */
char *riven_string_replace(const char *s, const char *from, const char *to) {
    if (!s) return riven_string_from("");
    if (!from || from[0] == '\0') return riven_string_from(s);
    const char *replacement = to ? to : "";
    size_t from_len = strlen(from);
    size_t to_len = strlen(replacement);
    size_t s_len = strlen(s);

    /* First pass: count occurrences to size the output exactly. */
    size_t occurrences = 0;
    {
        const char *cursor = s;
        while ((cursor = strstr(cursor, from)) != NULL) {
            occurrences++;
            cursor += from_len;
        }
    }
    if (occurrences == 0) return riven_string_from(s);

    size_t total = s_len + occurrences * (to_len > from_len ? (to_len - from_len) : 0);
    if (to_len < from_len) {
        total = s_len - occurrences * (from_len - to_len);
    }
    char *out = (char *)malloc(total + 1);
    if (!out) {
        riven_panic("out of memory");
    }

    char *write = out;
    const char *read = s;
    while (1) {
        const char *next = strstr(read, from);
        if (!next) {
            size_t tail = strlen(read);
            memcpy(write, read, tail);
            write += tail;
            break;
        }
        size_t prefix = (size_t)(next - read);
        memcpy(write, read, prefix);
        write += prefix;
        memcpy(write, replacement, to_len);
        write += to_len;
        read = next + from_len;
    }
    *write = '\0';
    return out;
}

/* ── Iterator → Vec collection ────────────────────────────────────────
 * `Iter::to_vec` collects an iterator into a Vec. In the v1 runtime
 * every iterator producer (`riven_str_split`, `riven_vec_iter`, …)
 * already returns a `RivenVec *` rather than a separate iterator
 * struct, so `to_vec` is an identity passthrough. When real iterator
 * types land later this can dispatch to a per-iterator collector.
 */
RivenVec *riven_iter_to_vec(RivenVec *iter) {
    return iter;
}

/* ── Hash Operations ──────────────────────────────────────────────── */

/* Simple Hash: array of bucket linked lists, keyed by uintptr_t.
   Keys and values are both pointer-sized (stored as int64_t).
   For string keys (char*), hashing walks the bytes. For integer keys,
   the raw bits are hashed. Chained collisions handled via next pointer. */

typedef struct RivenHashEntry {
    int64_t key;
    int64_t value;
    struct RivenHashEntry *next;
} RivenHashEntry;

#define RIVEN_HASH_INITIAL_BUCKETS 16u

struct RivenHash {
    /* Heap-allocated bucket array of length `bucket_count`. The
       count starts at RIVEN_HASH_INITIAL_BUCKETS and doubles when
       `len` would push the load factor past 0.75 — see
       `riven_hash_maybe_grow` below. Keeping the count as a power
       of two preserves a clean `% bucket_count` modulo without
       biasing the splitmix/FNV hash distribution. */
    RivenHashEntry **buckets;
    uint64_t bucket_count;
    uint64_t len;
    /* Flag set to 1 if keys should be compared/hashed as C strings. The
       first inserted key decides: string pointers have the top bit clear
       on practical platforms, but we can't reliably detect that. Instead,
       we use a heuristic: if the first key, as a pointer, points to a
       readable NUL-terminated region that is ASCII-ish, treat as string.
       For simplicity and correctness, we always hash the low 8 bytes as
       raw bits and only switch to strcmp if the caller uses the string
       variant (see riven_hash_insert_str). v1 keeps a single code path
       and treats keys by raw bits, relying on the fact that string
       interning / stable pointers aren't assumed — callers using string
       keys in the v1 runtime must pass pointers whose identity matches
       the `riven_string_from`-returned pointer. Since hash!{} lowers to
       insert calls on the same string literals, this works for the
       common case where the same string constant pointer is reused. */
    int8_t string_keys;
};

static uint64_t riven_hash_bits(int64_t k) {
    /* splitmix64-ish finalizer for decent distribution on raw int bits. */
    uint64_t x = (uint64_t)k;
    x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL;
    x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL;
    x = x ^ (x >> 31);
    return x;
}

static uint64_t riven_hash_str(const char *s) {
    /* FNV-1a on the byte contents for string-keyed hashes. */
    uint64_t h = 1469598103934665603ULL;
    if (!s) return h;
    while (*s) {
        h ^= (uint8_t)(*s);
        h *= 1099511628211ULL;
        s++;
    }
    return h;
}

static int riven_hash_keys_equal(const RivenHash *h, int64_t a, int64_t b) {
    if (a == b) return 1;
    if (h && h->string_keys) {
        const char *sa = (const char *)a;
        const char *sb = (const char *)b;
        if (!sa || !sb) return 0;
        return strcmp(sa, sb) == 0;
    }
    return 0;
}

static uint64_t riven_hash_key_hash(const RivenHash *h, int64_t key) {
    if (h && h->string_keys) {
        return riven_hash_str((const char *)key);
    }
    return riven_hash_bits(key);
}

/* Heuristic: assume the key is a string if its value looks like a
   valid pointer (non-zero, points to a readable byte region). This
   is conservative — tests use literal string constants whose bits
   are always >= 0x1000 on practical systems. Integers small enough
   to be clearly non-pointers fall through to bit hashing. */
static int riven_hash_looks_like_string(int64_t key) {
    uintptr_t p = (uintptr_t)key;
    /* Small non-pointer values. */
    if (p < 0x1000) return 0;
    /* Probe the first byte; if it's ASCII/UTF-8 and followed by a NUL
       within a short window, treat as string. This is best-effort; we
       accept false negatives (rare). */
    const unsigned char *s = (const unsigned char *)p;
    for (size_t i = 0; i < 256; i++) {
        if (s[i] == 0) return i > 0;
        if (s[i] < 0x09) return 0; /* control char — not a C string */
    }
    /* No NUL in 256 bytes — probably not a C string we care about. */
    return 0;
}

RivenHash *riven_hash_new(void) {
    RivenHash *h = (RivenHash *)malloc(sizeof(RivenHash));
    if (!h) {
        riven_panic("out of memory");
    }
    h->bucket_count = RIVEN_HASH_INITIAL_BUCKETS;
    /* Use malloc + explicit NULL init rather than calloc so the
       drop_fixtures leak harness (which rewrites `malloc(` →
       `riven_test_malloc(` and `free(` → `riven_test_free(`) keeps
       the per-allocation counters balanced. calloc is not rewritten,
       so a calloc'd buckets array would free-count without an
       alloc-count and underflow the raw_outstanding counter. */
    h->buckets = (RivenHashEntry **)malloc(
        h->bucket_count * sizeof(RivenHashEntry *));
    if (!h->buckets) {
        riven_panic("out of memory");
    }
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        h->buckets[i] = NULL;
    }
    h->len = 0;
    h->string_keys = -1; /* unset — decided on first insert */
    return h;
}

/* Hash drop — see the "Heap-owned built-in drops" comment in the
   memory-management section above for the dual-name pattern. The
   spine-only variant frees the bucket chains and the struct itself;
   per-element drop selectors below walk K/V slots before delegating
   here. */
void riven_hash_ORIG_FREE(RivenHash *h) RIVEN_ASM_LABEL(riven_hash_free);
void riven_hash_ORIG_FREE(RivenHash *h) {
    if (!h) return;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        RivenHashEntry *e = h->buckets[i];
        while (e) {
            RivenHashEntry *nx = e->next;
            free(e);
            e = nx;
        }
    }
    free(h->buckets);
    free(h);
}

/* ── HashMap[String, V] / [K, String] / [String, String] per-element
 * drop helpers — Phase 2 stdlib (#04 batch 2).
 *
 * These walk the bucket chains BEFORE the spine free, releasing the
 * heap-owned key (or value, or both) for each entry. The drop
 * selector in `mir/lower.rs::insert_drops` dispatches on `Ty::Map(K, V)`
 * to pick the right one based on K/V being heap-owning. The
 * `_ORIG_FREE` sentinel naming pattern (see `riven_string_ORIG_FREE`
 * comment ~line 620) keeps the link symbol clean while letting the
 * leak-tracker rewrite `free(` calls inside the body.
 */
void riven_hash_drop_string_v_ORIG_FREE(RivenHash *h)
    RIVEN_ASM_LABEL(riven_hash_drop_string_v);
void riven_hash_drop_string_v_ORIG_FREE(RivenHash *h) {
    if (!h) return;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        for (RivenHashEntry *e = h->buckets[i]; e; e = e->next) {
            char *k = (char *)e->key;
            if (k) riven_string_ORIG_FREE(k);
            e->key = 0;
        }
    }
    riven_hash_ORIG_FREE(h);
}

void riven_hash_drop_v_string_ORIG_FREE(RivenHash *h)
    RIVEN_ASM_LABEL(riven_hash_drop_v_string);
void riven_hash_drop_v_string_ORIG_FREE(RivenHash *h) {
    if (!h) return;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        for (RivenHashEntry *e = h->buckets[i]; e; e = e->next) {
            char *v = (char *)e->value;
            if (v) riven_string_ORIG_FREE(v);
            e->value = 0;
        }
    }
    riven_hash_ORIG_FREE(h);
}

void riven_hash_drop_string_string_ORIG_FREE(RivenHash *h)
    RIVEN_ASM_LABEL(riven_hash_drop_string_string);
void riven_hash_drop_string_string_ORIG_FREE(RivenHash *h) {
    if (!h) return;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        for (RivenHashEntry *e = h->buckets[i]; e; e = e->next) {
            char *k = (char *)e->key;
            char *v = (char *)e->value;
            if (k) riven_string_ORIG_FREE(k);
            if (v) riven_string_ORIG_FREE(v);
            e->key = 0;
            e->value = 0;
        }
    }
    riven_hash_ORIG_FREE(h);
}

/* HashMap[K, Vec[T]] — value slot owns a RivenVec*. v1 only walks one
 * level here (the inner Vec spine free); deeper element heap inside the
 * inner Vec is not currently freed. Acceptable for the v1 surface; the
 * trait-driven dispatch in #05 will widen this. */
void riven_hash_drop_v_vec_ORIG_FREE(RivenHash *h)
    RIVEN_ASM_LABEL(riven_hash_drop_v_vec);
void riven_hash_drop_v_vec_ORIG_FREE(RivenHash *h) {
    if (!h) return;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        for (RivenHashEntry *e = h->buckets[i]; e; e = e->next) {
            RivenVec *v = (RivenVec *)e->value;
            if (v) riven_vec_ORIG_FREE(v);
            e->value = 0;
        }
    }
    riven_hash_ORIG_FREE(h);
}

/* Double the bucket array and rehash every entry into the new
   spine. Called from `riven_hash_insert` when an additional entry
   would push the load factor past 0.75. We splice the existing
   RivenHashEntry nodes into the new buckets in place — no
   per-entry malloc/free, so a rehash costs one buckets-array
   alloc + one buckets-array free regardless of how many entries
   we relink. The leak harness counts only `free(` calls, and we
   add one matched malloc/free pair for the buckets spine, so
   counters stay balanced. */
static void riven_hash_maybe_grow(RivenHash *h) {
    /* Load factor 0.75 — grow when len+1 would exceed
       bucket_count * 3 / 4. Integer-math equivalent below. */
    if (h->len + 1 <= (h->bucket_count * 3) / 4) return;

    uint64_t old_n = h->bucket_count;
    uint64_t new_n = old_n * 2;
    /* malloc + NULL-init for leak-harness counter parity — see the
       comment in riven_hash_new. */
    RivenHashEntry **nb =
        (RivenHashEntry **)malloc(new_n * sizeof(RivenHashEntry *));
    if (!nb) {
        riven_panic("out of memory");
    }
    for (uint64_t j = 0; j < new_n; j++) {
        nb[j] = NULL;
    }
    for (uint64_t i = 0; i < old_n; i++) {
        RivenHashEntry *e = h->buckets[i];
        while (e) {
            RivenHashEntry *nx = e->next;
            uint64_t bj = riven_hash_key_hash(h, e->key) % new_n;
            e->next = nb[bj];
            nb[bj] = e;
            e = nx;
        }
    }
    free(h->buckets);
    h->buckets = nb;
    h->bucket_count = new_n;
}

void riven_hash_insert(RivenHash *h, int64_t key, int64_t value) {
    if (!h) return;
    if (h->string_keys < 0) {
        h->string_keys = riven_hash_looks_like_string(key) ? 1 : 0;
    }
    uint64_t bucket_idx = riven_hash_key_hash(h, key) % h->bucket_count;
    RivenHashEntry *e = h->buckets[bucket_idx];
    while (e) {
        if (riven_hash_keys_equal(h, e->key, key)) {
            e->value = value;
            return;
        }
        e = e->next;
    }
    /* Grow before linking the new entry. Grow recomputes the bucket
       index, so we re-read after. */
    riven_hash_maybe_grow(h);
    bucket_idx = riven_hash_key_hash(h, key) % h->bucket_count;
    RivenHashEntry *ne = (RivenHashEntry *)malloc(sizeof(RivenHashEntry));
    if (!ne) {
        riven_panic("out of memory");
    }
    ne->key = key;
    ne->value = value;
    ne->next = h->buckets[bucket_idx];
    h->buckets[bucket_idx] = ne;
    h->len += 1;
}

/* Return an Option tagged union (16 bytes): tag=1 Some(&value), tag=0 None.
   The payload carries the raw value (v1 treats &V the same as V at the
   runtime level — both are 8 bytes). */
void *riven_hash_get(RivenHash *h, int64_t key) {
    int64_t *result = (int64_t *)riven_alloc(16);
    if (!h) {
        *(int32_t *)result = 0;
        return result;
    }
    uint64_t bucket_idx = riven_hash_key_hash(h, key) % h->bucket_count;
    RivenHashEntry *e = h->buckets[bucket_idx];
    while (e) {
        if (riven_hash_keys_equal(h, e->key, key)) {
            *(int32_t *)result = 1; /* Some */
            result[1] = e->value;
            return result;
        }
        e = e->next;
    }
    *(int32_t *)result = 0; /* None */
    return result;
}

int8_t riven_hash_contains_key(RivenHash *h, int64_t key) {
    if (!h) return 0;
    uint64_t bucket_idx = riven_hash_key_hash(h, key) % h->bucket_count;
    RivenHashEntry *e = h->buckets[bucket_idx];
    while (e) {
        if (riven_hash_keys_equal(h, e->key, key)) {
            return 1;
        }
        e = e->next;
    }
    return 0;
}

uint64_t riven_hash_len(RivenHash *h) {
    return h ? h->len : 0;
}

int8_t riven_hash_is_empty(RivenHash *h) {
    return (!h || h->len == 0) ? 1 : 0;
}

/* ── Set Operations ───────────────────────────────────────────────── */

/* Built on top of the Hash — values are unused (set to 1). */

typedef struct {
    RivenHash inner;
} RivenSet;

RivenSet *riven_set_new(void) {
    RivenSet *s = (RivenSet *)malloc(sizeof(RivenSet));
    if (!s) {
        riven_panic("out of memory");
    }
    s->inner.bucket_count = RIVEN_HASH_INITIAL_BUCKETS;
    /* malloc + NULL-init for leak-harness counter parity with the
       free() rewrite — see the comment in riven_hash_new. */
    s->inner.buckets = (RivenHashEntry **)malloc(
        s->inner.bucket_count * sizeof(RivenHashEntry *));
    if (!s->inner.buckets) {
        riven_panic("out of memory");
    }
    for (uint64_t i = 0; i < s->inner.bucket_count; i++) {
        s->inner.buckets[i] = NULL;
    }
    s->inner.len = 0;
    s->inner.string_keys = -1;
    return s;
}

void riven_set_insert(RivenSet *s, int64_t item) {
    if (!s) return;
    /* Reuse hash insert for dedup semantics; value is 1 (unused). */
    riven_hash_insert(&s->inner, item, 1);
}

int8_t riven_set_contains(RivenSet *s, int64_t item) {
    if (!s) return 0;
    return riven_hash_contains_key(&s->inner, item);
}

uint64_t riven_set_len(RivenSet *s) {
    return s ? s->inner.len : 0;
}

int8_t riven_set_is_empty(RivenSet *s) {
    return (!s || s->inner.len == 0) ? 1 : 0;
}

/* Set spine drop — Phase 2 stdlib (#04 batch 2). Mirrors
   `riven_hash_ORIG_FREE`: walks bucket chains, frees each entry,
   frees the spine. The element heap (heap-owned T) is freed by
   `riven_set_drop_string` below before delegating here. */
void riven_set_ORIG_FREE(RivenSet *s) RIVEN_ASM_LABEL(riven_set_free);
void riven_set_ORIG_FREE(RivenSet *s) {
    if (!s) return;
    for (uint64_t i = 0; i < s->inner.bucket_count; i++) {
        RivenHashEntry *e = s->inner.buckets[i];
        while (e) {
            RivenHashEntry *nx = e->next;
            free(e);
            e = nx;
        }
    }
    free(s->inner.buckets);
    free(s);
}

/* HashSet[String] per-element drop — frees each owned key string before
 * the spine free. Selector dispatched from `mir/lower.rs::insert_drops`
 * for `Ty::Set(Ty::String)`. */
void riven_set_drop_string_ORIG_FREE(RivenSet *s)
    RIVEN_ASM_LABEL(riven_set_drop_string);
void riven_set_drop_string_ORIG_FREE(RivenSet *s) {
    if (!s) return;
    for (uint64_t i = 0; i < s->inner.bucket_count; i++) {
        for (RivenHashEntry *e = s->inner.buckets[i]; e; e = e->next) {
            char *k = (char *)e->key;
            if (k) riven_string_ORIG_FREE(k);
            e->key = 0;
        }
    }
    riven_set_ORIG_FREE(s);
}

/* ── HashMap[K,V] surface — Phase 2 stdlib (#04) ──────────────────── */

/* HashMap.with_capacity(Int) — capacity is an advisory hint in v1.
   The chained-bucket implementation has fixed bucket count, so this
   function returns a fresh empty map identical to `riven_hash_new`.
   A future open-addressing rewrite would use the hint to size the
   initial bucket array. */
RivenHash *riven_hash_with_capacity(int64_t cap) {
    (void)cap;
    return riven_hash_new();
}

/* HashMap.remove(&K) — return Option[V] (16-byte tagged union).
   tag=1 Some(prior_value), tag=0 None. Walks the bucket chain and
   unlinks the matching entry; the freed RivenHashEntry must go
   through `riven_test_free` under the leak harness, which is what
   plain `free(` rewrites to. The bucket-walk emits an explicit
   `free(e)` so the entry counter increments correctly. */
void *riven_hash_remove(RivenHash *h, int64_t key) {
    int64_t *result = (int64_t *)riven_alloc(16);
    if (!h) {
        *(int32_t *)result = 0;
        return result;
    }
    uint64_t bucket_idx = riven_hash_key_hash(h, key) % h->bucket_count;
    RivenHashEntry *prev = NULL;
    RivenHashEntry *e = h->buckets[bucket_idx];
    while (e) {
        if (riven_hash_keys_equal(h, e->key, key)) {
            int64_t value = e->value;
            if (prev) {
                prev->next = e->next;
            } else {
                h->buckets[bucket_idx] = e->next;
            }
            free(e);
            h->len -= 1;
            *(int32_t *)result = 1; /* Some */
            result[1] = value;
            return result;
        }
        prev = e;
        e = e->next;
    }
    *(int32_t *)result = 0; /* None */
    return result;
}

/* HashMap.clear — remove every entry. Frees per-bucket entry chains
   then resets `len` to 0 and clears the string-key flag so the next
   inserted key can re-decide. The spine itself is preserved. */
void riven_hash_clear(RivenHash *h) {
    if (!h) return;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        RivenHashEntry *e = h->buckets[i];
        while (e) {
            RivenHashEntry *nx = e->next;
            free(e);
            e = nx;
        }
        h->buckets[i] = NULL;
    }
    h->len = 0;
    h->string_keys = -1;
}

/* HashMap.keys -> Vec[&K]
   Returns a freshly-allocated Vec[K] containing each key. v1 does
   not distinguish &K from K at the runtime layer (both 8 bytes),
   so this is a flat slot copy. Iteration order matches bucket
   traversal order — callers must not rely on it (per prompt §44). */
RivenVec *riven_hash_keys(RivenHash *h) {
    RivenVec *out = riven_vec_new();
    if (!h) return out;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        for (RivenHashEntry *e = h->buckets[i]; e; e = e->next) {
            riven_vec_push(out, e->key);
        }
    }
    return out;
}

/* HashMap.values -> Vec[&V] — symmetric to `keys`. */
RivenVec *riven_hash_values(RivenHash *h) {
    RivenVec *out = riven_vec_new();
    if (!h) return out;
    for (uint64_t i = 0; i < h->bucket_count; i++) {
        for (RivenHashEntry *e = h->buckets[i]; e; e = e->next) {
            riven_vec_push(out, e->value);
        }
    }
    return out;
}

/* HashMap.iter -> Vec[K] — v1 ships an eager iterator (lazy iter
   lands in #05). Returns the keys list; callers using `for k in
   m.iter` get the same shape they would from a real iterator since
   the v1 `for` lowering already accepts a `Vec` directly. */
RivenVec *riven_hash_iter(RivenHash *h) {
    return riven_hash_keys(h);
}

/* HashMap == HashMap — pairwise key/value equality. Mirrors
   `riven_vec_eq` in spirit: returns 1 iff both maps have the same
   length and every key in `a` maps to a value structurally equal
   (bitwise on slot ints; for String values the slot pointers are
   compared, which is correct only for interned-pointer use cases
   today — full structural value-eq lands with the trait dispatch
   in #05, the same caveat documented on `riven_vec_eq`). */
int8_t riven_hash_eq(RivenHash *a, RivenHash *b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    if (a->len != b->len) return 0;
    for (uint64_t i = 0; i < a->bucket_count; i++) {
        for (RivenHashEntry *e = a->buckets[i]; e; e = e->next) {
            if (!riven_hash_contains_key(b, e->key)) return 0;
            uint64_t bj = riven_hash_key_hash(b, e->key) % b->bucket_count;
            int matched = 0;
            for (RivenHashEntry *f = b->buckets[bj]; f; f = f->next) {
                if (riven_hash_keys_equal(b, f->key, e->key)) {
                    if (f->value != e->value) return 0;
                    matched = 1;
                    break;
                }
            }
            if (!matched) return 0;
        }
    }
    return 1;
}

/* HashMap[&K] — panicking indexed read used by IndexOp lowering for
   HashMap receivers. Mirrors `riven_vec_get_or_panic`: missing key
   triggers a riven panic. Returns the value slot directly (raw 8
   bytes), not an Option. */
int64_t riven_hash_index(RivenHash *h, int64_t key) {
    if (!h) {
        riven_panic("hashmap index: missing key");
    }
    uint64_t bucket_idx = riven_hash_key_hash(h, key) % h->bucket_count;
    for (RivenHashEntry *e = h->buckets[bucket_idx]; e; e = e->next) {
        if (riven_hash_keys_equal(h, e->key, key)) {
            return e->value;
        }
    }
    riven_panic("hashmap index: missing key");
    return 0; /* unreachable */
}

/* ── HashSet[T] surface — Phase 2 stdlib (#04) ──────────────────── */

/* HashSet.with_capacity(Int) — capacity hint, see HashMap.with_capacity. */
RivenSet *riven_set_with_capacity(int64_t cap) {
    (void)cap;
    return riven_set_new();
}

/* HashSet.remove(&T) -> Bool — true iff the element was present.
   Reuses `riven_hash_remove` for the unlink work, then collapses the
   resulting Option to a Bool via the tag word. */
int8_t riven_set_remove(RivenSet *s, int64_t item) {
    if (!s) return 0;
    void *opt = riven_hash_remove(&s->inner, item);
    int8_t was_present = (*(int32_t *)opt) == 1 ? 1 : 0;
    riven_dealloc(opt);
    return was_present;
}

/* HashSet.clear — release every entry, reset len, mirror
   `riven_hash_clear`. */
void riven_set_clear(RivenSet *s) {
    if (!s) return;
    riven_hash_clear(&s->inner);
}

/* HashSet.iter -> Vec[&T] — v1 eager iterator, see HashMap.iter. */
RivenVec *riven_set_iter(RivenSet *s) {
    if (!s) return riven_vec_new();
    return riven_hash_keys(&s->inner);
}

/* HashMap.from_iter(iter[(K, V)]) / iter.collect[HashMap[K, V]].
   Each iter slot is a heap-allocated 2-tuple with field0 at +0 and
   field1 at +8, matching `riven_vec_zip`'s tuple layout and the
   compiler's generic tuple allocation rule. */
RivenHash *riven_hash_from_iter(RivenVec *iter) {
    RivenHash *out = riven_hash_new();
    if (!iter) return out;
    for (uint64_t i = 0; i < iter->len; i++) {
        int64_t *pair = (int64_t *)iter->data[i];
        if (!pair) continue;
        riven_hash_insert(out, pair[0], pair[1]);
    }
    return out;
}

/* HashSet.from_iter(iter[T]) / iter.collect[HashSet[T]]. */
RivenSet *riven_set_from_iter(RivenVec *iter) {
    RivenSet *out = riven_set_new();
    if (!iter) return out;
    for (uint64_t i = 0; i < iter->len; i++) {
        riven_set_insert(out, iter->data[i]);
    }
    return out;
}

/* HashSet.union(&Self) -> HashSet[T]
   Returns a freshly-allocated HashSet containing every element of
   `a` and every element of `b`. Both inputs are borrowed (insert
   semantics dedup). The new set is registered as a fresh-alloc
   callee in mir/lower.rs::FRESH_ALLOC_CALLEES so its lifetime is
   the caller's drop frame. */
RivenSet *riven_set_union(RivenSet *a, RivenSet *b) {
    RivenSet *out = riven_set_new();
    if (a) {
        for (uint64_t i = 0; i < a->inner.bucket_count; i++) {
            for (RivenHashEntry *e = a->inner.buckets[i]; e; e = e->next) {
                riven_set_insert(out, e->key);
            }
        }
    }
    if (b) {
        for (uint64_t i = 0; i < b->inner.bucket_count; i++) {
            for (RivenHashEntry *e = b->inner.buckets[i]; e; e = e->next) {
                riven_set_insert(out, e->key);
            }
        }
    }
    return out;
}

/* HashSet.intersection(&Self) -> HashSet[T] */
RivenSet *riven_set_intersection(RivenSet *a, RivenSet *b) {
    RivenSet *out = riven_set_new();
    if (!a || !b) return out;
    for (uint64_t i = 0; i < a->inner.bucket_count; i++) {
        for (RivenHashEntry *e = a->inner.buckets[i]; e; e = e->next) {
            if (riven_set_contains(b, e->key)) {
                riven_set_insert(out, e->key);
            }
        }
    }
    return out;
}

/* HashSet.difference(&Self) -> HashSet[T] — elements in `a` not in `b`. */
RivenSet *riven_set_difference(RivenSet *a, RivenSet *b) {
    RivenSet *out = riven_set_new();
    if (!a) return out;
    for (uint64_t i = 0; i < a->inner.bucket_count; i++) {
        for (RivenHashEntry *e = a->inner.buckets[i]; e; e = e->next) {
            if (!b || !riven_set_contains(b, e->key)) {
                riven_set_insert(out, e->key);
            }
        }
    }
    return out;
}

/* HashSet == HashSet — same length + every element of `a` is in `b`. */
int8_t riven_set_eq(RivenSet *a, RivenSet *b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    if (a->inner.len != b->inner.len) return 0;
    for (uint64_t i = 0; i < a->inner.bucket_count; i++) {
        for (RivenHashEntry *e = a->inner.buckets[i]; e; e = e->next) {
            if (!riven_set_contains(b, e->key)) return 0;
        }
    }
    return 1;
}

/* ── &str Operations ──────────────────────────────────────────────── */

RivenVec *riven_str_split(const char *s, const char *delimiter) {
    RivenVec *result = riven_vec_new();
    if (!s) return result;
    if (!delimiter || delimiter[0] == '\0') {
        riven_vec_push(result, (int64_t)riven_string_from(s));
        return result;
    }
    size_t dlen = strlen(delimiter);
    const char *start = s;
    while (1) {
        const char *found = strstr(start, delimiter);
        if (!found) {
            riven_vec_push(result, (int64_t)riven_string_from(start));
            break;
        }
        size_t part_len = (size_t)(found - start);
        char *part = (char *)malloc(part_len + 1);
        if (!part) {
            riven_panic("out of memory");
        }
        memcpy(part, start, part_len);
        part[part_len] = '\0';
        riven_vec_push(result, (int64_t)part);
        start = found + dlen;
    }
    return result;
}

/* Parse a string to an unsigned integer, returning a Result-like value.
   Returns a tagged union: tag=0 (Ok) with value, tag=1 (Err). */
void *riven_str_parse_uint(const char *s) {
    /* Allocate a tagged union: [tag:i32 pad:i32 payload:i64] = 16 bytes */
    int64_t *result = (int64_t *)riven_alloc(16);
    if (!s || *s == '\0') {
        *(int32_t *)result = 1; /* Err */
        return result;
    }
    char *end;
    unsigned long val = strtoul(s, &end, 10);
    if (*end != '\0') {
        *(int32_t *)result = 1; /* Err */
    } else {
        *(int32_t *)result = 0; /* Ok */
        result[1] = (int64_t)val;
    }
    return result;
}

/* ── Option / Result Helpers ──────────────────────────────────────── */

/* Option unwrap_or: if tag==0 (None), return default_val; if tag==1 (Some), return payload */
int64_t riven_option_unwrap_or(void *opt, int64_t default_val) {
    if (!opt) return default_val;
    int32_t tag = *(int32_t *)opt;
    if (tag == 0) return default_val; /* None */
    return ((int64_t *)opt)[1]; /* Some(payload) */
}

/* Result unwrap_or_else: if Ok (tag 0), return payload. If Err, call handler. */
int64_t riven_result_unwrap_or_else(void *result, void (*handler)(int64_t)) {
    if (!result) return 0;
    int32_t tag = *(int32_t *)result;
    if (tag == 0) return ((int64_t *)result)[1]; /* Ok */
    /* Err — call handler with error payload if handler is non-null */
    if (handler) {
        int64_t err_payload = ((int64_t *)result)[1];
        handler(err_payload);
    }
    return 0;
}

/* Result try_op (? operator): if Ok, return payload. If Err, propagate. */
int64_t riven_result_try_op(void *result) {
    if (!result) return 0;
    int32_t tag = *(int32_t *)result;
    if (tag == 0) return ((int64_t *)result)[1]; /* Ok */
    /* Err — in a real implementation, this would propagate via a return.
       For now, just return 0. */
    return 0;
}

/* Result expect!(msg): if Ok, return payload; if Err, panic with `msg`. */
int64_t riven_result_expect(void *result, const char *msg) {
    if (!result) riven_panic(msg ? msg : "expect! on null");
    int32_t tag = *(int32_t *)result;
    if (tag == 0) return ((int64_t *)result)[1]; /* Ok */
    riven_panic(msg ? msg : "expect! on Err");
    return 0; /* unreachable */
}

/* Result unwrap!: if Ok, return payload; if Err, panic. */
int64_t riven_result_unwrap(void *result) {
    if (!result) riven_panic("unwrap! on null");
    int32_t tag = *(int32_t *)result;
    if (tag == 0) return ((int64_t *)result)[1]; /* Ok */
    riven_panic("unwrap! on Err");
    return 0; /* unreachable */
}

/* Option expect!(msg): if Some, return payload; if None, panic with `msg`. */
int64_t riven_option_expect(void *opt, const char *msg) {
    if (!opt) riven_panic(msg ? msg : "expect! on null");
    int32_t tag = *(int32_t *)opt;
    if (tag == 1) return ((int64_t *)opt)[1]; /* Some */
    riven_panic(msg ? msg : "expect! on None");
    return 0; /* unreachable */
}

/* Option unwrap!: if Some, return payload; if None, panic. */
int64_t riven_option_unwrap(void *opt) {
    if (!opt) riven_panic("unwrap! on null");
    int32_t tag = *(int32_t *)opt;
    if (tag == 1) return ((int64_t *)opt)[1]; /* Some */
    riven_panic("unwrap! on None");
    return 0; /* unreachable */
}

/* Result ok(): Result[T,E] -> Option[T]. Ok(x) -> Some(x); Err(_) -> None. */
void *riven_result_ok(void *result) {
    int64_t *out = (int64_t *)riven_alloc(16);
    if (result && *(int32_t *)result == 0) {
        *(int32_t *)out = 1; /* Some */
        out[1] = ((int64_t *)result)[1];
    } else {
        *(int32_t *)out = 0; /* None */
    }
    return out;
}

/* Result err(): Result[T,E] -> Option[E]. Err(e) -> Some(e); Ok(_) -> None. */
void *riven_result_err(void *result) {
    int64_t *out = (int64_t *)riven_alloc(16);
    if (result && *(int32_t *)result == 1) {
        *(int32_t *)out = 1; /* Some */
        out[1] = ((int64_t *)result)[1];
    } else {
        *(int32_t *)out = 0; /* None */
    }
    return out;
}

/* Result unwrap_or(): Ok(v) -> v; Err(_) -> default. No closure variant
   (use unwrap_or_else for that). */
int64_t riven_result_unwrap_or(void *result, int64_t default_val) {
    if (result && *(int32_t *)result == 0) {
        return ((int64_t *)result)[1];
    }
    return default_val;
}

/* Option ok_or(): Some(v) -> Ok(v); None -> Err(err_value). The error
   value is supplied eagerly; for the closure-driven variant use
   `ok_or_else` (not yet implemented). */
void *riven_option_ok_or(void *opt, int64_t err_value) {
    int64_t *out = (int64_t *)riven_alloc(16);
    if (opt && *(int32_t *)opt == 1) {
        *(int32_t *)out = 0; /* Ok */
        out[1] = ((int64_t *)opt)[1];
    } else {
        *(int32_t *)out = 1; /* Err */
        out[1] = err_value;
    }
    return out;
}

/* Option / Result is_* predicates (return i8). */
int8_t riven_option_is_some(void *opt) {
    return opt && *(int32_t *)opt == 1;
}
int8_t riven_option_is_none(void *opt) {
    return !opt || *(int32_t *)opt == 0;
}
int8_t riven_result_is_ok(void *result) {
    return result && *(int32_t *)result == 0;
}
int8_t riven_result_is_err(void *result) {
    return !result || *(int32_t *)result == 1;
}

/* ── No-op Stubs ──────────────────────────────────────────────────── */

/* Pass through the first argument unchanged (for iterator wrappers etc.) */
int64_t riven_noop_passthrough(int64_t val) {
    return val;
}

/* Return null (for find/position that return Option) */
int64_t riven_noop_return_null(void) {
    return 0;
}

void riven_noop(void) {}

/* ── Panic ─────────────────────────────────────────────────────────── */

void riven_panic(const char *message) {
    fflush(stdout);
    fprintf(stderr, "riven panic: %s\n", message ? message : "(unknown)");
    fflush(stderr);
    exit(101);
}
