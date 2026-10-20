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
 *   - `static inline` helpers used in more than one TU: Result and
 *     Option builders, `ruxen_xmalloc`, `RuxenByteBuf`, the UTF-8 codec
 *     and the `RuxenFdHandle` close helpers.
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
#include <stddef.h>
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

/* ── Element descriptor (the collection-slot ownership rule) ──
 *
 * A collection slot is a bare 64-bit word; the runtime cannot tell an
 * `Int` from an owned `char*` by looking at it (and MUST NOT try — see
 * the "do not reintroduce a memory probe" warning above). So every
 * helper that frees or deep-copies slots takes a descriptor from the
 * compiler saying what a slot owns.
 *
 * A descriptor is a stream of 4-bit tags read from the LOW nibble up, in
 * pre-order: a container tag is followed by the descriptor of its
 * element (`VEC`, `SET`) or of its key and then its value (`HASH`).
 * `Array[Array[String]]`'s element descriptor is VEC then CSTR, i.e.
 * 0x13. Tags past the end of the stream read as POD. The compiler
 * builds descriptors in exactly one place (`mir/lower/drops/`
 * `elem_desc`), from the static element type, and the drop and the
 * deep copy read the same word, so copy depth and drop depth cannot
 * disagree.
 *
 * The obligation this exists to discharge is stated in full on
 * `FRESH_ALLOC_CALLEES` (compiler/ruxen_core/src/mir/lower/runtime_abi.rs):
 * a helper whose result will be deep-dropped must have allocated every
 * slot the drop will free.
 *
 * The CSTR and POD values deliberately COINCIDE with the RUXEN_KEY_* key
 * kinds above (see the _Static_asserts below). A Set's element IS its
 * key, so a Set passes its `key_kind` straight through as an element
 * descriptor. Any other key kind (CUSTOM, UNSET) reads as an unknown tag
 * and so as POD, which is what ownership-wise it is. For the same
 * reason the compiler only ever describes a Hash key or a Set element
 * as CSTR or POD: those tables copy and free their keys by `key_kind`. */
#define RUXEN_ELEM_POD   0 /* slot is a value: Int/Float/Bool/Char/&T/… */
#define RUXEN_ELEM_CSTR  1 /* slot OWNS a NUL-terminated heap string    */
#define RUXEN_ELEM_VEC   3 /* slot OWNS a RuxenVec; element desc follows */
#define RUXEN_ELEM_HASH  4 /* slot OWNS a RuxenHash; key, value follow   */
#define RUXEN_ELEM_SET   5 /* slot OWNS a set table; element desc follows */
#define RUXEN_ELEM_OBJ   6 /* slot OWNS a user object; glue address follows */
#define RUXEN_ELEM_CLOSURE 7 /* slot OWNS one share of a closure pair     */

/* `RUXEN_ELEM_CLOSURE` describes a slot holding a closure value: a pointer
 * to the compiler's four-word closure pair (`mir/lower/drops/`,
 * `CLOSURE_PAIR_SIZE`):
 *
 *     [0] fn_ptr  [1] captures_ptr  [2] drop_fn  [3] shares
 *
 * Dropping the slot calls `drop_fn(pair)`, the literal's own drop glue,
 * which releases one owner (`shares` > 0 is decremented; at 0 the owned
 * captures, the captures box and the pair are freed). Copying the slot adds
 * an owner (`shares` + 1) and returns the same pair: closures are shared,
 * never copied. A leaf tag: nothing follows it in the stream.
 *
 * The slot numbers and the pair size below are the C side of the layout;
 * the compiler's `CLOSURE_PAIR_*` constants (`mir/lower/drops/closures.rs`)
 * are the Rust side, and a test there reads these `#define`s back and
 * compares them. */
#define RUXEN_CLOSURE_FN_SLOT       0
#define RUXEN_CLOSURE_CAPTURES_SLOT 1
#define RUXEN_CLOSURE_DROP_SLOT     2
#define RUXEN_CLOSURE_SHARES_SLOT   3
#define RUXEN_CLOSURE_PAIR_SIZE     32
typedef struct {
    int64_t fn_ptr;       /* the body, called with captures_ptr first */
    int64_t captures_ptr; /* one word per capture, or 0 */
    int64_t drop_fn;      /* the literal's own `<closure>_drop` */
    int64_t shares;       /* owners beyond the first */
} RuxenClosurePair;
_Static_assert(offsetof(RuxenClosurePair, fn_ptr) == RUXEN_CLOSURE_FN_SLOT * 8,
               "closure pair: fn_ptr slot");
_Static_assert(offsetof(RuxenClosurePair, captures_ptr) == RUXEN_CLOSURE_CAPTURES_SLOT * 8,
               "closure pair: captures_ptr slot");
_Static_assert(offsetof(RuxenClosurePair, drop_fn) == RUXEN_CLOSURE_DROP_SLOT * 8,
               "closure pair: drop_fn slot");
_Static_assert(offsetof(RuxenClosurePair, shares) == RUXEN_CLOSURE_SHARES_SLOT * 8,
               "closure pair: shares slot");
_Static_assert(sizeof(RuxenClosurePair) == RUXEN_CLOSURE_PAIR_SIZE, "closure pair: size");
typedef void (*RuxenClosureDrop)(int64_t pair);
int64_t ruxen_closure_share(int64_t pair);

/* `RUXEN_ELEM_OBJ` describes a slot holding an instance of a user class or
 * struct. Freeing or copying one needs the type's own code (its `def drop`,
 * its `clone`), so the tag is followed not by more tags but by the address
 * of a compiler-synthesised glue function, filling the rest of the word:
 *
 *     glue(RUXEN_ELEM_GLUE_DROP,  slot) runs the type's drop and frees it;
 *     glue(RUXEN_ELEM_GLUE_CLONE, slot) returns an owned copy of it.
 *
 * OBJ is therefore always the LAST tag of a stream: `RUXEN_ELEM_REST` of an
 * OBJ descriptor is the glue address. The compiler (`mir/lower/drops/`
 * `elem_desc`) emits OBJ only when the address still fits after the
 * preceding tags, and otherwise describes the slot as POD. */
#define RUXEN_ELEM_GLUE_DROP  0
#define RUXEN_ELEM_GLUE_CLONE 1
typedef int64_t (*RuxenElemGlue)(int64_t op, int64_t slot);

#define RUXEN_ELEM_TAG(desc) ((int)((uint64_t)(desc) & 0xFu))
#define RUXEN_ELEM_REST(desc) ((int64_t)((uint64_t)(desc) >> 4))

/* `RUXEN_ELEM_VEC` is 3, not 2, because 2 is RUXEN_KEY_CUSTOM: a Set of
 * user-typed elements passes key_kind through here, and a custom key is
 * a POD word as far as OWNERSHIP is concerned (its hash/eq callbacks say
 * nothing about who frees it). Keeping 2 unassigned is what lets the
 * pass-through be a plain assignment. */
#if defined(__STDC_VERSION__) && __STDC_VERSION__ >= 201112L
_Static_assert(RUXEN_ELEM_POD == RUXEN_KEY_BITS,
               "element-kind/key-kind vocabularies must not drift");
_Static_assert(RUXEN_ELEM_CSTR == RUXEN_KEY_CSTR,
               "element-kind/key-kind vocabularies must not drift");
_Static_assert(RUXEN_ELEM_VEC != RUXEN_KEY_CUSTOM &&
                   RUXEN_ELEM_HASH != RUXEN_KEY_CUSTOM &&
                   RUXEN_ELEM_SET != RUXEN_KEY_CUSTOM &&
                   RUXEN_ELEM_OBJ != RUXEN_KEY_CUSTOM &&
                   RUXEN_ELEM_CLOSURE != RUXEN_KEY_CUSTOM,
               "a container tag must not collide with a pass-through key kind");
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

/* FdHandle — the 8-byte `{fd, closed}` wire layout every fd-owning class
 * shares (File, TcpStream, TcpListener, AsyncFile). `closed` is set once
 * the fd has been released; `fd` is then -1. Close through
 * `ruxen_fd_handle_close` (below) so the EINTR policy lives in one place.
 * io/bufio.c casts `inner` to a `RuxenFile *` / `RuxenTcpStream *` and
 * reads `->fd` / `->closed`, so the body is exported. */
typedef struct RuxenFdHandle {
    int32_t fd;
    int32_t closed;
} RuxenFdHandle;

/* File — owner: library/std/io/runtime/file.c. */
typedef RuxenFdHandle RuxenFile;

/* TcpStream / TcpListener — owner: library/std/net/runtime/tcp.c. */
typedef RuxenFdHandle RuxenTcpStream;
typedef RuxenFdHandle RuxenTcpListener;

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

/* Deep-copy ONE collection slot according to its element descriptor.
 *
 * OWNERSHIP CONTRACT
 *   Allocates: a new heap object for every owning tag in `desc`, all the
 *     way down (a `char*` via `ruxen_string_from` for CSTR, a fresh table
 *     or spine with deep-copied slots for VEC / HASH / SET, the type's own
 *     `clone` through the glue for OBJ). Allocates NOTHING for POD.
 *   Frees: nothing, ever. `slot` is BORROWED for the call's duration and
 *     is still owned by its original collection when this returns.
 *   Caller may assume: the returned word is safe to store in a slot that
 *     will later be freed by `ruxen_elem_drop` with the same descriptor,
 *     WITHOUT disturbing the source.
 *   A NULL slot returns NULL for every kind. An unrecognised tag is
 *     treated as POD, which aliases — so a new heap element kind MUST be
 *     added here and to `ruxen_elem_drop` in the same change. */
int64_t ruxen_elem_clone(int64_t slot, int64_t desc);

/* Free ONE collection slot according to its element descriptor,
 * recursively. The mirror of `ruxen_elem_clone`: a POD or unknown tag
 * frees nothing. Defined in array/runtime/vec.c. */
void ruxen_elem_drop(int64_t slot, int64_t desc);

/* The drop glue the MIR drop pass emits, one per container. Each frees
 * the slots per its descriptor(s) and then the spine. `ruxen_set_drop`
 * lives in hash.c next to the set layout; a set nested in another
 * collection is dropped through `ruxen_hash_drop` with a POD value. */
void ruxen_vec_drop(RuxenVec *v, int64_t elem_desc);
void ruxen_hash_drop(RuxenHash *h, int64_t key_desc, int64_t val_desc);

/* A new table with the same key descriptor as `h` and every entry
 * deep-copied per `key_desc` / `val_desc`. Backs `ruxen_elem_clone` for
 * HASH and SET slots. Defined in hash.c. */
RuxenHash *ruxen_hash_deep_clone(RuxenHash *h, int64_t key_desc,
                                 int64_t val_desc);

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
/* The IoError itself (not wrapped in Result) for a captured errno: the
 * variant and strerror payload `ruxen_io_error_from_errno` boxes into
 * Err. The async runtimes record errno at poll time and build the error
 * when the result is taken, so the message survives. */
void *ruxen_io_error_value_from_errno(int err);
/* errno → RUXEN_IO_ERROR_* tag. The one errno table (io/runtime/io_error.c);
 * the sync and async runtimes both classify through it. */
int32_t ruxen_io_error_classify_errno(int err);

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

/* `Err(IoError::InvalidInput)` — the null-handle / closed-handle / bad
 * argument result every IO entry point returns. */
static inline void *ruxen_io_error_invalid_input(void) {
    return ruxen_result_err_value(
        (int64_t)ruxen_io_error_unit(RUXEN_IO_ERROR_INVALID_INPUT));
}

/* `Err(IoError::WriteZero)` — a write returned 0 with bytes remaining.
 * No errno describes it, so the message is fixed. */
static inline void *ruxen_io_error_write_zero(void) {
    return ruxen_result_err_value((int64_t)ruxen_io_error_struct(
        RUXEN_IO_ERROR_WRITE_ZERO, "write returned 0 with bytes remaining"));
}

/* ── Option-construction helpers ──────────────────────────────────── */

/* Box `Some(payload)` / `None` into the canonical 16-byte tagged-enum
 * layout (`{i32 tag; i32 pad; i64 payload}`, None = 0, Some = 1). Matches
 * the resolver's variant order and `ruxen_option_unwrap_or` in alloc.c.
 * MIR builds Options itself; these are for C callers that return one. */
static inline void *ruxen_option_none(void) {
    int64_t *out = (int64_t *)ruxen_alloc(16);
    *(int32_t *)out = 0; /* None */
    out[1] = 0;
    return out;
}

static inline void *ruxen_option_some(int64_t payload) {
    int64_t *out = (int64_t *)ruxen_alloc(16);
    *(int32_t *)out = 1; /* Some */
    out[1] = payload;
    return out;
}

/* ── Raw heap helpers ─────────────────────────────────────────────── */

/* `malloc` that panics on OOM. The literal `malloc(` / `realloc(` / `free(`
 * spellings below are load-bearing: drop_fixtures rewrites them textually
 * to count raw heap traffic, so they must not hide behind a macro. */
static inline void *ruxen_xmalloc(size_t size) {
    void *p = malloc(size);
    if (!p && size > 0) {
        ruxen_panic("out of memory");
    }
    return p;
}

/* Growable byte buffer for read-until-EOF / read-line loops. `data` is
 * a raw `malloc` block the caller owns: hand it off (it is always
 * NUL-terminable, `cap > len`) or release it with `free(b.data)`. */
typedef struct RuxenByteBuf {
    char *data;
    size_t len;
    size_t cap;
} RuxenByteBuf;

static inline void ruxen_byte_buf_init(RuxenByteBuf *b, size_t cap) {
    b->data = (char *)ruxen_xmalloc(cap);
    b->len = 0;
    b->cap = cap;
}

/* Ensure room for `extra` more bytes plus a trailing NUL, doubling. */
static inline void ruxen_byte_buf_reserve(RuxenByteBuf *b, size_t extra) {
    if (b->len + extra + 1 <= b->cap) return;
    size_t next_cap = b->cap;
    while (b->len + extra + 1 > next_cap) next_cap *= 2;
    char *next = (char *)realloc(b->data, next_cap);
    if (!next) {
        free(b->data);
        ruxen_panic("out of memory");
    }
    b->data = next;
    b->cap = next_cap;
}

static inline void ruxen_byte_buf_push(RuxenByteBuf *b, char c) {
    ruxen_byte_buf_reserve(b, 1);
    b->data[b->len++] = c;
}

/* NUL-terminate and return the buffer; ownership passes to the caller. */
static inline char *ruxen_byte_buf_finish(RuxenByteBuf *b) {
    b->data[b->len] = '\0';
    return b->data;
}

/* ── UTF-8 codec ──────────────────────────────────────────────────── */

/* Encode `cp` into `out` and return the byte count (1..4). No range
 * check: callers that accept untrusted codepoints (json) validate first;
 * a Ruxen `Char` is a valid scalar value by construction. */
static inline size_t ruxen_utf8_encode(uint32_t cp, char out[4]) {
    if (cp < 0x80) {
        out[0] = (char)cp;
        return 1;
    }
    if (cp < 0x800) {
        out[0] = (char)(0xC0 | (cp >> 6));
        out[1] = (char)(0x80 | (cp & 0x3F));
        return 2;
    }
    if (cp < 0x10000) {
        out[0] = (char)(0xE0 | (cp >> 12));
        out[1] = (char)(0x80 | ((cp >> 6) & 0x3F));
        out[2] = (char)(0x80 | (cp & 0x3F));
        return 3;
    }
    out[0] = (char)(0xF0 | (cp >> 18));
    out[1] = (char)(0x80 | ((cp >> 12) & 0x3F));
    out[2] = (char)(0x80 | ((cp >> 6) & 0x3F));
    out[3] = (char)(0x80 | (cp & 0x3F));
    return 4;
}

/* Decode one codepoint from the NUL-terminated `p` (`*p != 0`) and store
 * the bytes consumed in `*len`. A malformed or truncated sequence decodes
 * as its lead byte with `*len = 1`, so a walk always makes progress and
 * never reads past the terminator. */
static inline uint32_t ruxen_utf8_decode(const unsigned char *p, size_t *len) {
    unsigned char b0 = p[0];
    if (b0 >= 0x80) {
        if ((b0 & 0xE0) == 0xC0 && (p[1] & 0xC0) == 0x80) {
            *len = 2;
            return ((uint32_t)(b0 & 0x1F) << 6) | (uint32_t)(p[1] & 0x3F);
        }
        if ((b0 & 0xF0) == 0xE0
            && (p[1] & 0xC0) == 0x80 && (p[2] & 0xC0) == 0x80) {
            *len = 3;
            return ((uint32_t)(b0 & 0x0F) << 12)
                 | ((uint32_t)(p[1] & 0x3F) << 6)
                 |  (uint32_t)(p[2] & 0x3F);
        }
        if ((b0 & 0xF8) == 0xF0
            && (p[1] & 0xC0) == 0x80 && (p[2] & 0xC0) == 0x80
            && (p[3] & 0xC0) == 0x80) {
            *len = 4;
            return ((uint32_t)(b0 & 0x07) << 18)
                 | ((uint32_t)(p[1] & 0x3F) << 12)
                 | ((uint32_t)(p[2] & 0x3F) << 6)
                 |  (uint32_t)(p[3] & 0x3F);
        }
    }
    *len = 1;
    return b0;
}

/* ── fd handles (host only) ───────────────────────────────────────── */

#if !defined(__wasm32__)
/* close(2), retried while it fails with EINTR. Returns close's final
 * result; on -1, errno holds the cause. The retry is the runtime's
 * long-standing policy on every target (Linux releases the fd even when
 * close reports EINTR, so a retry there can only fail with EBADF). */
static inline int ruxen_fd_close_retry(int fd) {
    int rc;
    do {
        rc = close(fd);
    } while (rc < 0 && errno == EINTR);
    return rc;
}

/* Release `h`'s fd once: a no-op returning 0 when already closed,
 * otherwise closes it and marks the handle closed (`fd = -1`) whether or
 * not close(2) succeeded. Returns close's result; errno holds the cause
 * on -1. */
static inline int ruxen_fd_handle_close(RuxenFdHandle *h) {
    if (h->closed) return 0;
    int rc = h->fd >= 0 ? ruxen_fd_close_retry(h->fd) : 0;
    h->closed = 1;
    h->fd = -1;
    return rc;
}
#endif

#endif /* RUXEN_RUNTIME_H */
