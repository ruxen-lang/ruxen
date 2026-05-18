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

/* Phase 2 #06.5 T8: per-platform CSPRNG headers for std::rand. The
 * unity build pulls these in at the top so the inner io/rand.c carve-
 * out can call the syscalls without redeclaring them. macOS additionally
 * needs `-framework Security` at link time (added in
 * codegen/object.rs::linker_args under cfg!(target_os = "macos")). */
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


/* ── Module includes (unity build) ─────────────────────────────────────
 *
 * Phase B of #06.75 split this 5800-LOC file into per-module carve-outs
 * under library/runtime/{core,io,net}/ plus top-level fs.c, process.c,
 * etc.  This file remains a single translation unit — every per-module
 * file is `#include`d below in the same definition order the original
 * file used, so `static` symbols defined in one carve-out remain
 * visible to every other carve-out at compile time.
 *
 * Order matters:
 *   1. core/alloc.c     riven_alloc / riven_free / riven_panic — every
 *                       other module calls these.
 *   2. core/vec.c       Vec primitives — string/hash/file/fs/iter use them.
 *   3. core/string.c    String ops — depends on vec.c for owned-byte storage.
 *   4. core/hash.c      Hash + HashMap[K,V] + HashSet[T].
 *   5. io/io_error.c    IoError tagged enum — every IO-returning fn uses it.
 *   6. io/stdio.c       Printing + Stdout/Stderr convenience.
 *   7. io/file.c        File / OpenOptions / SeekFrom.
 *   8. fs.c             Filesystem operations.
 *   9. time.c           Duration / Instant / thread sleep.
 *  10. net/tcp.c        std::net — TcpListener / TcpStream / Shutdown.
 *                       Must come AFTER time.c because
 *                       TcpStream.set_{read,write}_timeout
 *                       dereferences a RivenDuration pointer.
 *  11. signal.c         SIGINT handler.
 *  12. process.c        Command builder + ExitStatus + Output.
 *  13. fmt.c            RivenFormatter.
 *  14. env.c            env::args / env::var / env::vars / env::current_dir.
 *
 * Adding a new module file means adding it here AND keeping the order
 * consistent with its callers' positions above.
 * --------------------------------------------------------------------- */

#include "core/alloc.c"
#include "core/string.c"
#include "core/vec.c"
#include "core/hash.c"
#include "io/io_error.c"
#include "io/stdio.c"
#include "fs.c"
#include "io/file.c"
/* Phase 2 #06.5 T8: std::rand — CSPRNG-backed random_bytes /
 * random_u64 / random_fill. Placed after io/file.c since it reuses
 * the same RivenVec / IoError plumbing; no Duration dependency, so
 * it can sit before time.c. */
#include "io/rand.c"
/* time.c must precede net/tcp.c — Phase 2 #06.5 T5 added
 * TcpStream.set_read_timeout / set_write_timeout, which dereference
 * a RivenDuration pointer (defined in time.c). */
#include "time.c"
#include "net/tcp.c"
#include "signal.c"
#include "process.c"
#include "fmt.c"
#include "env.c"
