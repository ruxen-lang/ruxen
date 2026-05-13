//! Runtime function declarations for the LLVM backend.
//!
//! Declares all C runtime functions in the LLVM module so they can be
//! called from generated code.

use inkwell::context::Context;
use inkwell::module::Module;
use inkwell::types::BasicMetadataTypeEnum;
use inkwell::AddressSpace;

/// Declare all known runtime functions in the LLVM module.
///
/// Uses `add_function` with Import linkage — the linker resolves them
/// against the compiled C runtime object.
pub fn declare_runtime_functions<'ctx>(module: &Module<'ctx>, context: &'ctx Context) {
    let i8_ty = context.i8_type();
    let i64_ty = context.i64_type();
    let f64_ty = context.f64_type();
    let ptr_ty = context.ptr_type(AddressSpace::default());
    let void_ty = context.void_type();

    macro_rules! decl {
        ($name:expr, void, [$($p:expr),*]) => {
            {
                let params: &[BasicMetadataTypeEnum] = &[$($p.into()),*];
                let fn_ty = void_ty.fn_type(params, false);
                if module.get_function($name).is_none() {
                    module.add_function($name, fn_ty, Some(inkwell::module::Linkage::External));
                }
            }
        };
        ($name:expr, $ret:expr, [$($p:expr),*]) => {
            {
                let params: &[BasicMetadataTypeEnum] = &[$($p.into()),*];
                let fn_ty = $ret.fn_type(params, false);
                if module.get_function($name).is_none() {
                    module.add_function($name, fn_ty, Some(inkwell::module::Linkage::External));
                }
            }
        };
    }

    // I/O
    decl!("riven_puts", void, [ptr_ty]);
    decl!("riven_print", void, [ptr_ty]);
    decl!("riven_eputs", void, [ptr_ty]);
    decl!("riven_read_line", ptr_ty, []);
    decl!("riven_stdin", ptr_ty, []);
    decl!("riven_stdout", ptr_ty, []);
    decl!("riven_stderr", ptr_ty, []);
    decl!("riven_stdin_read_line", ptr_ty, [ptr_ty]);
    decl!("riven_stdin_read_to_string", ptr_ty, [ptr_ty]);
    decl!("riven_stdin_lines", ptr_ty, [ptr_ty]);
    // Phase 2 stdlib (#06.A3): std::fmt::Formatter buffer surface.
    decl!("riven_fmt_formatter_new", ptr_ty, []);
    decl!("riven_fmt_formatter_free", void, [ptr_ty]);
    decl!("riven_fmt_formatter_write_str", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_fmt_formatter_write_char", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_fmt_formatter_buffer", ptr_ty, [ptr_ty]);
    decl!("riven_fmt_formatter_len", i64_ty, [ptr_ty]);
    // Phase 2 stdlib (#06.D4): spec-aware Formatter constructor +
    // precision accessor + per-type precision helpers.
    decl!("riven_fmt_formatter_new_with_spec", ptr_ty, [i64_ty, i64_ty, i64_ty, i64_ty]);
    decl!("riven_fmt_formatter_precision", i64_ty, [ptr_ty]);
    decl!("riven_float_to_string_prec", ptr_ty, [f64_ty, i64_ty]);
    decl!("riven_string_truncate_chars", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_stdout_write_str", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_stdout_flush", ptr_ty, [ptr_ty]);
    decl!("riven_stderr_write_str", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_stderr_flush", ptr_ty, [ptr_ty]);
    decl!("riven_env_init", void, [context.i32_type(), ptr_ty]);
    decl!("riven_env_args_count", i64_ty, []);
    decl!("riven_env_args_at", ptr_ty, [i64_ty]);
    decl!("riven_env_args", ptr_ty, []);
    decl!("riven_env_var", ptr_ty, [ptr_ty]);
    decl!("riven_process_exit", void, [i64_ty]);
    decl!("riven_fs_read_to_string", ptr_ty, [ptr_ty]);
    decl!("riven_fs_write", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_fs_exists", i64_ty, [ptr_ty]);
    decl!("riven_fs_remove_file", ptr_ty, [ptr_ty]);
    decl!("riven_fs_create_dir", ptr_ty, [ptr_ty]);
    decl!("riven_fs_create_dir_all", ptr_ty, [ptr_ty]);
    decl!("riven_fs_rename", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_print_int", void, [i64_ty]);
    decl!("riven_print_float", void, [f64_ty]);

    // Conversions
    decl!("riven_int_to_string", ptr_ty, [i64_ty]);
    decl!("riven_float_to_string", ptr_ty, [f64_ty]);
    decl!("riven_bool_to_string", ptr_ty, [i64_ty]);

    // String operations
    decl!("riven_string_concat", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_string_from", ptr_ty, [ptr_ty]);
    decl!("riven_string_push_str", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_deref_ptr", ptr_ty, [ptr_ty]);
    decl!("riven_store_ptr", void, [ptr_ty, ptr_ty]);
    decl!("riven_string_len", i64_ty, [ptr_ty]);
    decl!("riven_string_is_empty", i8_ty, [ptr_ty]);
    decl!("riven_string_trim", ptr_ty, [ptr_ty]);
    decl!("riven_string_to_lower", ptr_ty, [ptr_ty]);
    decl!("riven_string_to_upper", ptr_ty, [ptr_ty]);
    decl!("riven_string_chars", ptr_ty, [ptr_ty]);
    decl!("riven_string_contains", i8_ty, [ptr_ty, ptr_ty]);
    decl!("riven_string_starts_with", i8_ty, [ptr_ty, ptr_ty]);
    decl!("riven_string_ends_with", i8_ty, [ptr_ty, ptr_ty]);
    decl!("riven_string_repeat", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_string_eq", i64_ty, [ptr_ty, ptr_ty]);
    decl!("riven_string_cmp", i64_ty, [ptr_ty, ptr_ty]);
    decl!("riven_string_hash", i64_ty, [ptr_ty]);
    decl!("riven_thread_sleep_ns", void, [i64_ty]);
    decl!("riven_thread_yield", void, []);
    decl!("riven_str_split", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_str_parse_uint", ptr_ty, [ptr_ty]);
    decl!("riven_iter_to_vec", ptr_ty, [ptr_ty]);
    decl!("riven_string_from_iter", ptr_ty, [ptr_ty]);

    // Memory
    decl!("riven_alloc", ptr_ty, [i64_ty]);
    decl!("riven_dealloc", void, [ptr_ty]);
    decl!("riven_realloc", ptr_ty, [ptr_ty, i64_ty]);
    // Heap-owned built-in drops (P0.7).
    decl!("riven_string_free", void, [ptr_ty]);
    decl!("riven_vec_free", void, [ptr_ty]);
    decl!("riven_hash_free", void, [ptr_ty]);
    // Phase 2 stdlib (#04 batch 2): set spine + HashMap/Set
    // per-element drop selectors.
    decl!("riven_set_free", void, [ptr_ty]);
    decl!("riven_hash_drop_string_v", void, [ptr_ty]);
    decl!("riven_hash_drop_v_string", void, [ptr_ty]);
    decl!("riven_hash_drop_string_string", void, [ptr_ty]);
    decl!("riven_hash_drop_v_vec", void, [ptr_ty]);
    decl!("riven_set_drop_string", void, [ptr_ty]);

    // Vec operations
    decl!("riven_vec_new", ptr_ty, []);
    decl!("riven_vec_push", void, [ptr_ty, i64_ty]);
    decl!("riven_vec_pop", ptr_ty, [ptr_ty]);
    decl!("riven_vec_len", i64_ty, [ptr_ty]);
    decl!("riven_vec_get", i64_ty, [ptr_ty, i64_ty]);
    decl!("riven_vec_get_opt", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_vec_get_mut", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_vec_get_mut_opt", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_vec_is_empty", i8_ty, [ptr_ty]);
    decl!("riven_vec_each", void, [ptr_ty, ptr_ty]);
    decl!("riven_vec_sum", i64_ty, [ptr_ty]);
    decl!("riven_vec_count", i64_ty, [ptr_ty]);
    decl!("riven_vec_reverse", ptr_ty, [ptr_ty]);
    decl!("riven_vec_first", ptr_ty, [ptr_ty]);
    decl!("riven_vec_last", ptr_ty, [ptr_ty]);
    decl!("riven_vec_clone", ptr_ty, [ptr_ty]);
    // Phase 2 stdlib (#05 batch 2): lazy iterator combinators.
    decl!("riven_vec_take", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_vec_skip", ptr_ty, [ptr_ty, i64_ty]);
    // Phase 2 stdlib (#05 batch 3): chain/zip eager-materialisers.
    decl!("riven_vec_chain", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_vec_zip", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_vec_contains_int", i8_ty, [ptr_ty, i64_ty]);
    decl!("riven_vec_sort", ptr_ty, [ptr_ty]);
    decl!("riven_vec_join", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_string_lines", ptr_ty, [ptr_ty]);
    decl!("riven_string_replace", ptr_ty, [ptr_ty, ptr_ty, ptr_ty]);
    // Phase 2 stdlib String additions (#02).
    decl!("riven_string_new", ptr_ty, []);
    decl!("riven_string_with_capacity", ptr_ty, [i64_ty]);
    decl!("riven_string_as_str", ptr_ty, [ptr_ty]);
    decl!("riven_string_to_string", ptr_ty, [ptr_ty]);
    decl!("riven_string_bytes", ptr_ty, [ptr_ty]);
    decl!("riven_string_trim_start", ptr_ty, [ptr_ty]);
    decl!("riven_string_trim_end", ptr_ty, [ptr_ty]);
    decl!("riven_string_find", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_string_splitn", ptr_ty, [ptr_ty, i64_ty, ptr_ty]);
    decl!("riven_string_clear", void, [ptr_ty]);
    decl!("riven_string_truncate", void, [ptr_ty, i64_ty]);
    decl!("riven_string_insert", ptr_ty, [ptr_ty, i64_ty, i64_ty]);
    decl!("riven_string_insert_str", ptr_ty, [ptr_ty, i64_ty, ptr_ty]);
    decl!("riven_string_remove", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_string_parse_int", ptr_ty, [ptr_ty]);
    decl!("riven_string_parse_float", ptr_ty, [ptr_ty]);
    decl!("riven_parse_error_message", ptr_ty, [ptr_ty]);
    // Phase 2 stdlib batch 2 (#02).
    decl!("riven_string_split", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_string_push", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_string_into_bytes", ptr_ty, [ptr_ty]);

    // Hash operations
    decl!("riven_hash_new", ptr_ty, []);
    decl!("riven_hash_from_iter", ptr_ty, [ptr_ty]);
    decl!("riven_hash_insert", void, [ptr_ty, i64_ty, i64_ty]);
    decl!("riven_hash_get", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_hash_contains_key", i8_ty, [ptr_ty, i64_ty]);
    decl!("riven_hash_len", i64_ty, [ptr_ty]);
    decl!("riven_hash_is_empty", i8_ty, [ptr_ty]);

    // Set operations
    decl!("riven_set_new", ptr_ty, []);
    decl!("riven_set_from_iter", ptr_ty, [ptr_ty]);
    decl!("riven_set_insert", void, [ptr_ty, i64_ty]);
    decl!("riven_set_contains", i8_ty, [ptr_ty, i64_ty]);
    decl!("riven_set_len", i64_ty, [ptr_ty]);
    decl!("riven_set_is_empty", i8_ty, [ptr_ty]);

    // Phase 2 stdlib (#04): HashMap[K,V] surface.
    decl!("riven_hash_with_capacity", ptr_ty, [i64_ty]);
    decl!("riven_hash_remove", ptr_ty, [ptr_ty, i64_ty]);
    decl!("riven_hash_clear", void, [ptr_ty]);
    decl!("riven_hash_keys", ptr_ty, [ptr_ty]);
    decl!("riven_hash_values", ptr_ty, [ptr_ty]);
    decl!("riven_hash_iter", ptr_ty, [ptr_ty]);
    decl!("riven_hash_eq", i8_ty, [ptr_ty, ptr_ty]);
    decl!("riven_hash_index", i64_ty, [ptr_ty, i64_ty]);

    // Phase 2 stdlib (#04): HashSet[T] surface.
    decl!("riven_set_with_capacity", ptr_ty, [i64_ty]);
    decl!("riven_set_remove", i8_ty, [ptr_ty, i64_ty]);
    decl!("riven_set_clear", void, [ptr_ty]);
    decl!("riven_set_iter", ptr_ty, [ptr_ty]);
    decl!("riven_set_eq", i8_ty, [ptr_ty, ptr_ty]);
    decl!("riven_set_union", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_set_intersection", ptr_ty, [ptr_ty, ptr_ty]);
    decl!("riven_set_difference", ptr_ty, [ptr_ty, ptr_ty]);

    // Option/Result helpers
    decl!("riven_option_unwrap_or", i64_ty, [ptr_ty, i64_ty]);
    decl!("riven_option_expect", i64_ty, [ptr_ty, ptr_ty]);
    decl!("riven_option_unwrap", i64_ty, [ptr_ty]);
    decl!("riven_option_is_some", i8_ty, [ptr_ty]);
    decl!("riven_option_is_none", i8_ty, [ptr_ty]);
    decl!("riven_result_unwrap_or_else", i64_ty, [ptr_ty, ptr_ty]);
    decl!("riven_result_try_op", i64_ty, [ptr_ty]);
    decl!("riven_result_expect", i64_ty, [ptr_ty, ptr_ty]);
    decl!("riven_result_unwrap", i64_ty, [ptr_ty]);
    decl!("riven_result_is_ok", i8_ty, [ptr_ty]);
    decl!("riven_result_is_err", i8_ty, [ptr_ty]);
    decl!("riven_result_ok", ptr_ty, [ptr_ty]);
    decl!("riven_result_err", ptr_ty, [ptr_ty]);
    decl!("riven_result_unwrap_or", i64_ty, [ptr_ty, i64_ty]);
    decl!("riven_option_ok_or", ptr_ty, [ptr_ty, i64_ty]);

    // Panic
    decl!("riven_panic", void, [ptr_ty]);
}
