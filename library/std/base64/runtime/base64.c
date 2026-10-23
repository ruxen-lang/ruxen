/* std/base64 — Base64 encode/decode (Ruby's Base64 module surface).
 * Shims return a Ruxen String (char*) via ruxen_string_from. Input is a
 * NUL-terminated string's bytes; text/token round-trips are exact. */
#include <string.h>
#include <stdlib.h>

extern char *ruxen_string_from(const char *s);

static const char STD_ALPHA[] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
static const char URL_ALPHA[] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/* Encode `in` with `alpha`; if `newlines`, insert '\n' every 60 output chars
 * plus a trailing one (Ruby's encode64 MIME shape). */
static char *b64_encode(const char *in, const char *alpha, int newlines) {
    size_t n = strlen(in);
    const unsigned char *d = (const unsigned char *)in;
    size_t groups = (n + 2) / 3;
    size_t olen = groups * 4;
    if (newlines) olen += olen / 60 + 2;
    char *out = (char *)malloc(olen + 4);
    if (!out) return NULL;
    size_t o = 0, line = 0;
    for (size_t i = 0; i < n; i += 3) {
        unsigned b0 = d[i];
        unsigned b1 = (i + 1 < n) ? d[i + 1] : 0u;
        unsigned b2 = (i + 2 < n) ? d[i + 2] : 0u;
        unsigned v = (b0 << 16) | (b1 << 8) | b2;
        out[o++] = alpha[(v >> 18) & 63];
        out[o++] = alpha[(v >> 12) & 63];
        out[o++] = (i + 1 < n) ? alpha[(v >> 6) & 63] : '=';
        out[o++] = (i + 2 < n) ? alpha[v & 63] : '=';
        line += 4;
        if (newlines && line >= 60) { out[o++] = '\n'; line = 0; }
    }
    if (newlines && (n > 0)) out[o++] = '\n';
    out[o] = '\0';
    return out;
}

static int b64_val(unsigned char c, int urlsafe) {
    if (c >= 'A' && c <= 'Z') return c - 'A';
    if (c >= 'a' && c <= 'z') return c - 'a' + 26;
    if (c >= '0' && c <= '9') return c - '0' + 52;
    if (c == '+' || (urlsafe && c == '-')) return 62;
    if (c == '/' || (urlsafe && c == '_')) return 63;
    return -1; /* '=' padding, '\n', whitespace, invalid: skipped */
}

/* Decode `in`; ignores padding/newlines/invalid, so it handles both the MIME
 * and strict/urlsafe forms. */
static char *b64_decode(const char *in, int urlsafe) {
    size_t n = strlen(in);
    char *out = (char *)malloc(n + 1);
    if (!out) return NULL;
    size_t o = 0;
    unsigned buf = 0;
    int bits = 0;
    for (size_t i = 0; i < n; i++) {
        int v = b64_val((unsigned char)in[i], urlsafe);
        if (v < 0) continue;
        buf = (buf << 6) | (unsigned)v;
        bits += 6;
        if (bits >= 8) {
            bits -= 8;
            out[o++] = (char)((buf >> bits) & 0xFFu);
        }
    }
    out[o] = '\0';
    return out;
}

static char *box(char *raw) {
    if (!raw) return ruxen_string_from("");
    char *r = ruxen_string_from(raw);
    free(raw);
    return r;
}

char *ruxen_base64_strict_encode(const char *s)  { return box(b64_encode(s, STD_ALPHA, 0)); }
char *ruxen_base64_strict_decode(const char *s)  { return box(b64_decode(s, 0)); }
char *ruxen_base64_urlsafe_encode(const char *s) { return box(b64_encode(s, URL_ALPHA, 0)); }
char *ruxen_base64_urlsafe_decode(const char *s) { return box(b64_decode(s, 1)); }
char *ruxen_base64_encode64(const char *s)       { return box(b64_encode(s, STD_ALPHA, 1)); }
char *ruxen_base64_decode64(const char *s)       { return box(b64_decode(s, 0)); }
