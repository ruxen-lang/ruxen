#include "runtime.h"

// Tiny C symbol used by #06.8 Phase 2 link-smoke tests
// (compiler/ruxen_core/tests/ffi_c_symbol_wired.rs).
//
// `def add_one as "ruxen_test_extern_add_one"(x: I64) -> I64` in a Ruxen
// fixture binds the Ruxen-side name `add_one` to the linker symbol
// `ruxen_test_extern_add_one`. The compile+link+run smoke verifies that
// the call site emits a direct C call to this function and that the
// returned value rolls back through the Ruxen program correctly.
//
// Kept under the unity-build include block in runtime.c so the runtime
// `.o` always carries this symbol — production binaries pay zero cost
// (the symbol is small and unused outside tests), and the test path
// does not need a separate compile/link dance.

#include <stdint.h>

int64_t ruxen_test_extern_add_one(int64_t x) {
  return x + 1;
}

int64_t ruxen_test_extern_double(int64_t x) {
  return x * 2;
}

// Instance-method FFI proof-of-life (#06.8 Phase 3b follow-up).
// Ruxen instance methods passed through FFI receive `self` as the
// first argument (a pointer to the heap-allocated instance). This
// helper ignores `self` entirely and just adds 1 to its second arg,
// so the test only needs to verify that the call wires up — it does
// not depend on the class's field layout.
int64_t ruxen_test_extern_instance_passthrough(void* self_ptr, int64_t x) {
  (void)self_ptr;
  return x + 1;
}
