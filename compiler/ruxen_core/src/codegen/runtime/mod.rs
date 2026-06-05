//! Runtime function declarations and name mapping.
//!
//! Provides the shared `runtime_name()` mapping used by both Cranelift and
//! LLVM backends. The historical grouped "known runtime symbol" catalogue
//! (`symbols/`, ~273 strings) was retired by the
//! `docs/specs/system/zero_rust_stdlib_classes.spec.md` ABI-derivation
//! migration: the set of valid runtime symbols is now "present in the
//! lib-decl-derived FFI map (codegen Pass-0) or in the compiler-internal
//! residual" — the catalogue had no consumer and was deleted.

#[cfg(test)]
mod tests_migrated;
#[cfg(test)]
mod tests_resolve;

/// Path to the C runtime source file, relative to the ruxenc crate root.
pub const RUNTIME_C_SOURCE: &str = "runtime/runtime.c";

/// Extract the method name from a mangled `TypeName_method` string.
///
/// Handles generic types like `Vec[T]_push` by finding `]_` as the
/// type/method separator. For simple types, uses the first `_`.
pub fn extract_method_name(mangled: &str) -> &str {
    // Look for `]_` which signals end of generic type params.
    if let Some(pos) = mangled.rfind("]_") {
        &mangled[pos + 2..]
    } else if let Some(pos) = mangled.find('_') {
        &mangled[pos + 1..]
    } else {
        mangled
    }
}

/// Format a "no runtime symbol" diagnostic for a generic method call that
/// codegen could not resolve to a real implementation.
pub(super) fn unresolved_method_error(callee: &str, type_label: &str) -> String {
    let method = extract_method_name(callee);
    format!(
        "codegen: no runtime symbol for `{type_label}::{method}` (mangled `{callee}`) — \
         this method is declared in stdlib but not implemented; remove the call site \
         or add a real symbol to ruxen_runtime"
    )
}

/// Map Ruxen built-in function names to their runtime C names.
///
/// Handles both top-level functions (puts, eputs) and mangled method
/// names for built-in types (String_from, Vec_push, etc.).
///
/// Returns `Err(diagnostic)` when a generic method call cannot be resolved
/// to a real runtime symbol. This replaces the historical silent fallback
/// to `ruxen_noop_passthrough` which masked dozens of unimplemented methods
/// (`.fold`, `.sum`, `.collect`, `.map_err`, `.contains`, ...) behind a
/// no-op that happened to produce the expected output for some fixtures.
pub fn runtime_name(name: &str) -> Result<&str, String> {
    super::lang_intrinsics::runtime_name(name)
}
