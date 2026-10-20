//! Cranelift JIT code generation for the Ruxen REPL.
//!
//! The batch driver, `ruxen_core::codegen::cranelift::CodeGen`, over a
//! `JITModule`: this file only builds the module (a non-PIC host ISA, the
//! REPL's runtime overrides and the allowlisted `dlsym` lookup), then
//! finalizes what `CodeGen::compile_program` defined and hands out entry
//! addresses.

use cranelift_jit::{JITBuilder, JITModule};

use ruxen_core::codegen::cranelift::{isa, CodeGen};
use ruxen_core::mir::nodes::MirProgram;

/// Cranelift JIT code generation engine for the REPL.
pub struct JITCodeGen {
    codegen: CodeGen<JITModule>,
}

impl JITCodeGen {
    /// Create a new JIT code generator targeting the host machine.
    pub fn new() -> Result<Self, String> {
        // JIT code runs at the absolute addresses it was emitted for, so
        // it is not position independent.
        let mut jit_builder =
            JITBuilder::with_isa(isa(None, false)?, cranelift_module::default_libcall_names());

        // Register the REPL-only overrides: the print-family capture shims
        // and the session-slot helpers. Every other runtime symbol comes
        // from the dlsym fallback below.
        register_runtime_symbols(&mut jit_builder);

        // Fallback symbol lookup via libc::dlsym. The ruxen-repl binary
        // statically links ruxen-core's C runtime (see build.rs), so
        // every `ruxen_*` symbol is present in the process address
        // space and dlsym can resolve it without an explicit
        // builder.symbol() entry. This keeps the REPL in sync with new
        // runtime symbols without per-symbol maintenance.
        jit_builder.symbol_lookup_fn(Box::new(|name: &str| {
            // Security boundary: `dlsym(RTLD_DEFAULT)` resolves ANY
            // symbol visible in the host process, including libc.
            // Without this allowlist a `lib def system as "system"` line
            // in user input JITs straight to `libc::system`. Restrict
            // to symbols we actually expect: the `ruxen_*` runtime,
            // synth helpers like `<Mixin>_dynamic_<method>` /
            // `<Class>_<method>` / `Future_*` / `Formatter_*`, and a
            // small set of libc primitives the runtime itself depends
            // on (no exec / process-control surface).
            if !is_repl_symbol_allowed(name) {
                return None;
            }
            // SAFETY: dlsym on a NUL-terminated string returned by
            // CString::new. RTLD_DEFAULT searches the main program and
            // dependent dynlibs in the standard order.
            let c_name = match std::ffi::CString::new(name) {
                Ok(c) => c,
                Err(_) => return None,
            };
            unsafe {
                let handle = libc::RTLD_DEFAULT;
                let sym = libc::dlsym(handle, c_name.as_ptr());
                if sym.is_null() {
                    None
                } else {
                    Some(sym as *const u8)
                }
            }
        }));

        Ok(JITCodeGen {
            codegen: CodeGen::with_module(JITModule::new(jit_builder)),
        })
    }

    /// Compile every function and data symbol of `program` the JIT has not
    /// compiled yet and make them callable. The REPL lowers the whole
    /// session on every input, so all but the input's new code is skipped.
    pub fn compile_program(&mut self, program: &MirProgram) -> Result<(), String> {
        self.codegen.compile_program(program)?;
        self.codegen
            .module_mut()
            .finalize_definitions()
            .map_err(|e| format!("Failed to finalize: {}", e))
    }

    /// The callable address of the compiled function `name`.
    pub fn function_ptr(&self, name: &str) -> Result<*const u8, String> {
        let func_id = self
            .codegen
            .func_id(name)
            .ok_or_else(|| format!("Internal error: function '{name}' was not compiled"))?;
        Ok(self.codegen.module().get_finalized_function(func_id))
    }
}

// ── Runtime symbol registration ────────────────────────────────────

fn register_runtime_symbols(builder: &mut JITBuilder) {
    // Print family → capture shims so the REPL can diff cumulative stdout
    // between inputs and surface only the newest input's output.
    builder.symbol(
        "ruxen_puts",
        crate::capture::ruxen_repl_puts_shim as *const u8,
    );
    builder.symbol(
        "ruxen_print",
        crate::capture::ruxen_repl_print_shim as *const u8,
    );
    builder.symbol(
        "ruxen_eputs",
        crate::capture::ruxen_repl_eputs_shim as *const u8,
    );
    builder.symbol(
        "ruxen_print_int",
        crate::capture::ruxen_repl_print_int_shim as *const u8,
    );
    builder.symbol(
        "ruxen_print_float",
        crate::capture::ruxen_repl_print_float_shim as *const u8,
    );
    // Session-variable slot helpers — invoked by the synthetic
    // prefix/suffix injection in `eval::build_program` (Phase 1
    // Approach A). Keep them adjacent to the puts/print shims so
    // future maintainers see all REPL-only runtime overrides in
    // one place.
    builder.symbol(
        "ruxen_repl_slot_load_i64",
        crate::slots::ruxen_repl_slot_load_i64 as *const u8,
    );
    builder.symbol(
        "ruxen_repl_slot_store_i64",
        crate::slots::ruxen_repl_slot_store_i64 as *const u8,
    );
}

/// Allowlist for the REPL's `dlsym(RTLD_DEFAULT)` symbol-lookup fallback.
///
/// Without this gate any `lib def foo as "system"` line in user input
/// resolves to `libc::system` and the REPL becomes an arbitrary-code
/// execution surface. We accept:
///
/// 1. The `ruxen_*` runtime helpers (every stdlib entry point goes
///    through this prefix).
/// 2. Class-method-mangled symbols matching `<Class>_<method>` —
///    same shape Ruxen's own codegen emits via
///    `synthesize_dynamic_dispatch_helpers` and the formatter / future
///    runtimes.
/// 3. A small list of explicit libc primitives the runtime itself
///    needs at link time (no exec / process-spawn surface here).
///
/// Anything else returns `None`, which Cranelift surfaces as a
/// "linker failed to find …" error — visible to the user and harmless.
pub(super) fn is_repl_symbol_allowed(name: &str) -> bool {
    // Ruxen runtime helpers — the canonical surface.
    if name.starts_with("ruxen_") {
        return true;
    }
    // Mangled class / mixin methods follow `<TitleCase>_<lower_snake>`.
    // Accept anything starting with an ASCII uppercase letter that's
    // a plausible Rust-style identifier (no shell metacharacters).
    if name.starts_with(|c: char| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.contains('_')
    {
        return true;
    }
    // Specific libc primitives the runtime is allowed to look up by
    // name. Keep this list minimal — anything that can spawn a process,
    // execute a string, or open a socket is OFF.
    matches!(
        name,
        "memcpy"
            | "memmove"
            | "memset"
            | "memcmp"
            | "strlen"
            | "strcmp"
            | "strncmp"
            | "strcpy"
            | "strncpy"
            | "strchr"
            | "strrchr"
            | "malloc"
            | "calloc"
            | "realloc"
            | "free"
            | "abort"
            | "exit"
            | "puts"
            | "putchar"
            | "fputs"
            | "fputc"
            | "fwrite"
            | "fflush"
            // Math primitives the codegen may reference for f64 ops.
            | "fmod"
            | "fmodf"
            | "pow"
            | "powf"
            | "sqrt"
            | "sqrtf"
            | "floor"
            | "ceil"
            | "round"
    )
}

#[cfg(test)]
mod symbol_allowlist_tests {
    use super::is_repl_symbol_allowed;

    #[test]
    fn ruxen_runtime_symbols_pass() {
        assert!(is_repl_symbol_allowed("ruxen_string_from"));
        assert!(is_repl_symbol_allowed("ruxen_vec_new"));
        assert!(is_repl_symbol_allowed("ruxen_panic"));
    }

    #[test]
    fn mangled_class_methods_pass() {
        assert!(is_repl_symbol_allowed("Vec_push"));
        assert!(is_repl_symbol_allowed("Future_dynamic_poll"));
        assert!(is_repl_symbol_allowed("Formatter_write_str"));
    }

    #[test]
    fn allowed_libc_primitives_pass() {
        assert!(is_repl_symbol_allowed("memcpy"));
        assert!(is_repl_symbol_allowed("strlen"));
        assert!(is_repl_symbol_allowed("malloc"));
    }

    #[test]
    fn dangerous_libc_symbols_blocked() {
        // Process-spawn / exec surface — these MUST not be reachable
        // via dlsym fallback. A user-input `lib def system as "system"`
        // would otherwise JIT straight to arbitrary command execution.
        assert!(!is_repl_symbol_allowed("system"));
        assert!(!is_repl_symbol_allowed("execve"));
        assert!(!is_repl_symbol_allowed("execvp"));
        assert!(!is_repl_symbol_allowed("popen"));
        assert!(!is_repl_symbol_allowed("fork"));
        assert!(!is_repl_symbol_allowed("dlopen"));
        assert!(!is_repl_symbol_allowed("dlsym"));
    }

    #[test]
    fn lowercase_freefns_blocked() {
        // `lib def foo as "getenv"` style — lowercase identifiers don't
        // match the Ruxen mangling convention and aren't in the libc
        // allowlist, so they're refused.
        assert!(!is_repl_symbol_allowed("getenv"));
        assert!(!is_repl_symbol_allowed("setenv"));
        assert!(!is_repl_symbol_allowed("open"));
        assert!(!is_repl_symbol_allowed("read"));
        assert!(!is_repl_symbol_allowed("write"));
    }
}
