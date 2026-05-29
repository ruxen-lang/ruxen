/* library/std/regex/runtime/regex.c
 *
 * std::regex C runtime — wraps the vendored PCRE2 under pcre2/.
 * Scaffolding only; real exports land in Phase 1 of the plan.
 */
#define PCRE2_CODE_UNIT_WIDTH 8
#include "../../core/runtime/runtime.h"
#include "pcre2/pcre2.h"
