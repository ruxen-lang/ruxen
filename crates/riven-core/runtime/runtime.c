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
char *riven_char_to_string(int64_t codepoint);
typedef struct RivenVec RivenVec;
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

static int32_t riven_io_error_classify_errno(int saved_errno) {
    switch (saved_errno) {
        case ENOENT:  return RIVEN_IO_ERROR_NOT_FOUND;
        case EACCES:
        case EPERM:   return RIVEN_IO_ERROR_PERMISSION_DENIED;
        case EEXIST:  return RIVEN_IO_ERROR_ALREADY_EXISTS;
        case EINTR:   return RIVEN_IO_ERROR_INTERRUPTED;
#ifdef EAGAIN
        case EAGAIN:  return RIVEN_IO_ERROR_WOULD_BLOCK;
#endif
        case EINVAL:  return RIVEN_IO_ERROR_INVALID_INPUT;
        case EPIPE:   return RIVEN_IO_ERROR_BROKEN_PIPE;
        default:      return RIVEN_IO_ERROR_OTHER;
    }
}

/* Build a Result::Err(IoError) from a user-supplied message. The
 * resulting variant is always `Other(message)`. Call sites without a
 * meaningful errno (EOF, env-var-not-found, …) use this helper. */
static void *riven_io_error_message(const char *message) {
    return riven_result_err_value((int64_t)riven_io_error_other(message));
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
    return riven_result_err_value((int64_t)riven_io_error_unit(tag));
}

/* `IoError.message() -> String`. Wired in `codegen/runtime.rs`
 * (`"IoError_message" -> "riven_io_error_get_message"`). Returns a
 * heap-allocated String pointer (interned static for unit variants;
 * the captured payload for `Other`). */
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
        case RIVEN_IO_ERROR_OTHER: {
            char *msg = (char *)((int64_t *)err)[1];
            return msg ? msg : riven_string_from("io error");
        }
        default:
            return riven_string_from("io error");
    }
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

/* Simple Vec: { int64_t *data; uint64_t len; uint64_t cap; } */
struct RivenVec {
    int64_t *data;
    uint64_t len;
    uint64_t cap;
};

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

#define RIVEN_HASH_BUCKETS 16u

struct RivenHash {
    RivenHashEntry *buckets[RIVEN_HASH_BUCKETS];
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
        RivenHashEntry *e = h->buckets[i];
        while (e) {
            RivenHashEntry *nx = e->next;
            free(e);
            e = nx;
        }
    }
    free(h);
}

/* ── HashMap[String, V] / [K, String] / [String, String] per-element
 * drop helpers — Phase 2 stdlib (#04 batch 2).
 *
 * These walk the bucket chains BEFORE the spine free, releasing the
 * heap-owned key (or value, or both) for each entry. The drop
 * selector in `mir/lower.rs::insert_drops` dispatches on `Ty::HashMap(K, V)`
 * to pick the right one based on K/V being heap-owning. The
 * `_ORIG_FREE` sentinel naming pattern (see `riven_string_ORIG_FREE`
 * comment ~line 620) keeps the link symbol clean while letting the
 * leak-tracker rewrite `free(` calls inside the body.
 */
void riven_hash_drop_string_v_ORIG_FREE(RivenHash *h)
    RIVEN_ASM_LABEL(riven_hash_drop_string_v);
void riven_hash_drop_string_v_ORIG_FREE(RivenHash *h) {
    if (!h) return;
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
        for (RivenHashEntry *e = h->buckets[i]; e; e = e->next) {
            RivenVec *v = (RivenVec *)e->value;
            if (v) riven_vec_ORIG_FREE(v);
            e->value = 0;
        }
    }
    riven_hash_ORIG_FREE(h);
}

void riven_hash_insert(RivenHash *h, int64_t key, int64_t value) {
    if (!h) return;
    if (h->string_keys < 0) {
        h->string_keys = riven_hash_looks_like_string(key) ? 1 : 0;
    }
    uint64_t bucket_idx = riven_hash_key_hash(h, key) % RIVEN_HASH_BUCKETS;
    RivenHashEntry *e = h->buckets[bucket_idx];
    while (e) {
        if (riven_hash_keys_equal(h, e->key, key)) {
            e->value = value;
            return;
        }
        e = e->next;
    }
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
    uint64_t bucket_idx = riven_hash_key_hash(h, key) % RIVEN_HASH_BUCKETS;
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
    uint64_t bucket_idx = riven_hash_key_hash(h, key) % RIVEN_HASH_BUCKETS;
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
        RivenHashEntry *e = s->inner.buckets[i];
        while (e) {
            RivenHashEntry *nx = e->next;
            free(e);
            e = nx;
        }
    }
    free(s);
}

/* HashSet[String] per-element drop — frees each owned key string before
 * the spine free. Selector dispatched from `mir/lower.rs::insert_drops`
 * for `Ty::Set(Ty::String)`. */
void riven_set_drop_string_ORIG_FREE(RivenSet *s)
    RIVEN_ASM_LABEL(riven_set_drop_string);
void riven_set_drop_string_ORIG_FREE(RivenSet *s) {
    if (!s) return;
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    uint64_t bucket_idx = riven_hash_key_hash(h, key) % RIVEN_HASH_BUCKETS;
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
        for (RivenHashEntry *e = a->buckets[i]; e; e = e->next) {
            if (!riven_hash_contains_key(b, e->key)) return 0;
            uint64_t bj = riven_hash_key_hash(b, e->key) % RIVEN_HASH_BUCKETS;
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
    uint64_t bucket_idx = riven_hash_key_hash(h, key) % RIVEN_HASH_BUCKETS;
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
        for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
            for (RivenHashEntry *e = a->inner.buckets[i]; e; e = e->next) {
                riven_set_insert(out, e->key);
            }
        }
    }
    if (b) {
        for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
    for (unsigned i = 0; i < RIVEN_HASH_BUCKETS; i++) {
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
