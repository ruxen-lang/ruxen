// Tiny C symbol used by #06.8 Phase 2 link-smoke tests
// (compiler/riven_core/tests/ffi_c_symbol_wired.rs).
//
// `def add_one as "riven_test_extern_add_one"(x: I64) -> I64` in a Riven
// fixture binds the Riven-side name `add_one` to the linker symbol
// `riven_test_extern_add_one`. The compile+link+run smoke verifies that
// the call site emits a direct C call to this function and that the
// returned value rolls back through the Riven program correctly.
//
// Kept under the unity-build include block in runtime.c so the runtime
// `.o` always carries this symbol — production binaries pay zero cost
// (the symbol is small and unused outside tests), and the test path
// does not need a separate compile/link dance.

#include <stdint.h>

int64_t riven_test_extern_add_one(int64_t x) {
  return x + 1;
}

int64_t riven_test_extern_double(int64_t x) {
  return x * 2;
}
