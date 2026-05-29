/* Ruxen stdlib C runtime — shared header.
 *
 * After #06.95 Phase B-2 each stdlib package's `.c` files are
 * standalone translation units (`cc -c <file>.c -o <file>.o`). This
 * header is the cross-package surface every `.c` file `#include`s to
 * see:
 *
 *   - System headers used by any runtime file.
 *   - Platform macros (RUXEN_ASM_LABEL, MSG_NOSIGNAL fallback).
 *   - Struct definitions for types passed across TU boundaries
 *     (RuxenVec, RuxenFile, RuxenTcpStream, RuxenDuration, …).
 *   - Forward declarations of `ruxen_*` symbols any cross-file caller
 *     resolves at link time (ruxen_panic, ruxen_alloc,
 *     ruxen_io_error_from_errno, …).
 *   - `static inline` helpers used in more than one TU
 *     (ruxen_result_ok_value / _err_value).
 *
 * Owner files (e.g. `library/std/io/runtime/file.c` for RuxenFile)
 * still implement these symbols; this header only declares them. The
 * include path from a sibling package is `../../core/runtime/runtime.h`;
 * core's own `.c` files use `"runtime.h"` since they sit next to this
 * header.
 */

#ifndef RUXEN_RUNTIME_H
#define RUXEN_RUNTIME_H

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
#include <fcntl.h>
#include <sys/types.h>

#if defined(__linux__)
#  include <sys/random.h>
#elif defined(__APPLE__)
#  include <Security/Security.h>
#endif

/* Linux-only send() flag that suppresses SIGPIPE on a closed peer.
 * macOS / *BSD don't define it; on those platforms we set the
 * per-socket SO_NOSIGPIPE option after creating the fd, so the flag
 * being a no-op (0) is safe. */
#ifndef MSG_NOSIGNAL
#  define MSG_NOSIGNAL 0
#endif

/* ── Platform Assertions ──────────────────────────────────────────── */

_Static_assert(sizeof(void *) == sizeof(int64_t),
    "Ruxen requires a 64-bit platform (sizeof(void*) must equal sizeof(int64_t))");

_Static_assert(sizeof(void *) == 8,
    "Ruxen requires 64-bit pointers");

/* `_ORIG_FREE`-style asm-label trick: the drop_fixtures textual
 * splice rewrites every `free(` callsite to `ruxen_test_free(`. To
 * keep our public ruxen_*_free symbol names intact, we declare
 * forward decls under an `_ORIG_FREE` sentinel identifier and pin
 * the link symbol via `__asm__`. macOS clang requires the asm label
 * to be visible on the forward declaration BEFORE any caller takes
 * the symbol's address (linux gcc tolerates the late label). */
#if defined(__APPLE__)
#  define RUXEN_ASM_LABEL(sym) __asm__("_" #sym)
#else
#  define RUXEN_ASM_LABEL(sym) __asm__(#sym)
#endif

/* ── Core types passed across TU boundaries ───────────────────────── */

/* Vec — owning growable int64 array. Owner: library/std/array/runtime/vec.c.
 * Cross-file consumers (file.c, fs.c, process.c, env.c, …) access
 * `->data`, `->len`, `->cap`, so the full body is exported. */
typedef struct RuxenVec {
    int64_t *data;
    uint64_t len;
    uint64_t cap;
} RuxenVec;

/* Hash — opaque to non-owning TUs. Owner: library/std/hash/runtime/hash.c.
 * Bodies inside hash.c access fields directly; cross-file callers use
 * only `RuxenHash *` and the ruxen_hash_* functions. */
typedef struct RuxenHash RuxenHash;

/* RuxenSet is fully package-local to library/std/hash/runtime/hash.c
 * — no other TU dereferences or even references it. The typedef
 * stays inside hash.c. */

/* Formatter — Display/Debug buffer. Owner: library/std/fmt/runtime/fmt.c.
 * Only fmt.c dereferences fields; everyone else holds opaque pointers. */
typedef struct RuxenFormatter RuxenFormatter;

/* File — wraps an OS fd. Owner: library/std/io/runtime/file.c.
 * io/bufio.c casts `inner` to `RuxenFile *` and reads `->fd` /
 * `->closed`, so the full body is exported. */
typedef struct RuxenFile {
    int32_t fd;
    int32_t closed;
} RuxenFile;

/* TcpStream — same shape as RuxenFile (just a different tag class).
 * io/bufio.c also reads `->fd` / `->closed` on this. */
typedef struct RuxenTcpStream {
    int32_t fd;
    int32_t closed;
} RuxenTcpStream;

/* TcpListener — only net/tcp.c dereferences; declare as opaque
 * cross-file. Body in net/tcp.c. */
typedef struct RuxenTcpListener RuxenTcpListener;

/* Duration — single-field scalar wrapper. Owner: time/time.c.
 * net/tcp.c reads `->nanos` when wiring SO_RCVTIMEO/SO_SNDTIMEO. */
typedef struct RuxenDuration {
    int64_t nanos;
} RuxenDuration;

/* Instant — monotonic timestamp. Owner: time/time.c. Opaque
 * cross-file (all manipulation goes through ruxen_instant_*). */
typedef struct RuxenInstant RuxenInstant;

/* ── REPL replay-suppression flag ────────────────────────────────────
 *
 * The REPL session re-runs prior `let_bindings` + session var
 * mutations on every input so cross-input state persists. To stop
 * non-idempotent side effects (puts, subprocess spawn, fs.write,
 * TcpListener.bind, …) from firing N times, the REPL sets this
 * thread-local flag to 1 around the replay portion of each input's
 * wrapper. Every non-idempotent `ruxen_*` runtime function early-
 * returns a benign value (Ok-unit / null / 0) when the flag is set.
 *
 * Idempotent reads (fs.read*, fs.metadata, fs.canonicalize,
 * fs.read_link, fs.read_dir, env getters) ignore the flag — they
 * don't perturb state and their values are needed for correct
 * replay of let-RHS expressions like `let n = file_size("x.txt")`.
 *
 * Owner: library/std/core/runtime/repl_replay.c.
 *
 * AOT binaries / `ruxen run` / `ruxen build` outputs never set this;
 * the flag stays 0, every wrapped function executes normally, and
 * the only cost is a single TLS load per gated entry point. */
extern __thread int ruxen_repl_is_replaying;
int ruxen_repl_set_replaying(int v);
int ruxen_repl_get_replaying(void);

/* ── Core memory + panic surface ──────────────────────────────────── */

void ruxen_panic(const char *message);
void *ruxen_alloc(uint64_t size);
void ruxen_dealloc(void *ptr);
void *ruxen_realloc(void *ptr, uint64_t size);

/* ASM-labeled free helpers — drop_fixtures rewrites `free(` to
 * `ruxen_test_free(`, so the public link symbols carry the original
 * names through the `_ORIG_FREE` sentinel. */
void ruxen_vec_ORIG_FREE(RuxenVec *v) RUXEN_ASM_LABEL(ruxen_vec_free);
void ruxen_string_ORIG_FREE(char *s) RUXEN_ASM_LABEL(ruxen_string_free);
void ruxen_fmt_formatter_ORIG_FREE(RuxenFormatter *f) RUXEN_ASM_LABEL(ruxen_fmt_formatter_free);
void ruxen_hash_free(RuxenHash *h);
/* ruxen_set_free is hash.c-internal in cross-TU surface terms — only
 * hash.c references RuxenSet. The function exists as a link symbol
 * but no other TU calls it. */

/* ── String surface used by many ─────────────────────────────────── */

char *ruxen_string_from(const char *s);
char *ruxen_char_to_string(int64_t codepoint);

/* ── Vec surface used by many ────────────────────────────────────── */

RuxenVec *ruxen_vec_new(void);
void ruxen_vec_push(RuxenVec *v, int64_t item);

/* ── Hash surface ─────────────────────────────────────────────────── */

RuxenHash *ruxen_hash_new(void);
void ruxen_hash_insert(RuxenHash *h, int64_t key, int64_t value);

/* ── IO error helpers (cross-package) ──────────────────────────────── */

/* IoError tag values. Variant order MUST match
 * library/std/io/src/lib.rx (each variant's tag = its zero-based
 * position). Pinned by `io_error_tag_stability` test. */
#define RUXEN_IO_ERROR_NOT_FOUND          0
#define RUXEN_IO_ERROR_PERMISSION_DENIED  1
#define RUXEN_IO_ERROR_ALREADY_EXISTS     2
#define RUXEN_IO_ERROR_INTERRUPTED        3
#define RUXEN_IO_ERROR_WOULD_BLOCK        4
#define RUXEN_IO_ERROR_INVALID_INPUT      5
#define RUXEN_IO_ERROR_UNEXPECTED_EOF     6
#define RUXEN_IO_ERROR_BROKEN_PIPE        7
#define RUXEN_IO_ERROR_OTHER              8
#define RUXEN_IO_ERROR_CONNECTION_REFUSED 9
#define RUXEN_IO_ERROR_CONNECTION_RESET   10
#define RUXEN_IO_ERROR_CONNECTION_ABORTED 11
#define RUXEN_IO_ERROR_NOT_CONNECTED      12
#define RUXEN_IO_ERROR_ADDR_IN_USE        13
#define RUXEN_IO_ERROR_ADDR_NOT_AVAILABLE 14
#define RUXEN_IO_ERROR_INVALID_DATA       15
#define RUXEN_IO_ERROR_TIMED_OUT          16
#define RUXEN_IO_ERROR_WRITE_ZERO         17
#define RUXEN_IO_ERROR_UNSUPPORTED        18
#define RUXEN_IO_ERROR_OUT_OF_MEMORY      19

/* Metadata kind tags (consumed by io/runtime/file.c's
 * `ruxen_file_metadata`; declared in fs/runtime/fs.c). */
#define RUXEN_METADATA_KIND_FILE    0
#define RUXEN_METADATA_KIND_DIR     1
#define RUXEN_METADATA_KIND_SYMLINK 2
#define RUXEN_METADATA_KIND_OTHER   3

void *ruxen_io_error_unit(int32_t tag);
void *ruxen_io_error_message(const char *message);
void *ruxen_io_error_struct(int32_t tag, const char *message);
void *ruxen_io_error_from_errno(int err);

/* Stream helpers (defined in io/runtime/io_error.c, called from io/stdio.c
 * and fs.c). */
void *ruxen_stream_handle(FILE *stream);
FILE *ruxen_stream_from_handle(void *handle, FILE *fallback);
void *ruxen_stream_read_line(FILE *stream);
void *ruxen_stream_read_to_string(FILE *stream);

/* String helpers cross-package (defined in string/string.c, hash/hash.c). */
char *ruxen_string_concat(const char *a, const char *b);
uint64_t ruxen_hash_str(const char *s);

/* Env saved-argv state (defined in io/io_error.c — really env state,
 * historically colocated). env.c references these. */
extern int ruxen_saved_argc;
extern char **ruxen_saved_argv;
void ruxen_free_saved_argv(void);

/* ── Thread / time helpers ────────────────────────────────────────── */

void ruxen_thread_sleep_ns(int64_t ns);

/* ── Result-construction helpers ──────────────────────────────────── */

/* Box an Ok value into the canonical 16-byte tagged-enum layout
 * (`{i32 tag=0; i32 pad; i64 payload}`). `static inline` so every
 * including TU gets its own copy without linker conflicts. */
static inline void *ruxen_result_ok_value(int64_t payload) {
    int64_t *result = (int64_t *)ruxen_alloc(16);
    *(int32_t *)result = 0; /* Ok */
    result[1] = payload;
    return result;
}

/* Box an Err value into the canonical 16-byte tagged-enum layout
 * (`{i32 tag=1; i32 pad; i64 payload}`). */
static inline void *ruxen_result_err_value(int64_t payload) {
    int64_t *result = (int64_t *)ruxen_alloc(16);
    *(int32_t *)result = 1; /* Err */
    result[1] = payload;
    return result;
}

#endif /* RUXEN_RUNTIME_H */
