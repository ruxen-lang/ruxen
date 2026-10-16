#include "../../core/runtime/runtime.h"

/* Stdin/Stdout/Stderr runtime. A standalone translation unit. */

/* ── Printing ──────────────────────────────────────────────────────── */

void ruxen_puts(const char *s) {
    /* REPL replay-suppression: prior inputs' puts are re-emitted by
     * the cumulative replay; squash them so each call fires once. */
    if (ruxen_repl_is_replaying) return;
    if (s) {
        puts(s);
    } else {
        puts("(nil)");
    }
}

void ruxen_print(const char *s) {
    if (ruxen_repl_is_replaying) return;
    if (s) {
        fputs(s, stdout);
    }
}

void ruxen_eputs(const char *s) {
    if (ruxen_repl_is_replaying) return;
    if (s) {
        fprintf(stderr, "%s\n", s);
    }
}

void *ruxen_read_line(void) {
    return ruxen_stream_read_line(stdin);
}

void *ruxen_stdin(void) {
    return ruxen_stream_handle(stdin);
}

void *ruxen_stdout(void) {
    return ruxen_stream_handle(stdout);
}

void *ruxen_stderr(void) {
    return ruxen_stream_handle(stderr);
}

void *ruxen_stdin_read_line(void *handle) {
    return ruxen_stream_read_line(ruxen_stream_from_handle(handle, stdin));
}

void *ruxen_stdin_read_to_string(void *handle) {
    return ruxen_stream_read_to_string(ruxen_stream_from_handle(handle, stdin));
}

/* Phase 2 stdlib (#06.2): `Stdin.lines() -> Vec[Result[String, IoError]]`.
 *
 * v1 simplification of Rust's `BufRead::lines`: read all of stdin to
 * EOF up front, split on '\n', materialise into a `RuxenVec*` of
 * `Result::Ok(line)` elements. If the read itself fails the returned
 * vec contains a single `Result::Err(IoError(strerror))`. Newlines are
 * stripped from each line; a trailing '\n' does NOT produce a final
 * empty element (matches Rust). Empty input → empty vec. */
void *ruxen_stdin_lines(void *handle) {
    FILE *stream = ruxen_stream_from_handle(handle, stdin);

    RuxenByteBuf bytes;
    ruxen_byte_buf_init(&bytes, 256);
    int ch;

    while ((ch = fgetc(stream)) != EOF) {
        ruxen_byte_buf_push(&bytes, (char)ch);
    }
    char *buf = bytes.data;
    size_t len = bytes.len;

    /* Capture errno BEFORE any other call that might clobber it. The
     * original `errno` from a failed `fgetc` could be overwritten by
     * `ruxen_vec_new` (allocator path) or `clearerr`, so snapshot it
     * here. */
    int saved_errno = errno;

    RuxenVec *v = ruxen_vec_new();

    if (ferror(stream)) {
        free(buf);
        clearerr(stream);
        ruxen_vec_push(v, (int64_t)ruxen_io_error_from_errno(saved_errno));
        return v;
    }

    size_t line_start = 0;
    for (size_t i = 0; i < len; i++) {
        if (buf[i] == '\n') {
            size_t line_len = i - line_start;
            char *line = (char *)ruxen_alloc(line_len + 1);
            memcpy(line, &buf[line_start], line_len);
            line[line_len] = '\0';
            ruxen_vec_push(v, (int64_t)ruxen_result_ok_value((int64_t)line));
            line_start = i + 1;
        }
    }
    if (line_start < len) {
        size_t line_len = len - line_start;
        char *line = (char *)ruxen_alloc(line_len + 1);
        memcpy(line, &buf[line_start], line_len);
        line[line_len] = '\0';
        ruxen_vec_push(v, (int64_t)ruxen_result_ok_value((int64_t)line));
    }

    free(buf);
    return v;
}

/* ── Stdout / Stderr ───────────────────────────────────────────────
 *
 * One implementation for both output handles: the handle carries its
 * own `FILE *`, so `Stdout` and `Stderr` alias these symbols from
 * their `lib` blocks. `stdout` is only the fallback for a NULL handle,
 * which `ruxen_stdout()` / `ruxen_stderr()` never return.
 *
 * `print` / `println` (Stderr: `eprint` / `eprintln`) are the
 * no-Result variants of `write_str`. Failures are silently swallowed
 * (matching the Rust `print!` / `println!` / `eprintln!` macros' "panic
 * on broken pipe" lineage trimmed for v1 simplicity — we just discard
 * the error). The trailing-newline variant emits `\n` after the
 * user-supplied text so the buffer flush behaviour matches what
 * `puts(3)` would do on a stdio stream. */

void *ruxen_stdio_write_str(void *handle, const char *s) {
    if (ruxen_repl_is_replaying) return ruxen_result_ok_value(0);
    FILE *stream = ruxen_stream_from_handle(handle, stdout);
    const char *text = s ? s : "";
    if (fputs(text, stream) == EOF) {
        return ruxen_io_error_from_errno(errno);
    }
    return ruxen_result_ok_value(0);
}

void *ruxen_stdio_flush(void *handle) {
    if (ruxen_repl_is_replaying) return ruxen_result_ok_value(0);
    FILE *stream = ruxen_stream_from_handle(handle, stdout);
    if (fflush(stream) != 0) {
        return ruxen_io_error_from_errno(errno);
    }
    return ruxen_result_ok_value(0);
}

void ruxen_stdio_print(void *handle, const char *s) {
    if (ruxen_repl_is_replaying) return;
    FILE *stream = ruxen_stream_from_handle(handle, stdout);
    if (s) {
        (void)fputs(s, stream);
    }
}

void ruxen_stdio_println(void *handle, const char *s) {
    if (ruxen_repl_is_replaying) return;
    FILE *stream = ruxen_stream_from_handle(handle, stdout);
    if (s) {
        (void)fputs(s, stream);
    }
    (void)fputc('\n', stream);
}
