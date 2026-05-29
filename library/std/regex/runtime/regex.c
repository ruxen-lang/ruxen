/* library/std/regex/runtime/regex.c
 *
 * std::regex C runtime — PCRE2 wrapper.
 *
 * The vendored PCRE2 lives under ./pcre2/ and is compiled into
 * libruxenrt.a by src/ruxen_repl/build.rs. We link against the
 * resulting static lib without going through any system pcre2
 * package — see commit 12e24ce (Phase 0 of the std.regex plan)
 * for the vendoring rationale.
 *
 * All ruxen_regex_* / ruxen_match_* / ruxen_regex_error_* symbols
 * here are wired through compiler/ruxen_core/src/codegen/cranelift/
 * runtime_sigs.rs in Phase 2; this TU just produces them.
 *
 * Match handle ownership: every successful match() / scan() copies
 * the subject string + ovector + named-table into a fresh RuxenMatch
 * so the Ruxen caller can drop the original subject String and still
 * query the match. Drop is auto-synthesised on the class Match in
 * lib.rx and routes to ruxen_match_drop here.
 *
 * Replay-flag interaction: regex helpers are idempotent (pure
 * compile + match + replace) and do NOT check ruxen_repl_is_replaying.
 * The REPL state refactor's flag exists to suppress side effects;
 * regex has none.
 */

#define PCRE2_CODE_UNIT_WIDTH 8
#include "../../core/runtime/runtime.h"
#include "pcre2/pcre2.h"

#include <string.h>
#include <stdint.h>

/* ruxen_vec_new / _push and ruxen_hash_new / _insert are declared in
 * runtime.h above (cross-package surface). Defined in
 * library/std/array/runtime/vec.c and library/std/hash/runtime/hash.c. */

/* ── Option constructors (inlined) ───────────────────────────────────
 *
 * The runtime has no public _some_value / _none_value factory — codegen
 * normally allocates Option payloads from MIR via Alloc + SetField, and
 * cross-package C call sites that need to fabricate one from C do this
 * inline. See library/std/io/runtime/bufio.c:90 for the canonical
 * pattern this is modelled on.
 *
 * Layout: 16 bytes, {i32 tag; i32 pad; i64 payload}. tag = 0 → None,
 * tag = 1 → Some. Matches the resolver's variant_idx ordering and
 * the Result helpers in runtime.h. */
static void *ruxen_regex_option_none(void) {
    int64_t *out = (int64_t *)ruxen_alloc(16);
    *(int32_t *)out = 0; /* None */
    out[1] = 0;
    return out;
}

static void *ruxen_regex_option_some(int64_t payload) {
    int64_t *out = (int64_t *)ruxen_alloc(16);
    *(int32_t *)out = 1; /* Some */
    out[1] = payload;
    return out;
}

/* ── Flag-string → PCRE2 compile options ─────────────────────────── */

/* Convert Ruxen flag-string chars to PCRE2 compile options. The lexer
 * (Phase 3) already restricts the flag set to [imsgx]; unknown chars
 * here are silently ignored to keep this helper resilient if it's ever
 * called from a runtime-built pattern. The 'g' flag has no PCRE2
 * counterpart — global-ness in Ruxen is expressed by .scan /
 * .replace_all, so we accept and ignore it. */
static uint32_t flags_to_pcre2(const char *flags) {
    uint32_t opts = 0;
    if (!flags) return opts;
    for (const char *p = flags; *p; p++) {
        switch (*p) {
            case 'i': opts |= PCRE2_CASELESS;  break;
            case 'm': opts |= PCRE2_MULTILINE; break;
            case 's': opts |= PCRE2_DOTALL;    break;
            case 'x': opts |= PCRE2_EXTENDED;  break;
            case 'g': /* no-op — Ruxen uses explicit .scan / .replace_all */ break;
            default:  /* unknown flag — typeck rejects earlier */            break;
        }
    }
    return opts;
}

/* ── Match handle ────────────────────────────────────────────────── */

/* Wire form of a Match. Owns copies of the subject string + ovector
 * + named-table so the caller can drop the original subject String
 * and still read groups out of the Match.
 *
 * Allocated via ruxen_alloc; freed in ruxen_match_drop (which Ruxen's
 * implicit_includes synth-Drop calls when a Match goes out of scope).
 * ovector_count is the *pair* count (PCRE2's `rc` from pcre2_match_8);
 * indexing uses ovector_copy[n*2] / [n*2+1] for start/end of group n. */
typedef struct RuxenMatch {
    char       *subject_copy;
    int         subject_len;
    PCRE2_SIZE *ovector_copy;
    int         ovector_count;   /* pair count */
    char       *named_table_copy;
    int         named_count;
    int         named_entry_size;
} RuxenMatch;

void ruxen_match_drop(RuxenMatch *m) {
    if (!m) return;
    if (m->subject_copy)     ruxen_dealloc(m->subject_copy);
    if (m->ovector_copy)     ruxen_dealloc(m->ovector_copy);
    if (m->named_table_copy) ruxen_dealloc(m->named_table_copy);
    ruxen_dealloc(m);
}

/* ── Compile-time literal path ───────────────────────────────────── */

/* Module-init compile of a /pat/flags literal. Pattern was already
 * validated by typeck (E1704). On the off-chance PCRE2 still rejects
 * it (different libpcre2 internal state — shouldn't happen given
 * we vendor the lib), we panic — the literal is well-formed source
 * code at compile time, a divergence here means the toolchain is in
 * an inconsistent state. */
pcre2_code_8 *ruxen_regex_compile_const(const char *pattern, const char *flags) {
    int errornumber = 0;
    PCRE2_SIZE erroroffset = 0;
    pcre2_code_8 *re = pcre2_compile_8(
        (PCRE2_SPTR)pattern, PCRE2_ZERO_TERMINATED,
        flags_to_pcre2(flags), &errornumber, &erroroffset, NULL);
    if (!re) ruxen_panic("ruxen_regex_compile_const: PCRE2 rejected a literal");
    return re;
}

void ruxen_regex_drop(pcre2_code_8 *r) {
    if (r) pcre2_code_free_8(r);
}

/* ── Runtime compile path + RegexError ───────────────────────────── */

/* Wire form of RegexError. Single-shot — created by ruxen_regex_new
 * on the Err path, freed by the auto-synth Drop on RegexError. */
typedef struct RuxenRegexError {
    char    *message;          /* allocated, NUL-terminated */
    int64_t  offset;           /* byte offset into pattern */
} RuxenRegexError;

void ruxen_regex_error_drop(RuxenRegexError *e) {
    if (!e) return;
    if (e->message) ruxen_dealloc(e->message);
    ruxen_dealloc(e);
}

char *ruxen_regex_error_message(RuxenRegexError *e) {
    /* Return a copy so the caller can take ownership / outlive `e`. */
    if (!e || !e->message) return NULL;
    size_t n = strlen(e->message);
    char *buf = (char *)ruxen_alloc(n + 1);
    memcpy(buf, e->message, n + 1);
    return buf;
}

int64_t ruxen_regex_error_offset(RuxenRegexError *e) {
    return e ? e->offset : 0;
}

/* Runtime compile. Returns Result[Regex, RegexError] via the existing
 * Result helpers. The caller pattern is `Regex.new(pat, flags)?` so
 * surface ergonomics match String.parse_int. */
void *ruxen_regex_new(const char *pattern, const char *flags) {
    int errornumber = 0;
    PCRE2_SIZE erroroffset = 0;
    pcre2_code_8 *re = pcre2_compile_8(
        (PCRE2_SPTR)pattern, PCRE2_ZERO_TERMINATED,
        flags_to_pcre2(flags), &errornumber, &erroroffset, NULL);
    if (re) return ruxen_result_ok_value((int64_t)(intptr_t)re);

    /* Build the error handle. PCRE2's get_error_message writes the
     * diagnostic into the caller-supplied buffer and returns the
     * length. 256 bytes is comfortably larger than any PCRE2 error. */
    PCRE2_UCHAR buf[256];
    pcre2_get_error_message_8(errornumber, buf, sizeof(buf));
    size_t n = strlen((const char *)buf);
    RuxenRegexError *err = (RuxenRegexError *)ruxen_alloc(sizeof(RuxenRegexError));
    err->message = (char *)ruxen_alloc(n + 1);
    memcpy(err->message, buf, n + 1);
    err->offset = (int64_t)erroroffset;
    return ruxen_result_err_value((int64_t)(intptr_t)err);
}

/* ── is_match (the Bool query backing ~=) ────────────────────────── */

int64_t ruxen_regex_is_match(pcre2_code_8 *r, const char *text) {
    if (!r || !text) return 0;
    pcre2_match_data_8 *md = pcre2_match_data_create_from_pattern_8(r, NULL);
    int rc = pcre2_match_8(r, (PCRE2_SPTR)text, PCRE2_ZERO_TERMINATED,
                           0, 0, md, NULL);
    pcre2_match_data_free_8(md);
    return rc >= 0 ? 1 : 0;
}

/* ── Match builder + match() ─────────────────────────────────────── */

/* Build a RuxenMatch from a successful pcre2_match result. Copies
 * subject, ovector, and the named-group table so the handle survives
 * the original String going out of scope. */
static RuxenMatch *build_match_handle(pcre2_code_8 *r, const char *text,
                                      pcre2_match_data_8 *md, int ovector_count) {
    RuxenMatch *m = (RuxenMatch *)ruxen_alloc(sizeof(RuxenMatch));
    size_t tlen = strlen(text);
    m->subject_copy = (char *)ruxen_alloc(tlen + 1);
    memcpy(m->subject_copy, text, tlen + 1);
    m->subject_len = (int)tlen;

    PCRE2_SIZE *ov = pcre2_get_ovector_pointer_8(md);
    size_t ov_bytes = (size_t)ovector_count * 2 * sizeof(PCRE2_SIZE);
    m->ovector_copy = (PCRE2_SIZE *)ruxen_alloc(ov_bytes);
    memcpy(m->ovector_copy, ov, ov_bytes);
    m->ovector_count = ovector_count;

    /* Copy PCRE2's named-group table so m->named() works after drop. */
    uint32_t name_count = 0, name_entry_size = 0;
    PCRE2_SPTR name_table = NULL;
    pcre2_pattern_info_8(r, PCRE2_INFO_NAMECOUNT, &name_count);
    pcre2_pattern_info_8(r, PCRE2_INFO_NAMEENTRYSIZE, &name_entry_size);
    pcre2_pattern_info_8(r, PCRE2_INFO_NAMETABLE, &name_table);
    if (name_count > 0 && name_table && name_entry_size > 0) {
        size_t nbytes = (size_t)name_count * name_entry_size;
        m->named_table_copy = (char *)ruxen_alloc(nbytes);
        memcpy(m->named_table_copy, name_table, nbytes);
        m->named_count = (int)name_count;
        m->named_entry_size = (int)name_entry_size;
    } else {
        m->named_table_copy = NULL;
        m->named_count = 0;
        m->named_entry_size = 0;
    }
    return m;
}

void *ruxen_regex_match(pcre2_code_8 *r, const char *text) {
    if (!r || !text) return ruxen_regex_option_none();
    pcre2_match_data_8 *md = pcre2_match_data_create_from_pattern_8(r, NULL);
    int rc = pcre2_match_8(r, (PCRE2_SPTR)text, PCRE2_ZERO_TERMINATED,
                           0, 0, md, NULL);
    if (rc < 0) {
        pcre2_match_data_free_8(md);
        return ruxen_regex_option_none();
    }
    RuxenMatch *m = build_match_handle(r, text, md, rc);
    pcre2_match_data_free_8(md);
    return ruxen_regex_option_some((int64_t)(intptr_t)m);
}

/* ── Match accessors ─────────────────────────────────────────────── */

char *ruxen_match_matched(RuxenMatch *m) {
    if (!m || m->ovector_count < 1) return NULL;
    PCRE2_SIZE start = m->ovector_copy[0];
    PCRE2_SIZE end   = m->ovector_copy[1];
    size_t n = end - start;
    char *buf = (char *)ruxen_alloc(n + 1);
    memcpy(buf, m->subject_copy + start, n);
    buf[n] = '\0';
    return buf;
}

int64_t ruxen_match_start(RuxenMatch *m) {
    if (!m || m->ovector_count < 1) return 0;
    return (int64_t)m->ovector_copy[0];
}

int64_t ruxen_match_end(RuxenMatch *m) {
    if (!m || m->ovector_count < 1) return 0;
    return (int64_t)m->ovector_copy[1];
}

void *ruxen_match_group(RuxenMatch *m, int64_t n) {
    if (!m || n < 0 || n >= m->ovector_count) return ruxen_regex_option_none();
    PCRE2_SIZE start = m->ovector_copy[n * 2];
    PCRE2_SIZE end   = m->ovector_copy[n * 2 + 1];
    /* PCRE2 marks non-participating groups with PCRE2_UNSET. */
    if (start == PCRE2_UNSET || end == PCRE2_UNSET) {
        return ruxen_regex_option_none();
    }
    size_t len = end - start;
    char *buf = (char *)ruxen_alloc(len + 1);
    memcpy(buf, m->subject_copy + start, len);
    buf[len] = '\0';
    return ruxen_regex_option_some((int64_t)(intptr_t)buf);
}

void *ruxen_match_named(RuxenMatch *m, const char *name) {
    if (!m || !m->named_table_copy || !name) return ruxen_regex_option_none();
    for (int i = 0; i < m->named_count; i++) {
        const char *entry = m->named_table_copy + (size_t)i * m->named_entry_size;
        /* PCRE2 name-table entry layout: 2-byte big-endian group index
         * followed by the NUL-terminated name. */
        if (strcmp(entry + 2, name) == 0) {
            uint16_t group_idx = (uint16_t)((unsigned char)entry[0] << 8)
                               | (uint16_t)(unsigned char)entry[1];
            return ruxen_match_group(m, (int64_t)group_idx);
        }
    }
    return ruxen_regex_option_none();
}

/* groups → Array[Option[String]], one entry per ovector pair. */
void *ruxen_match_groups(RuxenMatch *m) {
    RuxenVec *vec = ruxen_vec_new();
    if (!m) return vec;
    for (int i = 0; i < m->ovector_count; i++) {
        void *opt = ruxen_match_group(m, (int64_t)i);
        ruxen_vec_push(vec, (int64_t)(intptr_t)opt);
    }
    return vec;
}

/* named_groups → HashMap[String, String], one entry per named capture
 * that participated in the match. Non-participating named groups are
 * omitted (matches the Option-elision pattern from .group()). */
void *ruxen_match_named_groups(RuxenMatch *m) {
    RuxenHash *map = ruxen_hash_new();
    if (!m || !m->named_table_copy) return map;
    for (int i = 0; i < m->named_count; i++) {
        const char *entry = m->named_table_copy + (size_t)i * m->named_entry_size;
        const char *name = entry + 2;
        uint16_t group_idx = (uint16_t)((unsigned char)entry[0] << 8)
                           | (uint16_t)(unsigned char)entry[1];
        if ((int)group_idx >= m->ovector_count) continue;
        PCRE2_SIZE start = m->ovector_copy[group_idx * 2];
        PCRE2_SIZE end   = m->ovector_copy[group_idx * 2 + 1];
        if (start == PCRE2_UNSET) continue;
        size_t len = end - start;
        char *buf = (char *)ruxen_alloc(len + 1);
        memcpy(buf, m->subject_copy + start, len);
        buf[len] = '\0';
        /* Key is a copy of the name so the map outlives the named-table
         * snapshot if the Match is dropped before the map. */
        size_t name_len = strlen(name);
        char *name_copy = (char *)ruxen_alloc(name_len + 1);
        memcpy(name_copy, name, name_len + 1);
        ruxen_hash_insert(map,
                          (int64_t)(intptr_t)name_copy,
                          (int64_t)(intptr_t)buf);
    }
    return map;
}

/* ── scan / replace / replace_all / split ────────────────────────── */

void *ruxen_regex_scan(pcre2_code_8 *r, const char *text) {
    RuxenVec *vec = ruxen_vec_new();
    if (!r || !text) return vec;
    pcre2_match_data_8 *md = pcre2_match_data_create_from_pattern_8(r, NULL);
    PCRE2_SIZE offset = 0;
    size_t tlen = strlen(text);
    while (offset <= tlen) {
        int rc = pcre2_match_8(r, (PCRE2_SPTR)text, tlen, offset, 0, md, NULL);
        if (rc < 0) break;
        RuxenMatch *m = build_match_handle(r, text, md, rc);
        ruxen_vec_push(vec, (int64_t)(intptr_t)m);
        PCRE2_SIZE *ov = pcre2_get_ovector_pointer_8(md);
        PCRE2_SIZE start = ov[0], end = ov[1];
        if (end == start) {
            /* Zero-width match: advance by one to avoid infinite loop
             * on patterns like /(?=…)/ or /\b/. */
            offset = start + 1;
        } else {
            offset = end;
        }
    }
    pcre2_match_data_free_8(md);
    return vec;
}

/* Substitute helper used by both replace and replace_all. Probes the
 * required output length with PCRE2_SUBSTITUTE_OVERFLOW_LENGTH first,
 * then allocates exactly the right buffer and does the real call. */
static char *do_substitute(pcre2_code_8 *r, const char *text,
                           const char *repl, uint32_t opts) {
    PCRE2_SIZE outlen = 0;
    /* Probe: with OVERFLOW_LENGTH, PCRE2 fills outlen with the bytes
     * needed (including NUL) and returns PCRE2_ERROR_NOMEMORY. */
    pcre2_substitute_8(r, (PCRE2_SPTR)text, PCRE2_ZERO_TERMINATED,
                       0, opts | PCRE2_SUBSTITUTE_OVERFLOW_LENGTH,
                       NULL, NULL,
                       (PCRE2_SPTR)repl, PCRE2_ZERO_TERMINATED,
                       NULL, &outlen);
    char *out = (char *)ruxen_alloc(outlen + 1);
    pcre2_substitute_8(r, (PCRE2_SPTR)text, PCRE2_ZERO_TERMINATED,
                       0, opts,
                       NULL, NULL,
                       (PCRE2_SPTR)repl, PCRE2_ZERO_TERMINATED,
                       (PCRE2_UCHAR *)out, &outlen);
    out[outlen] = '\0';
    return out;
}

char *ruxen_regex_replace(pcre2_code_8 *r, const char *text, const char *repl) {
    if (!r || !text || !repl) return NULL;
    return do_substitute(r, text, repl, 0);
}

char *ruxen_regex_replace_all(pcre2_code_8 *r, const char *text, const char *repl) {
    if (!r || !text || !repl) return NULL;
    return do_substitute(r, text, repl, PCRE2_SUBSTITUTE_GLOBAL);
}

void *ruxen_regex_split(pcre2_code_8 *r, const char *text) {
    RuxenVec *out = ruxen_vec_new();
    if (!r || !text) return out;
    pcre2_match_data_8 *md = pcre2_match_data_create_from_pattern_8(r, NULL);
    size_t tlen = strlen(text);
    PCRE2_SIZE prev_end = 0;
    PCRE2_SIZE offset = 0;
    while (offset <= tlen) {
        int rc = pcre2_match_8(r, (PCRE2_SPTR)text, tlen, offset, 0, md, NULL);
        if (rc < 0) break;
        PCRE2_SIZE *ov = pcre2_get_ovector_pointer_8(md);
        size_t seg_len = (size_t)(ov[0] - prev_end);
        char *seg = (char *)ruxen_alloc(seg_len + 1);
        memcpy(seg, text + prev_end, seg_len);
        seg[seg_len] = '\0';
        ruxen_vec_push(out, (int64_t)(intptr_t)seg);
        prev_end = ov[1];
        offset = (ov[1] == ov[0]) ? ov[1] + 1 : ov[1];
    }
    /* Trailing segment after the last match (or the whole string if
     * no match occurred). */
    size_t tail_len = tlen - prev_end;
    char *tail = (char *)ruxen_alloc(tail_len + 1);
    memcpy(tail, text + prev_end, tail_len);
    tail[tail_len] = '\0';
    ruxen_vec_push(out, (int64_t)(intptr_t)tail);
    pcre2_match_data_free_8(md);
    return out;
}
