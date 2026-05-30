/*
 * std::foobar runtime — trio-leak detector for B5 of
 * docs/specs/system/zero_rust_stdlib_classes.spec.md.
 *
 * Two symbols:
 *  - `ruxen_foobar_runtime_double(x)` — backs the module-scope free-fn
 *    `foobar_runtime_double` lib decl. Returns `x * 2`.
 *  - `ruxen_foobar_drop(self)` — backs the class-body `def drop` lib
 *    decl on `FooBar[T]`. Increments a process-global counter so a
 *    test can assert drop actually fired.
 *
 * The drop counter is exposed via the public symbol
 * `ruxen_foobar_drop_count` so user programs / test harnesses can
 * read it without needing extra IPC.
 */
#include <stdint.h>

/* The runtime header (library/std/core/runtime/runtime.h) is included
 * by every per-package C compile so cross-package types resolve. Not
 * strictly needed here since we only use stdint types, but kept for
 * consistency with peer packages. */
#include "../../core/runtime/runtime.h"

int64_t ruxen_foobar_drop_count = 0;

int64_t ruxen_foobar_runtime_double(int64_t x) {
    return x * 2;
}

void ruxen_foobar_drop(void *self) {
    (void)self;
    ruxen_foobar_drop_count += 1;
}
