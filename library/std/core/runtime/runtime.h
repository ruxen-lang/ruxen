/* Public surface of the Riven C runtime.
 *
 * Phase B of #06.75 introduced this header alongside the per-module
 * carve-out of runtime.c.  The unity-built `runtime.c` remains the
 * single translation unit consumed by every `cc::Build` invocation in
 * the workspace; this header is populated incrementally as callers
 * outside the unity build need declarations.
 *
 * For now the runtime is self-contained — every `riven_*` symbol that
 * a Rust codegen backend emits an `extern "C"` for is declared from
 * the Rust side via `cranelift_module::Module::declare_function` or
 * the LLVM `runtime_decl.rs` table.  If a future caller needs a
 * pure-C declaration of a runtime symbol, add it here.
 */

#ifndef RIVEN_RUNTIME_H
#define RIVEN_RUNTIME_H

#endif /* RIVEN_RUNTIME_H */
