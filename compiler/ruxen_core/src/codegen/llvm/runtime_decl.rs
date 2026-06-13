//! Compiler-internal runtime declarations for the LLVM backend (residual).
//!
//! LLVM analogue of `cranelift/runtime_sigs.rs`. After
//! `docs/specs/system/zero_rust_stdlib_classes.spec.md`, the ABI of every
//! `ruxen_*` symbol a stdlib `.rx` `lib` block declares is DERIVED from the
//! declared `Ty`s by `emit::declare_ffi_function` (`ty_to_llvm` per param /
//! return). That runs BEFORE this function in `emit::compile_program`, so
//! every `.rx`-declared import is created from its derived signature; the
//! `module.get_function(name).is_none()` guard in the `decl!` macro then
//! skips any name already declared.
//!
//! What remains here is the IRREDUCIBLE residual: symbols emitted directly
//! by codegen / MIR lowering, declared by no `.rx` lib block (allocation,
//! `==`/`<=>`/hash lowering, drop glue, panic, the formatter-synthesis
//! surface, pointer-indirection helpers, the no-op sentinel, plus the
//! implicit conversion/accessor helpers). This is the SAME residual set the
//! Cranelift backend keeps in `compiler_internal_signature`; the parity
//! guard `compiler/ruxen_core/tests/runtime_abi_derivation.rs` pins it.
//!
//! Adding a stdlib runtime function NO LONGER touches this file: declare it
//! in the package's `.rx` + `.c` and its ABI is derived. Only a NEW
//! compiler-emitted-but-undeclared symbol belongs here.

use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::BasicMetadataTypeEnum;
use inkwell::AddressSpace;

/// Declare the compiler-internal residual runtime functions in the LLVM
/// module with External linkage. Idempotent per name (the `is_none` guard),
/// and called AFTER the derived FFI declarations so `.rx`-declared symbols
/// keep their derived signature.
pub fn declare_runtime_functions<'ctx>(module: &Module<'ctx>, context: &'ctx Context) {
    let i8_ty = context.i8_type();
    let i64_ty = context.i64_type();
    let f64_ty = context.f64_type();
    let ptr_ty = context.ptr_type(AddressSpace::default());
    let void_ty = context.void_type();

    macro_rules! decl {
        ($name:expr, void, [$($p:expr),*]) => {{
            let params: &[BasicMetadataTypeEnum] = &[$($p.into()),*];
            let fn_ty = void_ty.fn_type(params, false);
            if module.get_function($name).is_none() {
                module.add_function($name, fn_ty, Some(inkwell::module::Linkage::External));
            }
        }};
        ($name:expr, $ret:expr, [$($p:expr),*]) => {{
            let params: &[BasicMetadataTypeEnum] = &[$($p.into()),*];
            let fn_ty = $ret.fn_type(params, false);
            if module.get_function($name).is_none() {
                module.add_function($name, fn_ty, Some(inkwell::module::Linkage::External));
            }
        }};
    }

    // ── Implicit I/O entry points ───────────────────────────────────
    decl!("ruxen_puts", void, [ptr_ty]);
    decl!("ruxen_print", void, [ptr_ty]);
    decl!("ruxen_eputs", void, [ptr_ty]);
    decl!("ruxen_read_line", ptr_ty, []);
    decl!("ruxen_stdin", ptr_ty, []);
    decl!("ruxen_stdout", ptr_ty, []);
    decl!("ruxen_stderr", ptr_ty, []);
    decl!("ruxen_print_int", void, [i64_ty]);
    decl!("ruxen_env_init", void, [context.i32_type(), ptr_ty]);
    decl!("ruxen_env_args_count", i64_ty, []);
    decl!("ruxen_env_args_at", ptr_ty, [i64_ty]);

    // ── Implicit conversions (interpolation / `to_s` synthesis) ─────
    decl!("ruxen_int_to_string", ptr_ty, [i64_ty]);
    decl!("ruxen_float_to_string", ptr_ty, [f64_ty]);
    decl!("ruxen_bool_to_string", ptr_ty, [i64_ty]);
    decl!("ruxen_char_to_string", ptr_ty, [i64_ty]);
    decl!("ruxen_int_to_f", f64_ty, [i64_ty]);
    decl!("ruxen_float_to_i", i64_ty, [f64_ty]);

    // ── Formatter synthesis surface (Debug/Display derive) ──────────
    decl!("ruxen_float_to_string_prec", ptr_ty, [f64_ty, i64_ty]);
    decl!("ruxen_string_truncate_chars", ptr_ty, [ptr_ty, i64_ty]);
    decl!(
        "ruxen_fmt_formatter_new_with_spec",
        ptr_ty,
        [i64_ty, i64_ty, i64_ty, i64_ty]
    );
    decl!("ruxen_fmt_formatter_precision", i64_ty, [ptr_ty]);

    // ── Equality / ordering / hashing lowering (`==`, `<=>`) ────────
    decl!("ruxen_string_concat", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("ruxen_string_cmp", i64_ty, [ptr_ty, ptr_ty]);
    decl!("ruxen_string_hash", i64_ty, [ptr_ty]);
    decl!("ruxen_string_from_iter", ptr_ty, [ptr_ty]);
    decl!("ruxen_str_split", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("ruxen_str_parse_uint", ptr_ty, [ptr_ty]);
    decl!("ruxen_hash_eq", i8_ty, [ptr_ty, ptr_ty]);
    decl!("ruxen_hash_index", i64_ty, [ptr_ty, i64_ty]);
    decl!("ruxen_set_eq", i8_ty, [ptr_ty, ptr_ty]);

    // ── Indexing / element access emitted by `a[i]` lowering ────────
    decl!("ruxen_vec_get", i64_ty, [ptr_ty, i64_ty]);
    decl!("ruxen_vec_get_mut", ptr_ty, [ptr_ty, i64_ty]);
    decl!("ruxen_vec_get_mut_opt", ptr_ty, [ptr_ty, i64_ty]);
    decl!("ruxen_vec_set", void, [ptr_ty, i64_ty, i64_ty]);
    decl!("ruxen_vec_from_iter", ptr_ty, [ptr_ty]);

    // ── Vec constructors / iteration emitted by Array methods. These MUST
    //    be declared: without an explicit signature the call-site fallback
    //    (calls.rs) infers arg widths from values, which matches the C
    //    int64-slot ABI on 64-bit targets but passes a pointer-width (i32)
    //    item on wasm32 — an ABI mismatch with the runtime's int64_t
    //    parameters (tier 4.09). ──────────────────────────────────────────
    decl!("ruxen_vec_new", ptr_ty, []);
    decl!("ruxen_vec_push", void, [ptr_ty, i64_ty]);
    decl!("ruxen_vec_pop", ptr_ty, [ptr_ty]);
    decl!("ruxen_vec_len", i64_ty, [ptr_ty]);
    decl!("ruxen_vec_sum", i64_ty, [ptr_ty]);

    // ── Pointer indirection helpers (`&mut T` mutation lowering) ────
    decl!("ruxen_deref_ptr", ptr_ty, [ptr_ty]);
    decl!("ruxen_store_ptr", void, [ptr_ty, ptr_ty]);

    // ── Allocation + panic (core language runtime) ──────────────────
    decl!("ruxen_alloc", ptr_ty, [i64_ty]);
    decl!("ruxen_dealloc", void, [ptr_ty]);
    decl!("ruxen_realloc", ptr_ty, [ptr_ty, i64_ty]);
    decl!("ruxen_panic", void, [ptr_ty]);

    // ── Drop-glue selectors emitted by the MIR drop pass ────────────
    decl!("ruxen_string_free", void, [ptr_ty]);
    decl!("ruxen_vec_free", void, [ptr_ty]);
    decl!("ruxen_hash_free", void, [ptr_ty]);
    decl!("ruxen_set_free", void, [ptr_ty]);
    decl!("ruxen_hash_drop_string_v", void, [ptr_ty]);
    decl!("ruxen_hash_drop_v_string", void, [ptr_ty]);
    decl!("ruxen_hash_drop_string_string", void, [ptr_ty]);
    decl!("ruxen_hash_drop_v_vec", void, [ptr_ty]);
    decl!("ruxen_set_drop_string", void, [ptr_ty]);
    decl!("ruxen_vec_drop_string", void, [ptr_ty]);
    decl!("ruxen_vec_drop_vec", void, [ptr_ty]);

    // ── Result `?`-operator + closure-fallback helpers ──────────────
    decl!("ruxen_result_try_op", i64_ty, [ptr_ty]);
    decl!("ruxen_result_unwrap_or_else", i64_ty, [ptr_ty, ptr_ty]);

    // ── Implicit accessor/conversion helpers without a `.rx` decl ───
    // FOLLOW-UP (coverage gap, not a language primitive): see the
    // matching note in `cranelift/runtime_sigs.rs` — these should be
    // `.rx`-declared so they too become derived.
    decl!("ruxen_duration_as_secs", i64_ty, [ptr_ty]);
    decl!("ruxen_duration_as_millis", i64_ty, [ptr_ty]);
    decl!("ruxen_duration_as_micros", i64_ty, [ptr_ty]);
    decl!("ruxen_duration_as_nanos", i64_ty, [ptr_ty]);
    decl!("ruxen_thread_sleep_ns", void, [i64_ty]);
    decl!("ruxen_metadata_len", i64_ty, [ptr_ty]);
    decl!("ruxen_metadata_modified", i64_ty, [ptr_ty]);
    decl!("ruxen_metadata_is_file", i64_ty, [ptr_ty]);
    decl!("ruxen_metadata_is_dir", i64_ty, [ptr_ty]);
    decl!("ruxen_metadata_is_symlink", i64_ty, [ptr_ty]);
    decl!("ruxen_exit_status_code", i64_ty, [ptr_ty]);
    decl!("ruxen_exit_status_success", i64_ty, [ptr_ty]);
    decl!("ruxen_regex_compile_const", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("ruxen_regex_drop", void, [ptr_ty]);
    decl!("ruxen_regex_error_drop", void, [ptr_ty]);
    decl!("ruxen_match_drop", void, [ptr_ty]);

    // ── No-op sentinel ──────────────────────────────────────────────
    decl!("ruxen_noop", void, []);
}
