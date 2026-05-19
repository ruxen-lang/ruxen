/* Riven stdlib C runtime — shared header.
 *
 * After #06.95 Phase B-2 each stdlib package's `.c` files are
 * standalone translation units (`cc -c <file>.c -o <file>.o`). This
 * header is the cross-package surface every `.c` file `#include`s to
 * see:
 *
 *   - System headers used by any runtime file.
 *   - Platform macros (RIVEN_ASM_LABEL, MSG_NOSIGNAL fallback).
 *   - Struct definitions for types passed across TU boundaries
 *     (RivenVec, RivenFile, RivenTcpStream, RivenDuration, …).
 *   - Forward declarations of `riven_*` symbols any cross-file caller
 *     resolves at link time (riven_panic, riven_alloc,
 *     riven_io_error_from_errno, …).
 *   - `static inline` helpers used in more than one TU
 *     (riven_result_ok_value / _err_value).
 *
 * Owner files (e.g. `library/std/io/runtime/file.c` for RivenFile)
 * still implement these symbols; this header only declares them. The
 * include path from a sibling package is `../../core/runtime/runtime.h`;
 * core's own `.c` files use `"runtime.h"` since they sit next to this
 * header.
 */

#ifndef RIVEN_RUNTIME_H
#define RIVEN_RUNTIME_H

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
    "Riven requires a 64-bit platform (sizeof(void*) must equal sizeof(int64_t))");

_Static_assert(sizeof(void *) == 8,
    "Riven requires 64-bit pointers");

/* `_ORIG_FREE`-style asm-label trick: the drop_fixtures textual
 * splice rewrites every `free(` callsite to `riven_test_free(`. To
 * keep our public riven_*_free symbol names intact, we declare
 * forward decls under an `_ORIG_FREE` sentinel identifier and pin
 * the link symbol via `__asm__`. macOS clang requires the asm label
 * to be visible on the forward declaration BEFORE any caller takes
 * the symbol's address (linux gcc tolerates the late label). */
#if defined(__APPLE__)
#  define RIVEN_ASM_LABEL(sym) __asm__("_" #sym)
#else
#  define RIVEN_ASM_LABEL(sym) __asm__(#sym)
#endif

/* ── Core types passed across TU boundaries ───────────────────────── */

/* Vec — owning growable int64 array. Owner: library/std/array/runtime/vec.c.
 * Cross-file consumers (file.c, fs.c, process.c, env.c, …) access
 * `->data`, `->len`, `->cap`, so the full body is exported. */
typedef struct RivenVec {
    int64_t *data;
    uint64_t len;
    uint64_t cap;
} RivenVec;

/* Hash — opaque to non-owning TUs. Owner: library/std/hash/runtime/hash.c.
 * Bodies inside hash.c access fields directly; cross-file callers use
 * only `RivenHash *` and the riven_hash_* functions. */
typedef struct RivenHash RivenHash;

/* RivenSet is fully package-local to library/std/hash/runtime/hash.c
 * — no other TU dereferences or even references it. The typedef
 * stays inside hash.c. */

/* Formatter — Display/Debug buffer. Owner: library/std/fmt/runtime/fmt.c.
 * Only fmt.c dereferences fields; everyone else holds opaque pointers. */
typedef struct RivenFormatter RivenFormatter;

/* File — wraps an OS fd. Owner: library/std/io/runtime/file.c.
 * io/bufio.c casts `inner` to `RivenFile *` and reads `->fd` /
 * `->closed`, so the full body is exported. */
typedef struct RivenFile {
    int32_t fd;
    int32_t closed;
} RivenFile;

/* TcpStream — same shape as RivenFile (just a different tag class).
 * io/bufio.c also reads `->fd` / `->closed` on this. */
typedef struct RivenTcpStream {
    int32_t fd;
    int32_t closed;
} RivenTcpStream;

/* TcpListener — only net/tcp.c dereferences; declare as opaque
 * cross-file. Body in net/tcp.c. */
typedef struct RivenTcpListener RivenTcpListener;

/* Duration — single-field scalar wrapper. Owner: time/time.c.
 * net/tcp.c reads `->nanos` when wiring SO_RCVTIMEO/SO_SNDTIMEO. */
typedef struct RivenDuration {
    int64_t nanos;
} RivenDuration;

/* Instant — monotonic timestamp. Owner: time/time.c. Opaque
 * cross-file (all manipulation goes through riven_instant_*). */
typedef struct RivenInstant RivenInstant;

/* ── Core memory + panic surface ──────────────────────────────────── */

void riven_panic(const char *message);
void *riven_alloc(uint64_t size);
void riven_dealloc(void *ptr);
void *riven_realloc(void *ptr, uint64_t size);

/* ASM-labeled free helpers — drop_fixtures rewrites `free(` to
 * `riven_test_free(`, so the public link symbols carry the original
 * names through the `_ORIG_FREE` sentinel. */
void riven_vec_ORIG_FREE(RivenVec *v) RIVEN_ASM_LABEL(riven_vec_free);
void riven_string_ORIG_FREE(char *s) RIVEN_ASM_LABEL(riven_string_free);
void riven_fmt_formatter_ORIG_FREE(RivenFormatter *f) RIVEN_ASM_LABEL(riven_fmt_formatter_free);
void riven_hash_free(RivenHash *h);
/* riven_set_free is hash.c-internal in cross-TU surface terms — only
 * hash.c references RivenSet. The function exists as a link symbol
 * but no other TU calls it. */

/* ── String surface used by many ─────────────────────────────────── */

char *riven_string_from(const char *s);
char *riven_char_to_string(int64_t codepoint);

/* ── Vec surface used by many ────────────────────────────────────── */

RivenVec *riven_vec_new(void);
void riven_vec_push(RivenVec *v, int64_t item);

/* ── Hash surface ─────────────────────────────────────────────────── */

RivenHash *riven_hash_new(void);
void riven_hash_insert(RivenHash *h, int64_t key, int64_t value);

/* ── IO error helpers (cross-package) ──────────────────────────────── */

/* IoError tag values. Variant order MUST match
 * library/std/io/src/lib.rvn (each variant's tag = its zero-based
 * position). Pinned by `io_error_tag_stability` test. */
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

/* Metadata kind tags (consumed by io/runtime/file.c's
 * `riven_file_metadata`; declared in fs/runtime/fs.c). */
#define RIVEN_METADATA_KIND_FILE    0
#define RIVEN_METADATA_KIND_DIR     1
#define RIVEN_METADATA_KIND_SYMLINK 2
#define RIVEN_METADATA_KIND_OTHER   3

void *riven_io_error_unit(int32_t tag);
void *riven_io_error_message(const char *message);
void *riven_io_error_struct(int32_t tag, const char *message);
void *riven_io_error_from_errno(int err);

/* Stream helpers (defined in io/runtime/io_error.c, called from io/stdio.c
 * and fs.c). */
void *riven_stream_handle(FILE *stream);
FILE *riven_stream_from_handle(void *handle, FILE *fallback);
void *riven_stream_read_line(FILE *stream);
void *riven_stream_read_to_string(FILE *stream);

/* String helpers cross-package (defined in string/string.c, hash/hash.c). */
char *riven_string_concat(const char *a, const char *b);
uint64_t riven_hash_str(const char *s);

/* Env saved-argv state (defined in io/io_error.c — really env state,
 * historically colocated). env.c references these. */
extern int riven_saved_argc;
extern char **riven_saved_argv;
void riven_free_saved_argv(void);

/* ── Thread / time helpers ────────────────────────────────────────── */

void riven_thread_sleep_ns(int64_t ns);

/* ── Result-construction helpers ──────────────────────────────────── */

/* Box an Ok value into the canonical 16-byte tagged-enum layout
 * (`{i32 tag=0; i32 pad; i64 payload}`). `static inline` so every
 * including TU gets its own copy without linker conflicts. */
static inline void *riven_result_ok_value(int64_t payload) {
    int64_t *result = (int64_t *)riven_alloc(16);
    *(int32_t *)result = 0; /* Ok */
    result[1] = payload;
    return result;
}

/* Box an Err value into the canonical 16-byte tagged-enum layout
 * (`{i32 tag=1; i32 pad; i64 payload}`). */
static inline void *riven_result_err_value(int64_t payload) {
    int64_t *result = (int64_t *)riven_alloc(16);
    *(int32_t *)result = 1; /* Err */
    result[1] = payload;
    return result;
}

#endif /* RIVEN_RUNTIME_H */
