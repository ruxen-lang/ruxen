/* Ruxen-vendored PCRE2 config.h. Hand-authored for the subset we
 * compile: 8-bit width, no JIT, no readline, thread-safe defaults.
 * If you upgrade vendored PCRE2 (currently 10.44), regenerate this by
 * running `./configure` against the upstream source on a Linux dev
 * machine, then port any new HAVE_* macros over.
 */

#ifndef RUXEN_PCRE2_CONFIG_H
#define RUXEN_PCRE2_CONFIG_H

#define HAVE_SYS_TYPES_H 1
#define HAVE_SYS_STAT_H  1
#define HAVE_STDLIB_H    1
#define HAVE_STRING_H    1
#define HAVE_MEMORY_H    1
#define HAVE_STRINGS_H   1
#define HAVE_INTTYPES_H  1
#define HAVE_STDINT_H    1
#define HAVE_UNISTD_H    1
#define HAVE_LIMITS_H    1
#define HAVE_DIRENT_H    1
#define HAVE_BCOPY       1
#define HAVE_MEMMOVE     1
#define HAVE_STRERROR    1

#define LINK_SIZE        2
#define MATCH_LIMIT      10000000
#define MATCH_LIMIT_DEPTH 10000000
#define HEAP_LIMIT       20000000
#define PARENS_NEST_LIMIT 250
#define NEWLINE_DEFAULT  2   /* LF — best fit for typical text input */
#define PCRE2_STATIC     1
#define MAX_NAME_SIZE    32
#define MAX_NAME_COUNT   10000
/* Default maximum length, in code units, of a variable-length
 * lookbehind assertion. Matches upstream config.h.generic. */
#define MAX_VARLOOKBEHIND 255

#define SUPPORT_UNICODE  1
#define SUPPORT_PCRE2_8  1

/* PCRE2 10.44's pcre2_internal.h references PCRE2_EXPORT as a
 * symbol-visibility hook. Autotools defines it via configure; for
 * our vendored static build it's intentionally empty (matches
 * upstream config.h.generic line 288). Without this define, every
 * PCRE2_EXP_DECL expansion in pcre2.h fails with "unknown type name
 * 'PCRE2_EXPORT'". */
#define PCRE2_EXPORT
/* SUPPORT_JIT intentionally NOT defined — keeps the vendored
 * footprint smaller and avoids per-platform JIT codegen issues.
 * For a JIT-enabled future build, also pull in pcre2_jit_*.c. */

#define STDC_HEADERS     1
#define PACKAGE          "pcre2"
#define PACKAGE_BUGREPORT ""
#define PACKAGE_NAME     "PCRE2"
#define PACKAGE_STRING   "PCRE2 10.44"
#define PACKAGE_TARNAME  "pcre2"
#define PACKAGE_URL      ""
#define PACKAGE_VERSION  "10.44"
#define VERSION          "10.44"

#endif /* RUXEN_PCRE2_CONFIG_H */
