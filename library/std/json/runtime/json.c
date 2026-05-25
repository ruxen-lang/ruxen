#include "../../core/runtime/runtime.h"

#include <ctype.h>
#include <limits.h>
#include <math.h>

/* std::json first slice.
 *
 * This is a strict, Ruxen-native JSON implementation. It borrows the
 * high-level shape that makes Oj fast (cursor parser, single-pass string
 * scanning, direct value construction) without depending on Oj's Ruby
 * extension ABI.
 */

#define RUXEN_JSON_NULL   0
#define RUXEN_JSON_BOOL   1
#define RUXEN_JSON_INT    2
#define RUXEN_JSON_FLOAT  3
#define RUXEN_JSON_STRING 4
#define RUXEN_JSON_ARRAY  5
#define RUXEN_JSON_OBJECT 6

#define RUXEN_JSON_ERROR_SYNTAX              0
#define RUXEN_JSON_ERROR_DEPTH_LIMIT         1
#define RUXEN_JSON_ERROR_INVALID_UTF8        2
#define RUXEN_JSON_ERROR_NUMBER_OUT_OF_RANGE 3

#define RUXEN_JSON_MAX_DEPTH 512

#ifndef RUXEN_HASH_RUNTIME_STRUCTS
#define RUXEN_HASH_RUNTIME_STRUCTS

typedef struct RuxenHashEntry {
    int64_t key;
    int64_t value;
    struct RuxenHashEntry *next;
} RuxenHashEntry;

struct RuxenHash {
    RuxenHashEntry **buckets;
    uint64_t bucket_count;
    uint64_t len;
    int8_t string_keys;
};

#endif

typedef struct {
    const char *cur;
    const char *end;
    int allow_comments;
    int allow_trailing_commas;
    int error_tag;
    const char *error_message;
} JsonParser;

typedef struct {
    char *buf;
    size_t len;
    size_t cap;
    int error_tag;
    int pretty;
    int indent;
} JsonOut;

static void *json_error(int tag, const char *message) {
    int64_t *err = (int64_t *)ruxen_alloc(16);
    *(int32_t *)err = tag;
    err[1] = (tag == RUXEN_JSON_ERROR_SYNTAX)
        ? (int64_t)ruxen_string_from(message ? message : "invalid JSON")
        : 0;
    return err;
}

static void json_parser_fail(JsonParser *p, int tag, const char *message) {
    if (p->error_message) return;
    p->error_tag = tag;
    p->error_message = message ? message : "invalid JSON";
}

static void *json_result_error(JsonParser *p) {
    return ruxen_result_err_value((int64_t)json_error(
        p->error_tag,
        p->error_message ? p->error_message : "invalid JSON"));
}

static void *json_value(int tag, int64_t payload) {
    int64_t *v = (int64_t *)ruxen_alloc(16);
    *(int32_t *)v = tag;
    v[1] = payload;
    return v;
}

static void *json_null(void) {
    return json_value(RUXEN_JSON_NULL, 0);
}

static void *json_bool(int value) {
    return json_value(RUXEN_JSON_BOOL, value ? 1 : 0);
}

static void *json_int(int64_t value) {
    return json_value(RUXEN_JSON_INT, value);
}

static void *json_float(double value) {
    union { double d; int64_t i; } u;
    u.d = value;
    return json_value(RUXEN_JSON_FLOAT, u.i);
}

void *ruxen_json_make_null(void) {
    return json_null();
}

void *ruxen_json_make_bool(int8_t value) {
    return json_bool(value != 0);
}

void *ruxen_json_make_int(int64_t value) {
    return json_int(value);
}

void *ruxen_json_make_float(double value) {
    return json_float(value);
}

void *ruxen_json_make_string(const char *value) {
    return json_value(RUXEN_JSON_STRING, (int64_t)ruxen_string_from(value ? value : ""));
}

void *ruxen_json_make_array(RuxenVec *items) {
    return json_value(RUXEN_JSON_ARRAY, (int64_t)(items ? items : ruxen_vec_new()));
}

void *ruxen_json_make_object(RuxenHash *fields) {
    RuxenHash *obj = fields ? fields : ruxen_hash_new();
    obj->string_keys = 1;
    return json_value(RUXEN_JSON_OBJECT, (int64_t)obj);
}

void *ruxen_json_make_empty_array(void) {
    return json_value(RUXEN_JSON_ARRAY, (int64_t)ruxen_vec_new());
}

void *ruxen_json_make_empty_object(void) {
    RuxenHash *obj = ruxen_hash_new();
    obj->string_keys = 1;
    return json_value(RUXEN_JSON_OBJECT, (int64_t)obj);
}

static void skip_ws(JsonParser *p) {
    while (p->cur < p->end) {
        unsigned char c = (unsigned char)*p->cur;
        if (c == ' ' || c == '\n' || c == '\r' || c == '\t') {
            p->cur++;
            continue;
        }
        if (!p->allow_comments || c != '/' || p->cur + 1 >= p->end) break;
        if (p->cur[1] == '/') {
            p->cur += 2;
            while (p->cur < p->end && *p->cur != '\n' && *p->cur != '\r') {
                p->cur++;
            }
            continue;
        }
        if (p->cur[1] == '*') {
            p->cur += 2;
            while (p->cur + 1 < p->end && !(p->cur[0] == '*' && p->cur[1] == '/')) {
                p->cur++;
            }
            if (p->cur + 1 >= p->end) {
                json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "unterminated block comment");
                return;
            }
            p->cur += 2;
            continue;
        }
        break;
    }
}

static int consume(JsonParser *p, char c) {
    if (p->cur < p->end && *p->cur == c) {
        p->cur++;
        return 1;
    }
    return 0;
}

static int consume_lit(JsonParser *p, const char *lit) {
    const char *s = p->cur;
    while (*lit) {
        if (s >= p->end || *s != *lit) return 0;
        s++;
        lit++;
    }
    p->cur = s;
    return 1;
}

static void out_init(JsonOut *out, int pretty, int indent) {
    out->len = 0;
    out->cap = 128;
    out->error_tag = -1;
    out->pretty = pretty;
    out->indent = indent < 0 ? 0 : indent;
    out->buf = (char *)malloc(out->cap);
    if (!out->buf) ruxen_panic("out of memory");
    out->buf[0] = '\0';
}

static void out_free(JsonOut *out) {
    if (out->buf) free(out->buf);
    out->buf = NULL;
    out->len = 0;
    out->cap = 0;
}

static void out_reserve(JsonOut *out, size_t extra) {
    if (out->error_tag >= 0) return;
    if (extra > SIZE_MAX - out->len - 1) {
        out->error_tag = RUXEN_JSON_ERROR_NUMBER_OUT_OF_RANGE;
        return;
    }
    size_t needed = out->len + extra + 1;
    if (needed <= out->cap) return;
    size_t next = out->cap;
    while (next < needed) {
        if (next > SIZE_MAX / 2) {
            out->error_tag = RUXEN_JSON_ERROR_NUMBER_OUT_OF_RANGE;
            return;
        }
        next *= 2;
    }
    char *buf = (char *)realloc(out->buf, next);
    if (!buf) ruxen_panic("out of memory");
    out->buf = buf;
    out->cap = next;
}

static void out_byte(JsonOut *out, char c) {
    out_reserve(out, 1);
    if (out->error_tag >= 0) return;
    out->buf[out->len++] = c;
    out->buf[out->len] = '\0';
}

static void out_mem(JsonOut *out, const char *s, size_t len) {
    out_reserve(out, len);
    if (out->error_tag >= 0) return;
    memcpy(out->buf + out->len, s, len);
    out->len += len;
    out->buf[out->len] = '\0';
}

static void out_cstr(JsonOut *out, const char *s) {
    if (!s) s = "";
    out_mem(out, s, strlen(s));
}

static void out_newline_indent(JsonOut *out, int depth) {
    if (!out->pretty) return;
    out_byte(out, '\n');
    int spaces = depth * out->indent;
    for (int i = 0; i < spaces; i++) out_byte(out, ' ');
}

static int hex_val(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return 10 + (c - 'a');
    if (c >= 'A' && c <= 'F') return 10 + (c - 'A');
    return -1;
}

static int parse_hex4(JsonParser *p, uint32_t *out) {
    if ((size_t)(p->end - p->cur) < 4) return 0;
    uint32_t v = 0;
    for (int i = 0; i < 4; i++) {
        int h = hex_val(p->cur[i]);
        if (h < 0) return 0;
        v = (v << 4) | (uint32_t)h;
    }
    p->cur += 4;
    *out = v;
    return 1;
}

static int append_utf8(JsonOut *out, uint32_t cp) {
    if (cp == 0 || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF)) {
        return 0;
    }
    if (cp <= 0x7F) {
        out_byte(out, (char)cp);
    } else if (cp <= 0x7FF) {
        out_byte(out, (char)(0xC0 | (cp >> 6)));
        out_byte(out, (char)(0x80 | (cp & 0x3F)));
    } else if (cp <= 0xFFFF) {
        out_byte(out, (char)(0xE0 | (cp >> 12)));
        out_byte(out, (char)(0x80 | ((cp >> 6) & 0x3F)));
        out_byte(out, (char)(0x80 | (cp & 0x3F)));
    } else {
        out_byte(out, (char)(0xF0 | (cp >> 18)));
        out_byte(out, (char)(0x80 | ((cp >> 12) & 0x3F)));
        out_byte(out, (char)(0x80 | ((cp >> 6) & 0x3F)));
        out_byte(out, (char)(0x80 | (cp & 0x3F)));
    }
    return out->error_tag < 0;
}

static char *parse_string(JsonParser *p) {
    if (!consume(p, '"')) {
        json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "expected string");
        return NULL;
    }

    JsonOut out;
    out_init(&out, 0, 0);

    while (p->cur < p->end) {
        unsigned char c = (unsigned char)*p->cur++;
        if (c == '"') {
            char *result = out.buf;
            return result;
        }
        if (c < 0x20) {
            out_free(&out);
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "control character in string");
            return NULL;
        }
        if (c != '\\') {
            out_byte(&out, (char)c);
            if (out.error_tag >= 0) {
                out_free(&out);
                json_parser_fail(p, out.error_tag, "string too large");
                return NULL;
            }
            continue;
        }

        if (p->cur >= p->end) {
            out_free(&out);
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "unterminated escape");
            return NULL;
        }
        char esc = *p->cur++;
        switch (esc) {
            case '"': out_byte(&out, '"'); break;
            case '\\': out_byte(&out, '\\'); break;
            case '/': out_byte(&out, '/'); break;
            case 'b': out_byte(&out, '\b'); break;
            case 'f': out_byte(&out, '\f'); break;
            case 'n': out_byte(&out, '\n'); break;
            case 'r': out_byte(&out, '\r'); break;
            case 't': out_byte(&out, '\t'); break;
            case 'u': {
                uint32_t cp;
                if (!parse_hex4(p, &cp)) {
                    out_free(&out);
                    json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "invalid unicode escape");
                    return NULL;
                }
                if (cp >= 0xD800 && cp <= 0xDBFF) {
                    if ((size_t)(p->end - p->cur) < 6 || p->cur[0] != '\\' || p->cur[1] != 'u') {
                        out_free(&out);
                        json_parser_fail(p, RUXEN_JSON_ERROR_INVALID_UTF8, "missing low surrogate");
                        return NULL;
                    }
                    p->cur += 2;
                    uint32_t low;
                    if (!parse_hex4(p, &low) || low < 0xDC00 || low > 0xDFFF) {
                        out_free(&out);
                        json_parser_fail(p, RUXEN_JSON_ERROR_INVALID_UTF8, "invalid low surrogate");
                        return NULL;
                    }
                    cp = 0x10000 + (((cp - 0xD800) << 10) | (low - 0xDC00));
                }
                if (!append_utf8(&out, cp)) {
                    out_free(&out);
                    json_parser_fail(p, RUXEN_JSON_ERROR_INVALID_UTF8, "invalid unicode codepoint");
                    return NULL;
                }
                break;
            }
            default:
                out_free(&out);
                json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "invalid escape");
                return NULL;
        }
    }

    out_free(&out);
    json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "unterminated string");
    return NULL;
}

static void *parse_value(JsonParser *p, int depth);

static void *parse_array(JsonParser *p, int depth) {
    if (!consume(p, '[')) {
        json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "expected array");
        return NULL;
    }
    RuxenVec *items = ruxen_vec_new();
    skip_ws(p);
    if (p->error_message) return NULL;
    if (consume(p, ']')) return json_value(RUXEN_JSON_ARRAY, (int64_t)items);

    for (;;) {
        skip_ws(p);
        if (p->error_message) return NULL;
        void *item = parse_value(p, depth + 1);
        if (p->error_message) return NULL;
        ruxen_vec_push(items, (int64_t)item);
        skip_ws(p);
        if (p->error_message) return NULL;
        if (consume(p, ']')) break;
        if (!consume(p, ',')) {
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "expected comma or closing bracket");
            return NULL;
        }
        skip_ws(p);
        if (p->error_message) return NULL;
        if (p->cur < p->end && *p->cur == ']') {
            if (p->allow_trailing_commas) {
                p->cur++;
                break;
            }
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "trailing comma in array");
            return NULL;
        }
    }

    return json_value(RUXEN_JSON_ARRAY, (int64_t)items);
}

static void *parse_object(JsonParser *p, int depth) {
    if (!consume(p, '{')) {
        json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "expected object");
        return NULL;
    }
    RuxenHash *obj = ruxen_hash_new();
    obj->string_keys = 1;
    skip_ws(p);
    if (p->error_message) return NULL;
    if (consume(p, '}')) return json_value(RUXEN_JSON_OBJECT, (int64_t)obj);

    for (;;) {
        skip_ws(p);
        if (p->error_message) return NULL;
        if (p->cur >= p->end || *p->cur != '"') {
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "expected object key");
            return NULL;
        }
        char *key = parse_string(p);
        if (p->error_message) return NULL;
        skip_ws(p);
        if (!consume(p, ':')) {
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "expected colon after object key");
            return NULL;
        }
        skip_ws(p);
        void *value = parse_value(p, depth + 1);
        if (p->error_message) return NULL;
        ruxen_hash_insert(obj, (int64_t)key, (int64_t)value);
        skip_ws(p);
        if (p->error_message) return NULL;
        if (consume(p, '}')) break;
        if (!consume(p, ',')) {
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "expected comma or closing brace");
            return NULL;
        }
        skip_ws(p);
        if (p->error_message) return NULL;
        if (p->cur < p->end && *p->cur == '}') {
            if (p->allow_trailing_commas) {
                p->cur++;
                break;
            }
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "trailing comma in object");
            return NULL;
        }
    }

    return json_value(RUXEN_JSON_OBJECT, (int64_t)obj);
}

static void *parse_number(JsonParser *p) {
    const char *start = p->cur;
    int is_float = 0;

    if (consume(p, '-')) {
        if (p->cur >= p->end) {
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "invalid number");
            return NULL;
        }
    }

    if (consume(p, '0')) {
        if (p->cur < p->end && isdigit((unsigned char)*p->cur)) {
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "leading zero in number");
            return NULL;
        }
    } else if (p->cur < p->end && *p->cur >= '1' && *p->cur <= '9') {
        while (p->cur < p->end && isdigit((unsigned char)*p->cur)) p->cur++;
    } else {
        json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "invalid number");
        return NULL;
    }

    if (p->cur < p->end && *p->cur == '.') {
        is_float = 1;
        p->cur++;
        if (p->cur >= p->end || !isdigit((unsigned char)*p->cur)) {
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "invalid fraction");
            return NULL;
        }
        while (p->cur < p->end && isdigit((unsigned char)*p->cur)) p->cur++;
    }

    if (p->cur < p->end && (*p->cur == 'e' || *p->cur == 'E')) {
        is_float = 1;
        p->cur++;
        if (p->cur < p->end && (*p->cur == '+' || *p->cur == '-')) p->cur++;
        if (p->cur >= p->end || !isdigit((unsigned char)*p->cur)) {
            json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "invalid exponent");
            return NULL;
        }
        while (p->cur < p->end && isdigit((unsigned char)*p->cur)) p->cur++;
    }

    size_t len = (size_t)(p->cur - start);
    char *tmp = (char *)malloc(len + 1);
    if (!tmp) ruxen_panic("out of memory");
    memcpy(tmp, start, len);
    tmp[len] = '\0';

    errno = 0;
    if (is_float) {
        char *endptr = NULL;
        double d = strtod(tmp, &endptr);
        if (errno == ERANGE || !endptr || *endptr != '\0' || !isfinite(d)) {
            free(tmp);
            json_parser_fail(p, RUXEN_JSON_ERROR_NUMBER_OUT_OF_RANGE, "number out of range");
            return NULL;
        }
        free(tmp);
        return json_float(d);
    }

    char *endptr = NULL;
    long long i = strtoll(tmp, &endptr, 10);
    if (errno == ERANGE || !endptr || *endptr != '\0') {
        free(tmp);
        json_parser_fail(p, RUXEN_JSON_ERROR_NUMBER_OUT_OF_RANGE, "integer out of range");
        return NULL;
    }
    free(tmp);
    return json_int((int64_t)i);
}

static void *parse_value(JsonParser *p, int depth) {
    if (depth > RUXEN_JSON_MAX_DEPTH) {
        json_parser_fail(p, RUXEN_JSON_ERROR_DEPTH_LIMIT, "JSON nesting depth exceeded");
        return NULL;
    }
    skip_ws(p);
    if (p->error_message) return NULL;
    if (p->cur >= p->end) {
        json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "unexpected end of input");
        return NULL;
    }
    char c = *p->cur;
    if (c == 'n' && consume_lit(p, "null")) return json_null();
    if (c == 't' && consume_lit(p, "true")) return json_bool(1);
    if (c == 'f' && consume_lit(p, "false")) return json_bool(0);
    if (c == '"') {
        char *s = parse_string(p);
        if (p->error_message) return NULL;
        return json_value(RUXEN_JSON_STRING, (int64_t)s);
    }
    if (c == '[') return parse_array(p, depth);
    if (c == '{') return parse_object(p, depth);
    if (c == '-' || (c >= '0' && c <= '9')) return parse_number(p);

    json_parser_fail(p, RUXEN_JSON_ERROR_SYNTAX, "unexpected token");
    return NULL;
}

static void *json_parse_with_options(const char *input, int allow_comments, int allow_trailing_commas) {
    if (!input) {
        return ruxen_result_err_value((int64_t)json_error(
            RUXEN_JSON_ERROR_SYNTAX, "input is null"));
    }

    JsonParser p;
    p.cur = input;
    p.end = input + strlen(input);
    p.allow_comments = allow_comments;
    p.allow_trailing_commas = allow_trailing_commas;
    p.error_tag = RUXEN_JSON_ERROR_SYNTAX;
    p.error_message = NULL;

    skip_ws(&p);
    if (p.error_message) return json_result_error(&p);
    void *value = parse_value(&p, 0);
    if (p.error_message) return json_result_error(&p);
    skip_ws(&p);
    if (p.error_message) return json_result_error(&p);
    if (p.cur != p.end) {
        json_parser_fail(&p, RUXEN_JSON_ERROR_SYNTAX, "trailing characters after JSON value");
        return json_result_error(&p);
    }
    return ruxen_result_ok_value((int64_t)value);
}

void *ruxen_json_parse(const char *input) {
    return json_parse_with_options(input, 1, 1);
}

void *ruxen_json_parse_strict(const char *input) {
    return json_parse_with_options(input, 0, 0);
}

static void stringify_value(JsonOut *out, int64_t raw, int depth);

static void stringify_string(JsonOut *out, const char *s) {
    out_byte(out, '"');
    if (!s) s = "";
    for (const unsigned char *p = (const unsigned char *)s; *p; p++) {
        switch (*p) {
            case '"': out_cstr(out, "\\\""); break;
            case '\\': out_cstr(out, "\\\\"); break;
            case '\b': out_cstr(out, "\\b"); break;
            case '\f': out_cstr(out, "\\f"); break;
            case '\n': out_cstr(out, "\\n"); break;
            case '\r': out_cstr(out, "\\r"); break;
            case '\t': out_cstr(out, "\\t"); break;
            default:
                if (*p < 0x20) {
                    char esc[7];
                    snprintf(esc, sizeof(esc), "\\u%04x", *p);
                    out_cstr(out, esc);
                } else {
                    out_byte(out, (char)*p);
                }
                break;
        }
    }
    out_byte(out, '"');
}

static void stringify_array(JsonOut *out, RuxenVec *v, int depth) {
    out_byte(out, '[');
    if (v && v->len > 0) {
        for (uint64_t i = 0; i < v->len; i++) {
            if (i > 0) out_byte(out, ',');
            out_newline_indent(out, depth + 1);
            stringify_value(out, v->data[i], depth + 1);
            if (out->error_tag >= 0) return;
        }
        out_newline_indent(out, depth);
    }
    out_byte(out, ']');
}

static void stringify_object(JsonOut *out, RuxenHash *h, int depth) {
    out_byte(out, '{');
    int first = 1;
    if (h) {
        for (uint64_t i = 0; i < h->bucket_count; i++) {
            for (RuxenHashEntry *e = h->buckets[i]; e; e = e->next) {
                if (!first) out_byte(out, ',');
                first = 0;
                out_newline_indent(out, depth + 1);
                stringify_string(out, (const char *)e->key);
                out_byte(out, ':');
                if (out->pretty) out_byte(out, ' ');
                stringify_value(out, e->value, depth + 1);
                if (out->error_tag >= 0) return;
            }
        }
    }
    if (!first) out_newline_indent(out, depth);
    out_byte(out, '}');
}

static void stringify_value(JsonOut *out, int64_t raw, int depth) {
    if (out->error_tag >= 0) return;
    if (depth > RUXEN_JSON_MAX_DEPTH) {
        out->error_tag = RUXEN_JSON_ERROR_DEPTH_LIMIT;
        return;
    }
    int64_t *v = (int64_t *)raw;
    if (!v) {
        out_cstr(out, "null");
        return;
    }
    int tag = *(int32_t *)v;
    switch (tag) {
        case RUXEN_JSON_NULL:
            out_cstr(out, "null");
            break;
        case RUXEN_JSON_BOOL:
            out_cstr(out, v[1] ? "true" : "false");
            break;
        case RUXEN_JSON_INT: {
            char buf[32];
            snprintf(buf, sizeof(buf), "%" PRId64, v[1]);
            out_cstr(out, buf);
            break;
        }
        case RUXEN_JSON_FLOAT: {
            union { double d; int64_t i; } u;
            u.i = v[1];
            if (!isfinite(u.d)) {
                out->error_tag = RUXEN_JSON_ERROR_NUMBER_OUT_OF_RANGE;
                return;
            }
            char buf[64];
            snprintf(buf, sizeof(buf), "%.17g", u.d);
            out_cstr(out, buf);
            break;
        }
        case RUXEN_JSON_STRING:
            stringify_string(out, (const char *)v[1]);
            break;
        case RUXEN_JSON_ARRAY:
            stringify_array(out, (RuxenVec *)v[1], depth);
            break;
        case RUXEN_JSON_OBJECT:
            stringify_object(out, (RuxenHash *)v[1], depth);
            break;
        default:
            out->error_tag = RUXEN_JSON_ERROR_SYNTAX;
            break;
    }
}

static void *json_stringify_with_options(void *value, int pretty, int64_t indent) {
    JsonOut out;
    if (indent > INT_MAX) {
        return ruxen_result_err_value((int64_t)json_error(
            RUXEN_JSON_ERROR_NUMBER_OUT_OF_RANGE, "indent is too large"));
    }
    out_init(&out, pretty, (int)indent);
    stringify_value(&out, (int64_t)value, 0);
    if (out.error_tag >= 0) {
        int tag = out.error_tag;
        out_free(&out);
        return ruxen_result_err_value((int64_t)json_error(tag, "invalid Json value"));
    }
    char *result = out.buf;
    return ruxen_result_ok_value((int64_t)result);
}

void *ruxen_json_stringify(void *value) {
    return json_stringify_with_options(value, 0, 0);
}

void *ruxen_json_stringify_pretty(void *value, int64_t indent) {
    if (indent < 0) indent = 0;
    return json_stringify_with_options(value, 1, indent);
}

static int json_tag(void *value) {
    if (!value) return RUXEN_JSON_NULL;
    return *(int32_t *)value;
}

static int64_t json_payload(void *value) {
    if (!value) return 0;
    return ((int64_t *)value)[1];
}

static void *option_none(void) {
    int64_t *out = (int64_t *)ruxen_alloc(16);
    *(int32_t *)out = 0;
    out[1] = 0;
    return out;
}

static void *option_some(int64_t payload) {
    int64_t *out = (int64_t *)ruxen_alloc(16);
    *(int32_t *)out = 1;
    out[1] = payload;
    return out;
}

void *ruxen_json_kind(void *value) {
    int tag = json_tag(value);
    if (tag < RUXEN_JSON_NULL || tag > RUXEN_JSON_OBJECT) tag = RUXEN_JSON_NULL;
    return json_value(tag, 0);
}

int8_t ruxen_json_is_null(void *value) {
    return json_tag(value) == RUXEN_JSON_NULL;
}

int8_t ruxen_json_is_bool(void *value) {
    return json_tag(value) == RUXEN_JSON_BOOL;
}

int8_t ruxen_json_is_int(void *value) {
    return json_tag(value) == RUXEN_JSON_INT;
}

int8_t ruxen_json_is_float(void *value) {
    return json_tag(value) == RUXEN_JSON_FLOAT;
}

int8_t ruxen_json_is_string(void *value) {
    return json_tag(value) == RUXEN_JSON_STRING;
}

int8_t ruxen_json_is_array(void *value) {
    return json_tag(value) == RUXEN_JSON_ARRAY;
}

int8_t ruxen_json_is_object(void *value) {
    return json_tag(value) == RUXEN_JSON_OBJECT;
}

void *ruxen_json_as_bool(void *value) {
    if (!ruxen_json_is_bool(value)) return option_none();
    return option_some(json_payload(value) ? 1 : 0);
}

void *ruxen_json_as_int(void *value) {
    if (!ruxen_json_is_int(value)) return option_none();
    return option_some(json_payload(value));
}

void *ruxen_json_as_float(void *value) {
    if (!ruxen_json_is_float(value)) return option_none();
    return option_some(json_payload(value));
}

void *ruxen_json_as_string(void *value) {
    if (!ruxen_json_is_string(value)) return option_none();
    return option_some((int64_t)ruxen_string_from((const char *)json_payload(value)));
}

void *ruxen_json_array_len(void *value) {
    if (!ruxen_json_is_array(value)) return option_none();
    RuxenVec *items = (RuxenVec *)json_payload(value);
    return option_some(items ? (int64_t)items->len : 0);
}

void *ruxen_json_object_len(void *value) {
    if (!ruxen_json_is_object(value)) return option_none();
    RuxenHash *obj = (RuxenHash *)json_payload(value);
    return option_some(obj ? (int64_t)obj->len : 0);
}

static RuxenHashEntry *json_object_find(RuxenHash *obj, const char *key) {
    if (!obj || !key || obj->bucket_count == 0) return NULL;
    uint64_t bucket_idx = ruxen_hash_str(key) % obj->bucket_count;
    for (RuxenHashEntry *e = obj->buckets[bucket_idx]; e; e = e->next) {
        const char *entry_key = (const char *)e->key;
        if (entry_key && strcmp(entry_key, key) == 0) return e;
    }
    return NULL;
}

int8_t ruxen_json_object_has(void *value, const char *key) {
    if (!ruxen_json_is_object(value)) return 0;
    return json_object_find((RuxenHash *)json_payload(value), key) ? 1 : 0;
}

void *ruxen_json_object_get(void *value, const char *key) {
    if (!ruxen_json_is_object(value)) return option_none();
    RuxenHashEntry *entry = json_object_find((RuxenHash *)json_payload(value), key);
    if (!entry) return option_none();
    return option_some(entry->value);
}
