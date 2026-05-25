//! Known runtime function signatures.
//!
//! Hand-rolled signature table for every `ruxen_*` C runtime helper that
//! `coerce_call_args` / `get_or_declare_func` consult to widen narrow-int
//! arguments correctly. Split out of the original monolithic
//! `cranelift.rs` for navigability — the contents are otherwise unchanged.

use cranelift_codegen::ir::types::{self, Type};

/// Known runtime function signatures.
///
/// Returns `(param_types, optional_return_type)`.
pub(super) fn runtime_signature(name: &str) -> Option<(Vec<Type>, Option<Type>)> {
    match name {
        // I/O
        "puts" | "ruxen_puts" => Some((vec![types::I64], None)),
        "eputs" | "ruxen_eputs" => Some((vec![types::I64], None)),
        "print" | "ruxen_print" => Some((vec![types::I64], None)),
        "println" => Some((vec![types::I64], None)),
        "eprintln" => Some((vec![types::I64], None)),
        "read_line" | "ruxen_read_line" => Some((vec![], Some(types::I64))),
        "stdin" | "ruxen_stdin" => Some((vec![], Some(types::I64))),
        "stdout" | "ruxen_stdout" => Some((vec![], Some(types::I64))),
        "stderr" | "ruxen_stderr" => Some((vec![], Some(types::I64))),
        "ruxen_stdin_read_line" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_stdin_read_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_stdin_lines" => Some((vec![types::I64], Some(types::I64))),
        // Phase 2 stdlib (#06.A3): std::fmt::Formatter buffer surface.
        "ruxen_fmt_formatter_new" => Some((vec![], Some(types::I64))),
        "ruxen_fmt_formatter_free" => Some((vec![types::I64], None)),
        "ruxen_fmt_formatter_write_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_write_char" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_buffer" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_len" => Some((vec![types::I64], Some(types::I64))),
        // Phase 2 stdlib (#06.D4): spec helpers.
        "ruxen_fmt_formatter_new_with_spec" => Some((
            vec![types::I64, types::I64, types::I64, types::I64],
            Some(types::I64),
        )),
        "ruxen_fmt_formatter_precision" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_float_to_string_prec" => Some((vec![types::F64, types::I64], Some(types::I64))),
        "ruxen_string_truncate_chars" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_stdout_write_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_stdout_flush" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_stderr_write_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_stderr_flush" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_env_init" => Some((vec![types::I32, types::I64], None)),
        "ruxen_env_args_count" => Some((vec![], Some(types::I64))),
        "ruxen_env_args_at" => Some((vec![types::I64], Some(types::I64))),
        "args" | "ruxen_env_args" => Some((vec![], Some(types::I64))),
        "ruxen_env_var" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_process_exit" => Some((vec![types::I64], None)),
        "ruxen_fs_read_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_write" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fs_exists" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_remove_file" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_create_dir" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_create_dir_all" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_rename" => Some((vec![types::I64, types::I64], Some(types::I64))),
        // Phase 2 stdlib (#06.5 T3): fs completeness.
        "ruxen_fs_copy" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fs_remove_dir_all" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_canonicalize" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_write_atomic" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fs_read_link" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_symlink" => Some((vec![types::I64, types::I64], Some(types::I64))),
        // Phase 2 stdlib (#06): fs::metadata + Metadata accessors.
        "ruxen_fs_metadata" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_modified" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_is_file" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_is_dir" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_is_symlink" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_free" => Some((vec![types::I64], None)),
        // Phase 2 stdlib (#06): std::process::Command builder + Output /
        // ExitStatus accessors.
        "ruxen_command_new" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_command_arg" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_command_args" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_command_env" => Some((vec![types::I64, types::I64, types::I64], Some(types::I64))),
        "ruxen_command_current_dir" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_command_status" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_command_output" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_command_drop" => Some((vec![types::I64], None)),
        "ruxen_exit_status_code" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_exit_status_success" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_exit_status_free" => Some((vec![types::I64], None)),
        "ruxen_output_stdout" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_output_stderr" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_output_status" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_output_drop" => Some((vec![types::I64], None)),
        // Phase 2 stdlib (#06.5 T2): File + OpenOptions surface.
        "ruxen_file_open" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_file_create" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_file_append" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_file_open_options" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_file_read" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_file_read_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_file_read_all" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_file_write" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_file_write_all" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_file_write_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_file_flush" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_file_seek" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_file_metadata" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_file_close" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_file_drop" => Some((vec![types::I64], None)),
        "ruxen_open_options_new" => Some((vec![], Some(types::I64))),
        "ruxen_open_options_read" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_open_options_write" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_open_options_append" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_open_options_truncate" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_open_options_create" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_open_options_create_new" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_print_int" => Some((vec![types::I64], None)),
        // Conversions
        "ruxen_int_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_float_to_string" => Some((vec![types::F64], Some(types::I64))),
        "ruxen_bool_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_char_to_string" => Some((vec![types::I64], Some(types::I64))),
        // String operations
        "ruxen_string_concat" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_from" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_push_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        // Pointer-to-pointer helpers used to implement &mut T mutation.
        "ruxen_deref_ptr" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_store_ptr" => Some((vec![types::I64, types::I64], None)),
        "ruxen_string_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_is_empty" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_string_trim" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_to_lower" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_to_upper" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_chars" => Some((vec![types::I64], Some(types::I64))),
        // String stdlib (#02): all char* / Vec ptr / Option / Result are
        // pointer-sized, so they ride the I64 calling convention.
        "ruxen_string_new" => Some((vec![], Some(types::I64))),
        "ruxen_string_with_capacity" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_as_str" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_to_string" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_bytes" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_trim_start" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_trim_end" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_find" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_splitn" => Some((vec![types::I64, types::I64, types::I64], Some(types::I64))),
        "ruxen_string_clear" => Some((vec![types::I64], None)),
        "ruxen_string_truncate" => Some((vec![types::I64, types::I64], None)),
        "ruxen_string_insert" => Some((vec![types::I64, types::I64, types::I64], Some(types::I64))),
        "ruxen_string_insert_str" => {
            Some((vec![types::I64, types::I64, types::I64], Some(types::I64)))
        }
        "ruxen_string_remove" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_parse_int" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_parse_float" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_parse_error_message" => Some((vec![types::I64], Some(types::I64))),
        // String stdlib batch 2 (#02): split / push / into_bytes.
        "ruxen_string_split" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_push" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_into_bytes" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_eq" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_cmp" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_hash" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_thread_sleep_ns" => Some((vec![types::I64], None)),
        "ruxen_thread_yield" => Some((vec![], None)),
        // Phase 2 stdlib (#06.5 T4): Duration / Instant ABI.
        //   `from_*` : (i64) -> ptr<RuxenDuration>
        //   `as_*`   : (ptr<RuxenDuration>) -> i64
        //   `add/sub`: (ptr, ptr) -> ptr<RuxenDuration>
        //   `Instant.now`                  : () -> ptr<RuxenInstant>
        //   `Instant.elapsed`              : (ptr<RuxenInstant>) -> ptr<RuxenDuration>
        //   `Instant.{duration_since,sub}` : (ptr, ptr) -> ptr<RuxenDuration>
        //   `sleep`                        : (ptr<RuxenDuration>) -> void
        // All pointers surface as I64 per the existing convention.
        "ruxen_duration_from_secs"
        | "ruxen_duration_from_millis"
        | "ruxen_duration_from_micros"
        | "ruxen_duration_from_nanos" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_duration_as_secs"
        | "ruxen_duration_as_millis"
        | "ruxen_duration_as_micros"
        | "ruxen_duration_as_nanos" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_duration_add" | "ruxen_duration_sub" => {
            Some((vec![types::I64, types::I64], Some(types::I64)))
        }
        "ruxen_instant_now" => Some((vec![], Some(types::I64))),
        "ruxen_instant_elapsed" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_instant_duration_since" | "ruxen_instant_sub" => {
            Some((vec![types::I64, types::I64], Some(types::I64)))
        }
        "ruxen_thread_sleep_duration" => Some((vec![types::I64], None)),
        "ruxen_str_split" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_str_parse_uint" => Some((vec![types::I64], Some(types::I64))),
        // Memory
        "ruxen_alloc" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_dealloc" => Some((vec![types::I64], None)),
        "ruxen_realloc" => Some((vec![types::I64, types::I64], Some(types::I64))),
        // Heap-owned built-in drops (P0.7).
        "ruxen_string_free" => Some((vec![types::I64], None)),
        "ruxen_vec_free" => Some((vec![types::I64], None)),
        "ruxen_hash_free" => Some((vec![types::I64], None)),
        // Phase 2 stdlib (#04 batch 2): set spine + HashMap/Set
        // per-element drop selectors.
        "ruxen_set_free" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_string_v" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_v_string" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_string_string" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_v_vec" => Some((vec![types::I64], None)),
        "ruxen_set_drop_string" => Some((vec![types::I64], None)),
        // Phase 2 stdlib batch 2 (#03): element-aware Vec drop helpers
        // and the new from_iter / dedup / set surface.
        "ruxen_vec_drop_string" => Some((vec![types::I64], None)),
        "ruxen_vec_drop_vec" => Some((vec![types::I64], None)),
        "ruxen_vec_from_iter" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_from_iter" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_vec_dedup" => Some((vec![types::I64], None)),
        "ruxen_vec_set" => Some((vec![types::I64, types::I64, types::I64], None)),
        // Panic
        "ruxen_panic" => Some((vec![types::I64], None)),
        // Vec operations
        "ruxen_vec_new" => Some((vec![], Some(types::I64))),
        "ruxen_vec_push" => Some((vec![types::I64, types::I64], None)),
        "ruxen_vec_pop" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_vec_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_vec_get" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_get_opt" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_get_mut" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_get_mut_opt" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_vec_is_empty" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_vec_each" => Some((vec![types::I64, types::I64], None)),
        // Hash operations
        "ruxen_hash_new" => Some((vec![], Some(types::I64))),
        "ruxen_hash_from_iter" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_insert" => Some((vec![types::I64, types::I64, types::I64], None)),
        "ruxen_hash_get" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_hash_contains_key" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "ruxen_hash_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_is_empty" => Some((vec![types::I64], Some(types::I8))),
        // Set operations
        "ruxen_set_new" => Some((vec![], Some(types::I64))),
        "ruxen_set_from_iter" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_set_insert" => Some((vec![types::I64, types::I64], None)),
        "ruxen_set_contains" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "ruxen_set_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_set_is_empty" => Some((vec![types::I64], Some(types::I8))),
        // Phase 2 stdlib (#04): HashMap[K,V] full surface.
        "ruxen_hash_with_capacity" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_remove" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_hash_clear" => Some((vec![types::I64], None)),
        "ruxen_hash_keys" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_values" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_iter" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_eq" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "ruxen_hash_index" => Some((vec![types::I64, types::I64], Some(types::I64))),
        // Phase 2 stdlib (#04): HashSet[T] full surface.
        "ruxen_set_with_capacity" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_set_remove" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "ruxen_set_clear" => Some((vec![types::I64], None)),
        "ruxen_set_iter" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_set_eq" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "ruxen_set_union" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_set_intersection" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_set_difference" => Some((vec![types::I64, types::I64], Some(types::I64))),
        // Option/Result helpers
        "ruxen_option_unwrap_or" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_result_unwrap_or_else" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_result_try_op" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_result_expect" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_result_unwrap" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_option_expect" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_option_unwrap" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_result_ok" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_result_err" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_option_is_some" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_option_is_none" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_result_is_ok" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_result_is_err" => Some((vec![types::I64], Some(types::I8))),
        // No-ops: these are declared via call-site inference (variable arity).
        "ruxen_noop" => Some((vec![], None)),
        _ => None,
    }
}
