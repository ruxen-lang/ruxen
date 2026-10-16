#include "../../core/runtime/runtime.h"

/* String runtime: conversions, search, split and UTF-8 helpers.
   A standalone translation unit. */

/* ── Numeric Conversions ───────────────────────────────────────────── */

/* `Int.to_f()` — widen a 64-bit integer to a double. The Cranelift/LLVM
   ABI passes the result back as a real f64 (see runtime_sigs.rs), so this
   is a plain cast, not a bit-reinterpretation. */
double ruxen_int_to_f(int64_t n) {
    return (double)n;
}

/* `Float.to_i()` — truncate a double toward zero into a 64-bit integer.
   Matches C cast semantics; out-of-range values are implementation-defined
   exactly as a C `(int64_t)` cast. */
int64_t ruxen_float_to_i(double f) {
    return (int64_t)f;
}

/* ── Exponentiation (`a ** b`) ──────────────────────────────────────── */

/* `Int ** Int`, exponentiation by squaring in 64-bit. Ruby returns a
   Rational for a negative exponent; Ruxen has no Rational, so a negative
   exponent yields the truncated integer result: exact for a base of
   1 / -1, and 0 for any |base| > 1 (its reciprocal truncates to zero).
   The squaring/multiply run in unsigned so overflow wraps modulo 2^64
   (well-defined) rather than tripping signed-overflow UB, matching the
   two's-complement wrap of the machine int arithmetic used elsewhere. */
int64_t ruxen_int_pow(int64_t base, int64_t exp) {
    if (exp < 0) {
        if (base == 1) return 1;
        if (base == -1) return (exp & 1) ? -1 : 1;
        return 0;
    }
    uint64_t result = 1;
    uint64_t b = (uint64_t)base;
    uint64_t e = (uint64_t)exp;
    while (e > 0) {
        if (e & 1u) result *= b;
        e >>= 1;
        if (e > 0) b *= b;
    }
    return (int64_t)result;
}

/* Three-way comparison `a <=> b` for integers, returning exactly -1 / 0 / 1
   (the spaceship convention), unlike a bare subtraction which can overflow. */
int64_t ruxen_int_cmp(int64_t a, int64_t b) {
    return (a < b) ? -1 : (a > b) ? 1 : 0;
}

/* Three-way comparison for floats. NaN operands fall through to 0 (neither
   `<` nor `>`), matching a total-order-free comparison. */
int64_t ruxen_float_cmp(double a, double b) {
    return (a < b) ? -1 : (a > b) ? 1 : 0;
}

/* `String <=> String`: `strcmp` normalized to the spaceship's -1 / 0 / 1,
   with the same null handling as `ruxen_string_cmp`. */
int64_t ruxen_str_spaceship(const char *a, const char *b) {
    if (!a && !b) return 0;
    if (!a) return -1;
    if (!b) return 1;
    int c = strcmp(a, b);
    return (c < 0) ? -1 : (c > 0) ? 1 : 0;
}

/* `a ** b` when either operand is a Float, IEEE-754 `pow` via the compiler
   builtin (avoids a `<math.h>` include, keeping the runtime include order
   untouched; `libm` is linked into every Ruxen binary via std-core). Int
   operands are widened to `double` at the MIR call site. */
double ruxen_float_pow(double base, double exp) {
    return __builtin_pow(base, exp);
}

/* ── To-String Conversions ─────────────────────────────────────────── */

char *ruxen_int_to_string(int64_t n) {
    char buf[32];
    snprintf(buf, sizeof(buf), "%" PRId64, n);
    size_t len = strlen(buf);
    char *result = (char *)ruxen_xmalloc(len + 1);
    memcpy(result, buf, len + 1);
    return result;
}

char *ruxen_float_to_string(double f) {
    char buf[64];
    snprintf(buf, sizeof(buf), "%g", f);
    size_t len = strlen(buf);
    char *result = (char *)ruxen_xmalloc(len + 1);
    memcpy(result, buf, len + 1);
    return result;
}

/* Phase 2 #06.D4: format a float with an explicit decimal precision.
 * `prec < 0` falls back to `%g` (matches `ruxen_float_to_string`).
 * `prec >= 0` uses `%.<prec>f`. */
char *ruxen_float_to_string_prec(double f, int64_t prec) {
    if (prec < 0) return ruxen_float_to_string(f);
    if (prec > 60) prec = 60; /* snprintf safety bound */
    char buf[96];
    snprintf(buf, sizeof(buf), "%.*f", (int)prec, f);
    size_t len = strlen(buf);
    char *result = (char *)ruxen_xmalloc(len + 1);
    memcpy(result, buf, len + 1);
    return result;
}

/* Convert a Unicode codepoint (passed widened to i64) into a heap-allocated
   UTF-8 string. Used for `"#{c}"` interpolation on values of type `Char`. */
char *ruxen_char_to_string(int64_t codepoint) {
    char buf[4];
    size_t len = ruxen_utf8_encode((uint32_t)codepoint, buf);
    char *result = (char *)ruxen_xmalloc(len + 1);
    memcpy(result, buf, len);
    result[len] = '\0';
    return result;
}

char *ruxen_bool_to_string(int64_t b) {
    const char *s = b ? "true" : "false";
    size_t len = strlen(s);
    char *result = (char *)ruxen_xmalloc(len + 1);
    memcpy(result, s, len + 1);
    return result;
}

/* ── String Operations ─────────────────────────────────────────────── */

/* ── String Comparison ─────────────────────────────────────────── */

int64_t ruxen_string_eq(const char *a, const char *b) {
    if (a == b) return 1;
    if (!a || !b) return 0;
    return strcmp(a, b) == 0 ? 1 : 0;
}

int64_t ruxen_string_cmp(const char *a, const char *b) {
    if (!a && !b) return 0;
    if (!a) return -1;
    if (!b) return 1;
    return (int64_t)strcmp(a, b);
}

int64_t ruxen_string_hash(const char *s) {
    return (int64_t)ruxen_hash_str(s);
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
 * Option-of-heap helper exists (the current `ruxen_option_*` family
 * only handles primitive payloads).
 * --------------------------------------------------------------------- */
char *ruxen_path_join(const char *a, const char *b) {
    const char *base = a ? a : "";
    const char *tail = b ? b : "";

    /* If the second component is absolute it overrides the first. */
    if (tail[0] == '/') {
        return ruxen_string_from(tail);
    }

    size_t la = strlen(base);
    size_t lb = strlen(tail);

    /* Empty base → just clone tail; empty tail → just clone base. */
    if (la == 0) return ruxen_string_from(tail);
    if (lb == 0) return ruxen_string_from(base);

    int needsep = base[la - 1] != '/';
    size_t total = la + (needsep ? 1 : 0) + lb;
    char *out = (char *)malloc(total + 1);
    if (!out) return ruxen_string_from("");
    memcpy(out, base, la);
    size_t i = la;
    if (needsep) {
        out[i++] = '/';
    }
    memcpy(out + i, tail, lb);
    out[total] = '\0';
    return out;
}

char *ruxen_path_parent(const char *p) {
    if (!p || !*p) return ruxen_string_from("");
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
        return ruxen_string_from("");
    }
    if (i == 0) {
        /* Parent is root. */
        return ruxen_string_from("/");
    }

    char *out = (char *)malloc((size_t)i + 1);
    if (!out) return ruxen_string_from("");
    memcpy(out, p, (size_t)i);
    out[i] = '\0';
    return out;
}

char *ruxen_path_file_name(const char *p) {
    if (!p || !*p) return ruxen_string_from("");
    size_t n = strlen(p);

    /* Trailing slashes mean the path designates a directory whose name
     * we still want — `/foo/bar/` → `bar`. Strip them first. */
    while (n > 0 && p[n - 1] == '/') {
        n--;
    }
    if (n == 0) return ruxen_string_from("");

    ssize_t start = (ssize_t)n - 1;
    while (start >= 0 && p[start] != '/') {
        start--;
    }
    start++; /* point at first char of file name */

    size_t len = n - (size_t)start;
    char *out = (char *)malloc(len + 1);
    if (!out) return ruxen_string_from("");
    memcpy(out, p + start, len);
    out[len] = '\0';
    return out;
}

char *ruxen_path_extension(const char *p) {
    char *name = ruxen_path_file_name(p);
    if (!name || !*name) {
        return name ? name : ruxen_string_from("");
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
        return ruxen_string_from("");
    }

    size_t ext_len = n - (size_t)dot - 1;
    char *out = (char *)malloc(ext_len + 1);
    if (!out) {
        free(name);
        return ruxen_string_from("");
    }
    memcpy(out, name + dot + 1, ext_len);
    out[ext_len] = '\0';
    free(name);
    return out;
}

int64_t ruxen_path_is_absolute(const char *p) {
    return (p && p[0] == '/') ? 1 : 0;
}

char *ruxen_string_concat(const char *a, const char *b) {
    if (!a && !b) return NULL;
    if (!a) return ruxen_string_from(b);
    if (!b) return ruxen_string_from(a);
    size_t len_a = strlen(a);
    size_t len_b = strlen(b);
    size_t total;
    if (__builtin_add_overflow(len_a, len_b, &total) ||
        __builtin_add_overflow(total, 1, &total)) {
        ruxen_panic("string size overflow");
    }
    char *result = (char *)ruxen_xmalloc(total);
    memcpy(result, a, len_a);
    memcpy(result + len_a, b, len_b + 1);
    return result;
}

char *ruxen_string_from(const char *s) {
    if (!s) return NULL;
    size_t len = strlen(s);
    char *result = (char *)ruxen_xmalloc(len + 1);
    memcpy(result, s, len + 1);
    return result;
}

/* Build a heap-allocated null-terminated String from a Vec[Int]
 * whose entries are byte values (0–255). Each i64 slot's low byte
 * becomes one output byte — matches the wire shape of
 * `TcpStream.read` / `BufReader.read` which push raw bytes as
 * widened int64s. Surface: `String.from_bytes(&Array[Int]) -> String`.
 *
 * The caller retains ownership of the source Vec; we copy out. */
char *ruxen_string_from_bytes(const RuxenVec *v) {
    if (!v) return ruxen_string_from("");
    uint64_t n = v->len;
    char *result = (char *)ruxen_xmalloc(n + 1);
    for (uint64_t i = 0; i < n; i++) {
        result[i] = (char)(v->data[i] & 0xff);
    }
    result[n] = '\0';
    return result;
}

/* Phase 2 #06.D4: return a fresh string truncated to at most `max_chars`
 * UTF-8 codepoints.  `max_chars < 0` returns a copy of the input
 * unchanged (used when the precision spec is unset).  Truncation
 * respects UTF-8 boundaries — the returned string is always valid
 * UTF-8.  Returns a freshly-allocated heap string the caller owns.
 *
 * Distinct from the in-place `ruxen_string_truncate(char*, n)` used by
 * `String.truncate(n)` — this variant takes char count (not bytes) and
 * returns a fresh string so the source remains usable. */
char *ruxen_string_truncate_chars(const char *s, int64_t max_chars) {
    if (!s) return ruxen_string_from("");
    if (max_chars < 0) return ruxen_string_from(s);
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
    char *result = (char *) ruxen_xmalloc(kept_bytes + 1);
    memcpy(result, s, kept_bytes);
    result[kept_bytes] = '\0';
    return result;
}

/* ── String Extended Operations ────────────────────────────────────── */

uint64_t ruxen_string_len(const char *s) {
    return s ? (uint64_t)strlen(s) : 0;
}

int8_t ruxen_string_is_empty(const char *s) {
    return (!s || s[0] == '\0') ? 1 : 0;
}

/* The receiver-mutating String methods (`def var` in string.rx) take the
   address of the caller's String slot (`char**`, a `&var String`) and
   replace the buffer in place: they store the new buffer and free the old
   one, so the caller never sees a stale pointer. */
static void ruxen_string_replace_slot(char **slot, char *fresh) {
    char *old = *slot;
    *slot = fresh;
    free(old);
}

/* String.push_str(&String) — append `src` onto the receiver. */
void ruxen_string_push_str(char **slot, const char *src) {
    ruxen_string_replace_slot(slot, ruxen_string_concat(*slot, src));
}


char *ruxen_string_trim(const char *s) {
    if (!s) return ruxen_string_from("");
    /* Skip leading whitespace */
    while (*s == ' ' || *s == '\t' || *s == '\n' || *s == '\r') s++;
    size_t len = strlen(s);
    /* Skip trailing whitespace */
    while (len > 0 && (s[len-1] == ' ' || s[len-1] == '\t' ||
           s[len-1] == '\n' || s[len-1] == '\r')) len--;
    char *result = (char *)ruxen_xmalloc(len + 1);
    memcpy(result, s, len);
    result[len] = '\0';
    return result;
}

char *ruxen_string_to_lower(const char *s) {
    if (!s) return ruxen_string_from("");
    size_t len = strlen(s);
    char *result = (char *)ruxen_xmalloc(len + 1);
    for (size_t i = 0; i < len; i++) {
        char c = s[i];
        if (c >= 'A' && c <= 'Z') c = c + ('a' - 'A');
        result[i] = c;
    }
    result[len] = '\0';
    return result;
}

char *ruxen_string_to_upper(const char *s) {
    if (!s) return ruxen_string_from("");
    size_t len = strlen(s);
    char *result = (char *)ruxen_xmalloc(len + 1);
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
char *ruxen_string_new(void) {
    return ruxen_string_from("");
}

/* String.with_capacity(n) — empty string with at least `n` bytes
   pre-allocated. v1 stores strings as plain malloc'd char* with no
   inline capacity field; we honour the request by allocating a buffer
   of size max(n+1, 1) but the user-visible length is still 0. */
char *ruxen_string_with_capacity(int64_t cap) {
    size_t want = cap > 0 ? (size_t)cap : 0;
    char *out = (char *)ruxen_xmalloc(want + 1);
    out[0] = '\0';
    return out;
}

/* String.as_str / String.to_string — identity at the runtime layer.
   `to_string` returns an owned clone so the user can mutate it. */
const char *ruxen_string_as_str(const char *s) {
    return s ? s : "";
}

char *ruxen_string_to_string(const char *s) {
    return ruxen_string_from(s ? s : "");
}

/* String.bytes / String.into_bytes — return a Vec[U8] (i.e. RuxenVec
   with one slot per byte). Currently runtime Vec slots are 64-bit so
   each byte gets widened on the way in; the typecker promises the
   user-visible element type is U8. */
RuxenVec *ruxen_string_bytes(const char *s) {
    RuxenVec *result = ruxen_vec_new();
    if (!s) return result;
    while (*s) {
        ruxen_vec_push(result, (int64_t)(uint8_t)*s);
        s++;
    }
    return result;
}

/* trim_start / trim_end — like trim but only one side. Returns owned. */
char *ruxen_string_trim_start(const char *s) {
    if (!s) return ruxen_string_from("");
    while (*s == ' ' || *s == '\t' || *s == '\n' || *s == '\r') s++;
    return ruxen_string_from(s);
}

char *ruxen_string_trim_end(const char *s) {
    if (!s) return ruxen_string_from("");
    size_t len = strlen(s);
    while (len > 0 && (s[len-1] == ' ' || s[len-1] == '\t' ||
           s[len-1] == '\n' || s[len-1] == '\r')) len--;
    char *result = (char *)ruxen_xmalloc(len + 1);
    memcpy(result, s, len);
    result[len] = '\0';
    return result;
}

/* String.find(&str) — return Option[USize] of the first byte index of
   the needle, or None if not found / empty needle / null receiver. */
void *ruxen_string_find(const char *s, const char *needle) {
    if (!s || !needle || needle[0] == '\0') return ruxen_option_none();
    const char *hit = strstr(s, needle);
    if (!hit) return ruxen_option_none();
    return ruxen_option_some((int64_t)(hit - s));
}

/* String.splitn(n, &str) — split into at most `n` parts. Returns a
   Vec of owned strings. n <= 0 produces an empty Vec. n == 1 yields
   a Vec containing the entire string. */
RuxenVec *ruxen_string_splitn(const char *s, int64_t n, const char *delimiter) {
    RuxenVec *result = ruxen_vec_new();
    if (!s || n <= 0) return result;
    if (!delimiter || delimiter[0] == '\0' || n == 1) {
        ruxen_vec_push(result, (int64_t)ruxen_string_from(s));
        return result;
    }
    size_t dlen = strlen(delimiter);
    const char *start = s;
    int64_t produced = 0;
    while (produced < n - 1) {
        const char *found = strstr(start, delimiter);
        if (!found) break;
        size_t part_len = (size_t)(found - start);
        char *part = (char *)ruxen_xmalloc(part_len + 1);
        memcpy(part, start, part_len);
        part[part_len] = '\0';
        ruxen_vec_push(result, (int64_t)part);
        start = found + dlen;
        produced++;
    }
    ruxen_vec_push(result, (int64_t)ruxen_string_from(start));
    return result;
}

/* String.clear / truncate(n) — mutating, in place on the slot's owned
   buffer. truncate(n) keeps at most `n` leading bytes; n >= len is a
   no-op. Negative n is a no-op. */
void ruxen_string_clear(char **slot) {
    char *s = *slot;
    if (s) s[0] = '\0';
}

void ruxen_string_truncate(char **slot, int64_t n) {
    char *s = *slot;
    if (!s) return;
    if (n < 0) return;
    size_t len = strlen(s);
    size_t cap = (size_t)n;
    if (cap >= len) return;
    s[cap] = '\0';
}

/* String.insert(i, char) — insert the char widened to UTF-8 at byte
   index `i`. Out-of-range index panics. */
void ruxen_string_insert(char **slot, int64_t i, int64_t codepoint) {
    const char *base = *slot ? *slot : "";
    size_t len = strlen(base);
    if (i < 0 || (size_t)i > len) {
        ruxen_panic("string insert index out of bounds");
    }
    char *one = ruxen_char_to_string(codepoint);
    size_t one_len = strlen(one);
    char *out = (char *)ruxen_xmalloc(len + one_len + 1);
    memcpy(out, base, (size_t)i);
    memcpy(out + i, one, one_len);
    memcpy(out + i + one_len, base + i, len - (size_t)i);
    out[len + one_len] = '\0';
    free(one);
    ruxen_string_replace_slot(slot, out);
}

/* String.insert_str(i, &str) — same idea but for a borrowed slice. */
void ruxen_string_insert_str(char **slot, int64_t i, const char *part) {
    const char *base = *slot ? *slot : "";
    size_t len = strlen(base);
    if (i < 0 || (size_t)i > len) {
        ruxen_panic("string insert_str index out of bounds");
    }
    const char *src = part ? part : "";
    size_t plen = strlen(src);
    char *out = (char *)ruxen_xmalloc(len + plen + 1);
    memcpy(out, base, (size_t)i);
    memcpy(out + i, src, plen);
    memcpy(out + i + plen, base + i, len - (size_t)i);
    out[len + plen] = '\0';
    ruxen_string_replace_slot(slot, out);
}

/* String.remove(i) — for v1 we operate on a single byte at a time
   (proper UTF-8 codepoint removal lands with full Char support, see
   §9 of the stdlib brief). Removes the byte at `i` from the slot's
   buffer and returns it as a Char. */
int32_t ruxen_string_remove(char **slot, int64_t i) {
    const char *s = *slot;
    if (!s) {
        ruxen_panic("string remove on null");
    }
    size_t len = strlen(s);
    if (i < 0 || (size_t)i >= len) {
        ruxen_panic("string remove index out of bounds");
    }
    int32_t removed = (int32_t)(uint8_t)s[i];
    char *new_buf = (char *)ruxen_xmalloc(len);  /* one byte shorter + NUL */
    memcpy(new_buf, s, (size_t)i);
    memcpy(new_buf + i, s + i + 1, len - (size_t)i - 1);
    new_buf[len - 1] = '\0';
    ruxen_string_replace_slot(slot, new_buf);
    return removed;
}

/* String.parse_int / parse_float — full surface, returning the same
   tagged Result the rest of the runtime uses. The Err payload is a
   ParseIntError / ParseFloatError box (newtype around a String message). */
static void *ruxen_parse_error_box(const char *msg) {
    char **box_ = (char **)ruxen_alloc(8);
    *box_ = ruxen_string_from(msg ? msg : "parse error");
    return box_;
}

void *ruxen_string_parse_int(const char *s) {
    if (!s || *s == '\0') {
        return ruxen_result_err_value((int64_t)ruxen_parse_error_box("empty string"));
    }
    /* Skip leading whitespace — matches Rust's i64::from_str_radix
       which does *not* skip whitespace, but matches the user expectation
       that "  -42" parses. We follow Rust here: no whitespace stripping. */
    char *end = NULL;
    errno = 0;
    long long val = strtoll(s, &end, 10);
    if (end == s || *end != '\0') {
        return ruxen_result_err_value((int64_t)ruxen_parse_error_box("invalid integer"));
    }
    if (errno == ERANGE) {
        return ruxen_result_err_value((int64_t)ruxen_parse_error_box("integer out of range"));
    }
    return ruxen_result_ok_value((int64_t)val);
}

void *ruxen_string_parse_float(const char *s) {
    if (!s || *s == '\0') {
        return ruxen_result_err_value((int64_t)ruxen_parse_error_box("empty string"));
    }
    char *end = NULL;
    errno = 0;
    double val = strtod(s, &end);
    if (end == s || *end != '\0') {
        return ruxen_result_err_value((int64_t)ruxen_parse_error_box("invalid float"));
    }
    if (errno == ERANGE) {
        return ruxen_result_err_value((int64_t)ruxen_parse_error_box("float out of range"));
    }
    /* Bit-cast double into the 64-bit payload slot. */
    union { double d; int64_t i; } u;
    u.d = val;
    return ruxen_result_ok_value(u.i);
}

/* ParseIntError / ParseFloatError accessors. The runtime stores both
   error variants as a newtype around a heap String — a `char**` whose
   pointee is the message buffer. The .message accessor returns a
   reference (just the inner pointer) so the caller doesn't free it. */
const char *ruxen_parse_error_message(char **box_) {
    if (!box_ || !*box_) return "";
    return *box_;
}

/* String.split(&str) — split at every occurrence of the delimiter.
   Returns a Vec of owned strings (heap-allocated `char*` per part).
   Differs from `ruxen_string_splitn`, which caps the number of parts.
   Empty delimiter or null receiver yields a Vec containing the
   original string (matches Rust's surprising `"abc".split("")`-like
   convention only at the empty-receiver edge; for our v1 surface we
   treat empty delimiter as "no split"). */
RuxenVec *ruxen_string_split(const char *s, const char *delimiter) {
    RuxenVec *result = ruxen_vec_new();
    if (!s) return result;
    if (!delimiter || delimiter[0] == '\0') {
        ruxen_vec_push(result, (int64_t)ruxen_string_from(s));
        return result;
    }
    size_t dlen = strlen(delimiter);
    const char *start = s;
    while (1) {
        const char *found = strstr(start, delimiter);
        if (!found) {
            ruxen_vec_push(result, (int64_t)ruxen_string_from(start));
            break;
        }
        size_t part_len = (size_t)(found - start);
        char *part = (char *)ruxen_xmalloc(part_len + 1);
        memcpy(part, start, part_len);
        part[part_len] = '\0';
        ruxen_vec_push(result, (int64_t)part);
        start = found + dlen;
    }
    return result;
}

/* String.push(Char) — append a single Char (a UTF-8 codepoint) onto the
   receiver. */
void ruxen_string_push(char **slot, int32_t codepoint) {
    const char *base = *slot ? *slot : "";
    size_t len = strlen(base);
    char *one = ruxen_char_to_string(codepoint);
    size_t one_len = strlen(one);
    char *out = (char *)ruxen_xmalloc(len + one_len + 1);
    memcpy(out, base, len);
    memcpy(out + len, one, one_len + 1);
    free(one);
    ruxen_string_replace_slot(slot, out);
}

/* String.into_bytes — consuming variant of `bytes`. Takes ownership of
   the receiver: returns a Vec[U8] holding the UTF-8 bytes and frees
   the source `char*` spine. The MIR call site is responsible for
   suppressing the regular scope-exit drop on the source local so we
   don't double-free. The Vec returned holds widened i64 byte values
   per the existing v1 element-slot convention. */
RuxenVec *ruxen_string_into_bytes(char *s) {
    RuxenVec *result = ruxen_vec_new();
    if (!s) return result;
    const char *p = s;
    while (*p) {
        ruxen_vec_push(result, (int64_t)(uint8_t)*p);
        p++;
    }
    /* Free the source buffer — the caller has handed us ownership and
       relies on us draining it. The drop pass above the call site must
       not also emit a `ruxen_string_free` for the same local, or this
       becomes a double-free. */
    free(s);
    return result;
}

/* ── &str Operations ──────────────────────────────────────────────── */

/* Parse a string to an unsigned integer, returning a Result-like value.
   Returns a tagged union: tag=0 (Ok) with value, tag=1 (Err). */
void *ruxen_str_parse_uint(const char *s) {
    if (!s || *s == '\0') return ruxen_result_err_value(0);
    char *end;
    unsigned long val = strtoul(s, &end, 10);
    if (*end != '\0') return ruxen_result_err_value(0);
    return ruxen_result_ok_value((int64_t)val);
}

/* ── Search, split and collect ─────────────────────────────────────── */

/* String predicates / search / repeat — quick helpers around libc. */

int8_t ruxen_string_contains(const char *s, const char *needle) {
    if (!s || !needle) return 0;
    return strstr(s, needle) ? 1 : 0;
}

int8_t ruxen_string_starts_with(const char *s, const char *prefix) {
    if (!s || !prefix) return 0;
    size_t plen = strlen(prefix);
    if (plen == 0) return 1;
    if (strlen(s) < plen) return 0;
    return strncmp(s, prefix, plen) == 0 ? 1 : 0;
}

int8_t ruxen_string_ends_with(const char *s, const char *suffix) {
    if (!s || !suffix) return 0;
    size_t slen = strlen(s);
    size_t xlen = strlen(suffix);
    if (xlen == 0) return 1;
    if (slen < xlen) return 0;
    return strncmp(s + slen - xlen, suffix, xlen) == 0 ? 1 : 0;
}

char *ruxen_string_repeat(const char *s, int64_t count) {
    if (!s || count <= 0) return ruxen_string_from("");
    size_t slen = strlen(s);
    if (slen == 0) return ruxen_string_from("");
    size_t total = slen * (size_t)count;
    char *out = (char *)ruxen_xmalloc(total + 1);
    for (int64_t i = 0; i < count; i++) {
        memcpy(out + (size_t)i * slen, s, slen);
    }
    out[total] = '\0';
    return out;
}

/* Decode a UTF-8 string into a Vec of codepoints (widened to i64) so that
   the existing Vec-iteration machinery can drive `for ch in s.chars`.
   Malformed bytes are passed through as single-byte codepoints. */
RuxenVec *ruxen_string_chars(const char *s) {
    RuxenVec *result = ruxen_vec_new();
    if (!s) return result;
    const unsigned char *p = (const unsigned char *)s;
    while (*p) {
        size_t n;
        uint32_t cp = ruxen_utf8_decode(p, &n);
        ruxen_vec_push(result, (int64_t)cp);
        p += n;
    }
    return result;
}

/* String.from_iter(iter[String]) / iter.collect[String] — concatenate
   the owned string (or &str-like) elements in iteration order into one
   fresh owned string. v1 keeps this narrow: typeck only accepts
   String/&str items, not arbitrary Display/ToString conversions. */
char *ruxen_string_from_iter(RuxenVec *iter) {
    char *out = ruxen_string_from("");
    if (!iter) return out;
    for (uint64_t i = 0; i < iter->len; i++) {
        const char *part = (const char *)iter->data[i];
        char *next = ruxen_string_concat(out, part ? part : "");
        ruxen_string_ORIG_FREE(out);
        out = next;
    }
    return out;
}

/* String::lines — split on '\n'. The trailing empty element after
   a terminating newline is dropped (matches Rust's `str::lines`).
   '\r' immediately before a '\n' is also dropped. */
RuxenVec *ruxen_string_lines(const char *s) {
    RuxenVec *result = ruxen_vec_new();
    if (!s) return result;
    const char *start = s;
    while (1) {
        const char *nl = strchr(start, '\n');
        size_t len = nl ? (size_t)(nl - start) : strlen(start);
        if (len > 0 && start[len - 1] == '\r') {
            len--;
        }
        char *line = (char *)ruxen_xmalloc(len + 1);
        memcpy(line, start, len);
        line[len] = '\0';
        ruxen_vec_push(result, (int64_t)line);
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
char *ruxen_string_replace(const char *s, const char *from, const char *to) {
    if (!s) return ruxen_string_from("");
    if (!from || from[0] == '\0') return ruxen_string_from(s);
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
    if (occurrences == 0) return ruxen_string_from(s);

    size_t total = s_len + occurrences * (to_len > from_len ? (to_len - from_len) : 0);
    if (to_len < from_len) {
        total = s_len - occurrences * (from_len - to_len);
    }
    char *out = (char *)ruxen_xmalloc(total + 1);

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
