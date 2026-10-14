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

#if defined(__wasm32__)
/* wasm32-unknown-unknown has no sysroot — the POSIX headers below don't exist
 * there (tier 4.09). Pull only the freestanding headers clang ships, and declare
 * the small libc surface the heap-core runtime (alloc.c, vec.c, string.c, …) uses;
 * their definitions come from the bundled wasm runtime shim
 * (library/runtime/wasm/wasm_rt.c). The full POSIX set in the #else is for the
 * host-only modules (io/net/fs/process/time), which are never compiled for wasm. */
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdarg.h>
/* POSIX `ssize_t` (normally <sys/types.h>) — string.c uses it for reverse
 * indices. Pointer-width signed integer, matching its native definition. */
typedef ptrdiff_t ssize_t;
void *malloc(size_t);
void free(void *);
void *realloc(void *, size_t);
void *calloc(size_t, size_t);
void *memcpy(void *, const void *, size_t);
void *memmove(void *, const void *, size_t);
void *memset(void *, int, size_t);
int memcmp(const void *, const void *, size_t);
size_t strlen(const char *);
int strcmp(const char *, const char *);
int strncmp(const char *, const char *, size_t);
char *strchr(const char *, int);
char *strstr(const char *, const char *);
void qsort(void *, size_t, size_t, int (*)(const void *, const void *));
/* Formatting / numeric / error-path libc the heap-core runtime (string.c, fmt.c)
 * needs. snprintf + strtod are real (small) impls in the shim; fprintf/exit are
 * stubs (error paths trap). FILE is opaque (only used as `stderr` for fprintf). */
typedef void FILE;
extern FILE *stderr;
int snprintf(char *, size_t, const char *, ...);
int fprintf(FILE *, const char *, ...);
void exit(int);
double strtod(const char *, char **);
long long strtoll(const char *, char **, int);
unsigned long strtoul(const char *, char **, int);
double round(double);
extern int errno;
#ifndef ERANGE
#define ERANGE 34
#endif
/* <inttypes.h> is gated out; string.c uses `"%" PRId64`. On wasm32 int64_t is
 * `long long`, so PRId64 is "lld". */
#ifndef PRId64
#define PRId64 "lld"
#endif
#else
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
#endif

/* Secure-entropy headers (<sys/random.h>, <Security/Security.h>) are
 * intentionally NOT included here. This header is pulled into every
 * runtime translation unit, and <sys/random.h> was added in glibc 2.25
 * — it is absent from older cross-compile sysroots (e.g. the Ubuntu
 * 16.04 image `cross` uses for aarch64-unknown-linux-gnu), which would
 * break compilation of EVERY runtime TU. The sole consumer is
 * library/std/rand/runtime/rand.c, which includes what it needs locally
 * and reaches the kernel CSPRNG via syscall(SYS_getrandom) on Linux. */

/* Linux-only send() flag that suppresses SIGPIPE on a closed peer.
 * macOS / *BSD don't define it; on those platforms we set the
 * per-socket SO_NOSIGPIPE option after creating the fd, so the flag
 * being a no-op (0) is safe. */
#ifndef MSG_NOSIGNAL
#  define MSG_NOSIGNAL 0
#endif

/* ── Platform Assertions ──────────────────────────────────────────── */

/* The runtime stores pointers in `int64_t` slots (see RuxenVec below), which
 * requires 64-bit pointers on native targets. wasm32 is the deliberate
 * exception (tier 4.09): pointers are 32-bit there, but wasm is always
 * little-endian, so a 32-bit pointer reinterpret-cast into a 64-bit slot lives
 * in the low bytes and round-trips back losslessly. Gate the 64-bit assertions
 * out for wasm32 so the core runtime subset compiles for the browser-GUI target.
 * (A proper `slot_t`→`intptr_t` pass is tracked separately; cosmetic on LE wasm.) */
#if !defined(__wasm32__)
_Static_assert(sizeof(void *) == sizeof(int64_t),
    "Ruxen requires a 64-bit platform (sizeof(void*) must equal sizeof(int64_t))");

_Static_assert(sizeof(void *) == 8,
    "Ruxen requires 64-bit pointers");
#endif

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

/* Hash — chained-bucket table. Owner: library/std/hash/runtime/hash.c.
 * Most cross-file callers use only `RuxenHash *` and the ruxen_hash_*
 * functions, but json.c walks the bucket chains directly to stringify
 * and look up a JSON object, so the body lives HERE rather than being
 * duplicated per TU. It used to be copy-pasted into hash.c and json.c
 * behind a shared `RUXEN_HASH_RUNTIME_STRUCTS` guard; the two copies
 * were an ODR hazard, since a layout change in one silently corrupted
 * the other. One definition, no drift.
 *
 * ── The key-kind protocol (B73) ─────────────────────────────────────
 * How a key hashes and compares is a property of its TYPE, which the
 * compiler knows statically. The kind is supplied at construction
 * (`ruxen_hash_new_keyed` and friends) and is fixed for the table's
 * life. It is NEVER inferred from a key's bit pattern: the pre-B73
 * runtime dereferenced the first inserted key as a `char*` looking for
 * a NUL, which SIGSEGV'd on `Hash[Int, V]` keys >= 0x1000, made user
 * types unusable as keys, and made a table's behaviour depend on its
 * insertion order. Do not reintroduce a memory probe under any name. */
typedef struct RuxenHashEntry {
    int64_t key;
    int64_t value;
    struct RuxenHashEntry *next;
} RuxenHashEntry;

/* Key kinds. An OPEN set — a new key representation adds a value here
 * plus a case in hash.c's `ruxen_hash_key_hash` / `_keys_equal`, and
 * never re-derives the kind from the key's bits. `Symbol` (B29c) joins
 * as a plain RUXEN_KEY_BITS key, a symbol being a 64-bit intern id. */
#define RUXEN_KEY_UNSET  (-1) /* no descriptor supplied — insert is a bug */
#define RUXEN_KEY_BITS   0    /* raw 64-bit value: Int/USize/Char/Bool/… */
#define RUXEN_KEY_CSTR   1    /* NUL-terminated C string: FNV-1a + strcmp */
#define RUXEN_KEY_CUSTOM 2    /* user type: compiler-supplied hash + eq */

/* ── Element-kind descriptor (the DERIVED-COLLECTION ownership rule) ──
 *
 * A collection slot is a bare 64-bit word; the runtime cannot tell an
 * `Int` from an owned `char*` by looking at it (and MUST NOT try — see
 * the "do not reintroduce a memory probe" warning above). So any helper
 * that builds a fresh OWNED collection by copying slots out of a
 * borrowed source needs the caller to say what a slot holds.
 *
 * The obligation this exists to discharge is stated in full on
 * `FRESH_ALLOC_CALLEES` (compiler/ruxen_core/src/mir/lower/runtime_abi.rs).
 * In short: drop elaboration gives such a result the element-aware deep
 * drop from `heap_free_callee`, so every slot the result will deep-free
 * must be a slot the helper itself allocated. Copying a source's pointer
 * into a slot the result will free is a double free by construction.
 *
 * The values deliberately COINCIDE with the RUXEN_KEY_* key kinds above
 * (see the _Static_asserts below). A Set's element IS its key, so a Set
 * or Hash passes its existing `key_kind` straight through as an element
 * kind with no translation and no second vocabulary to drift out of
 * sync. A Vec has no descriptor of its own, so the compiler injects one
 * at the call site from the static element type — the same `Ty` ladder
 * that picks the drop helper, so copy-depth and drop-depth cannot
 * disagree. */
#define RUXEN_ELEM_POD   0 /* slot is a value: Int/Float/Bool/Char/&T/… */
#define RUXEN_ELEM_CSTR  1 /* slot OWNS a NUL-terminated heap string    */
#define RUXEN_ELEM_VEC   3 /* slot OWNS a RuxenVec spine (one level)    */

/* `RUXEN_ELEM_VEC` is 3, not 2, because 2 is RUXEN_KEY_CUSTOM: a Set of
 * user-typed elements passes key_kind through here, and a custom key is
 * a POD word as far as OWNERSHIP is concerned (its hash/eq callbacks say
 * nothing about who frees it). Keeping 3 free of that collision is what
 * lets the pass-through be a plain assignment. */
#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(RUXEN_ELEM_POD == RUXEN_KEY_BITS,
               "element-kind/key-kind vocabularies must not drift");
_Static_assert(RUXEN_ELEM_CSTR == RUXEN_KEY_CSTR,
               "element-kind/key-kind vocabularies must not drift");
_Static_assert(RUXEN_ELEM_VEC != RUXEN_KEY_CUSTOM,
               "RUXEN_ELEM_VEC must not collide with a pass-through key kind");
#endif

/* Callbacks for RUXEN_KEY_CUSTOM. The signatures match the MIR
 * functions `mir/lower/derive.rs` synthesizes for a struct that
 * includes `Hashable` / `PartialEq`:
 *   <Struct>_hash_code(self)        -> Int   (i64)
 *   <Struct>_eq(self, other: &Self) -> Bool  (i8)
 * Both take pointer-width key words, which is what the table stores. */
typedef int64_t (*RuxenKeyHashFn)(int64_t);
typedef int8_t (*RuxenKeyEqFn)(int64_t, int64_t);

typedef struct RuxenHash {
    /* Heap-allocated bucket array of length `bucket_count`, doubling
     * when `len` would push the load factor past 0.75. The count is a
     * power of two, so the `% bucket_count` modulo stays clean without
     * biasing the splitmix/FNV distribution. */
    RuxenHashEntry **buckets;
    uint64_t bucket_count;
    uint64_t len;
    /* One of the RUXEN_KEY_* constants. Installed once, at construction,
     * from the static key type. `clear` does NOT reset it: emptying a
     * table does not change what its key type is. */
    int32_t key_kind;
    /* RUXEN_KEY_CUSTOM only; NULL otherwise. */
    RuxenKeyHashFn key_hash;
    RuxenKeyEqFn key_eq;
} RuxenHash;

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
RuxenVec *ruxen_vec_clone(RuxenVec *v);

/* Deep-copy ONE collection slot according to its RUXEN_ELEM_* kind.
 *
 * OWNERSHIP CONTRACT
 *   Allocates: a new heap object for RUXEN_ELEM_CSTR (a `char*` via
 *     `ruxen_string_from`) and RUXEN_ELEM_VEC (a spine + backing array
 *     via `ruxen_vec_clone`). Allocates NOTHING for RUXEN_ELEM_POD.
 *   Frees: nothing, ever. `slot` is BORROWED for the call's duration and
 *     is still owned by its original collection when this returns.
 *   Caller may assume: the returned word is safe to store in a slot that
 *     will later be deep-freed by the matching `heap_free_callee` helper
 *     (`ruxen_vec_drop_string` / `ruxen_vec_drop_vec` / `ruxen_set_drop_string`)
 *     WITHOUT disturbing the source. The copy depth is exactly the depth
 *     those helpers free: one `char*` for CSTR, one spine for VEC. Any
 *     deeper nesting (a `Vec[Vec[String]]`'s inner strings) is aliased by
 *     BOTH — which is sound only because the drop helpers stop at the
 *     same depth. Deepening one side without the other reintroduces the
 *     double free this exists to prevent.
 *   A NULL slot returns NULL for every kind. An unrecognised kind is
 *     treated as POD, which aliases — so a new heap element kind MUST be
 *     added here and to `heap_free_callee` in the same change. */
int64_t ruxen_elem_clone(int64_t slot, int32_t elem_kind);

/* ── Hash surface ─────────────────────────────────────────────────── */

RuxenHash *ruxen_hash_new(void);
RuxenHash *ruxen_hash_new_keyed(int64_t kind, int64_t hash_fn, int64_t eq_fn);
void ruxen_hash_ensure_key_kind(RuxenHash *h, int64_t kind, int64_t hash_fn,
                                int64_t eq_fn);
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
 * and fs.c). FILE-typed → host-only; the io module isn't built for wasm. */
#if !defined(__wasm32__)
void *ruxen_stream_handle(FILE *stream);
FILE *ruxen_stream_from_handle(void *handle, FILE *fallback);
void *ruxen_stream_read_line(FILE *stream);
void *ruxen_stream_read_to_string(FILE *stream);
#endif

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
