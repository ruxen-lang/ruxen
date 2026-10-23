#include "../../core/runtime/runtime.h"

/* Formatter runtime for Display/Debug and interpolation format specs.
   A standalone translation unit. */

/* ── std::fmt::Formatter ─────────────────────────────────────────────
 *
 * Phase 2 #06.D2.S0: six runtime helpers that Phase A's CHANGELOG
 * claimed but never landed. Codegen in cranelift.rs and llvm/ already
 * has the signatures wired; these definitions close the link gap.
 *
 * Layout: heap-owned grow-able byte buffer plus reserved spec slots
 * (width/precision/align/fill) for Phase D4.
 */

typedef struct RuxenFormatter {
    char    *buf;
    size_t   len;
    size_t   cap;
    /* Reserved spec slots — populated by Phase D4. */
    int32_t  width;
    int32_t  precision;
    int8_t   align;    /* 0 = default, 1 = left, 2 = center, 3 = right */
    int32_t  fill_cp;  /* UTF-32 codepoint; -1 = default ' ' */
} RuxenFormatter;

/* Grow the buffer so at least `additional` more bytes can be appended. */
static void ruxen_fmt_formatter_reserve(RuxenFormatter *f, size_t additional) {
    size_t needed = f->len + additional + 1; /* +1 for NUL */
    if (needed <= f->cap) return;
    size_t newcap = f->cap == 0 ? 16 : f->cap;
    while (newcap < needed) newcap *= 2;
    char *nb = (char *) realloc(f->buf, newcap);
    if (!nb) { fprintf(stderr, "ruxen: formatter alloc failed\n"); exit(101); }
    f->buf = nb;
    f->cap = newcap;
}

/* Allocate and initialise a fresh Formatter with an empty buffer. */
RuxenFormatter *ruxen_fmt_formatter_new(void) {
    RuxenFormatter *f = (RuxenFormatter *) calloc(1, sizeof(RuxenFormatter));
    if (!f) { fprintf(stderr, "ruxen: formatter alloc failed\n"); exit(101); }
    f->precision = -1;
    f->fill_cp   = -1;
    ruxen_fmt_formatter_reserve(f, 0);
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
RuxenFormatter *ruxen_fmt_formatter_new_with_spec(
    int64_t width, int64_t precision, int64_t align, int64_t fill
) {
    RuxenFormatter *f = ruxen_fmt_formatter_new();
    f->width     = (int32_t) width;
    f->precision = (int32_t) precision;
    f->align     = (int8_t)  align;
    f->fill_cp   = (int32_t) fill;
    return f;
}

/* Phase 2 #06.D4: precision accessor.  Synth `Float_fmt` / `String_fmt`
 * read this to know whether to truncate / round.  Returns -1 when the
 * formatter was constructed without a precision (the default). */
int64_t ruxen_fmt_formatter_precision(const RuxenFormatter *f) {
    return f ? (int64_t) f->precision : -1;
}

/* Free the Formatter and its buffer.
 * Uses the _ORIG_FREE sentinel + RUXEN_ASM_LABEL rebind so the
 * drop_fixtures textual `free(` → `ruxen_test_free(` rewrite does not
 * mangle this call site (same pattern as ruxen_string_free /
 * ruxen_vec_free).  Forward decl + asm label are at the top of the
 * file to satisfy macOS clang's "asm label after first use" rule. */
void ruxen_fmt_formatter_ORIG_FREE(RuxenFormatter *f) {
    if (!f) return;
    if (f->buf) free(f->buf);
    free(f);
}

/* Append the NUL-terminated string `s` to the buffer.
 * Returns a POINTER-WIDTH `Result[nil, FmtError]` tag: NULL (tag-0 =
 * Ok(())) on success, (void *)1 (tag-1 = FmtError) on null input (v1
 * simplification: buffer overflow is not surfaced).
 *
 * Q54: the return type is `void *`, NOT `int64_t`. The compiler derives
 * this symbol's FFI signature from the `.rx` decl `-> Result[nil,
 * FmtError]`, a pointer-like type it lowers to a pointer (i64 on a
 * 64-bit target, i32 on wasm32). An `int64_t` return matched only on
 * 64-bit, where a pointer is also 64 bits; on wasm32 it linked as an
 * i32-vs-i64 mismatch that trapped EVERY string interpolation. Every
 * other Result-returning runtime helper already returns a pointer-width
 * value (`void *`); this matches that convention on every target. The
 * interpolation lowerer discards the tag. */
void *ruxen_fmt_formatter_write_str(RuxenFormatter *f, const char *s) {
    if (!f || !s) return (void *)1;
    size_t n = strlen(s);
    ruxen_fmt_formatter_reserve(f, n);
    memcpy(f->buf + f->len, s, n);
    f->len += n;
    f->buf[f->len] = '\0';
    return NULL;
}

/* Append a single Unicode codepoint, UTF-8 encoded. A value outside the
 * Unicode scalar range (negative, a surrogate, or above U+10FFFF) cannot
 * come from a `Char` and is written as U+FFFD REPLACEMENT CHARACTER.
 * Returns the pointer-width `Result[nil, FmtError]` tag (NULL = Ok); see
 * `ruxen_fmt_formatter_write_str` for why the return is `void *` (Q54). */
void *ruxen_fmt_formatter_write_char(RuxenFormatter *f, int64_t codepoint) {
    if (!f) return (void *)1;
    if (codepoint < 0 || codepoint > 0x10FFFF
        || (codepoint >= 0xD800 && codepoint <= 0xDFFF)) {
        codepoint = 0xFFFD;
    }
    char bytes[4];
    size_t n = ruxen_utf8_encode((uint32_t)codepoint, bytes);
    ruxen_fmt_formatter_reserve(f, n);
    memcpy(f->buf + f->len, bytes, n);
    f->len += n;
    f->buf[f->len] = '\0';
    return NULL;
}

/* Phase 2 #06.D4: apply width / align / fill to `taken` in place.
 * Returns a freshly allocated buffer when padding is needed; otherwise
 * returns `taken` untouched.  When a new buffer is allocated the input
 * is freed.  `len` is the byte length of `taken` (must not include the
 * trailing NUL).  The padded buffer is NUL-terminated. */
static char *ruxen_fmt_apply_pad(
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
    if (!out) { fprintf(stderr, "ruxen: pad alloc failed\n"); exit(101); }
    memset(out, fill_ch, left_pad);
    if (taken && len > 0) memcpy(out + left_pad, taken, len);
    memset(out + left_pad + len, fill_ch, right_pad);
    out[(size_t) width] = '\0';
    if (taken) free(taken);
    return out;
}

/* Transfer buffer ownership to a Ruxen String and free the Formatter.
 * The accumulated `buf` is taken directly (no copy unless padding is
 * required by the spec) and returned as a heap `char*` that satisfies
 * the Ruxen String ABI.  Codegen must not emit a follow-up `_free` call
 * on the Formatter after this.
 *
 * Phase 2 #06.D4: when the formatter was constructed with a width spec,
 * width/align/fill are applied here.  Precision is type-specific and is
 * consumed earlier by the synth `Float_fmt` / `String_fmt` bodies, not
 * here. */
const char *ruxen_fmt_formatter_buffer(RuxenFormatter *f) {
    if (!f) return ruxen_string_from("");
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
    ruxen_fmt_formatter_ORIG_FREE(f);
    if (!taken) return ruxen_string_from("");
    taken = ruxen_fmt_apply_pad(taken, len, width, align, fill_cp);
    return taken;
}

/* Return the number of bytes currently accumulated in the buffer. */
int64_t ruxen_fmt_formatter_len(const RuxenFormatter *f) {
    return f ? (int64_t) f->len : 0;
}

