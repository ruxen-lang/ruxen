//! Characterization / parity oracle for the C-runtime ABI WIDTH table.
//!
//! Context: `docs/specs/system/zero_rust_stdlib_classes.spec.md`. The
//! cranelift backend has historically carried a hand-rolled
//! `runtime_signature(name) -> (Vec<cranelift Type>, Option<Type>)` table
//! (`codegen/cranelift/runtime_sigs.rs`, ~239 symbols) listing the wire
//! ABI of every `ruxen_*` C runtime helper. The migration retires that
//! table for every symbol whose ABI is already DERIVABLE from its `.rx`
//! `lib` declaration — the resolver records the declared `Ty`s, and
//! `ty_to_cranelift` lowers each to its cranelift width.
//!
//! THIS TEST IS THE REGRESSION GUARD against silent width drift. It drives
//! a full stdlib bootstrap merge, derives every FFI symbol's `(params,
//! ret)` cranelift widths from the `.rx`-declared types exactly as codegen
//! Pass-0 does (`compile_program` in `codegen/cranelift/mod.rs`), and
//! asserts the derived widths equal the hand-rolled table for every symbol
//! that appears in BOTH sets. Symbols in the table but NOT derived are the
//! compiler-internal residual (emitted directly by codegen, declared by no
//! `.rx` block) — printed so the residual table's contents are auditable.
//!
//! Per global rule 41 the e2e gate caches under tmp/test-cache/; this is a
//! pure in-process unit test (no compile/link) so it runs in the narrow
//! `cargo test -p ruxen_core` sweep.

use std::collections::BTreeMap;

use ruxen_core::codegen::cranelift::ty_to_cranelift;
use ruxen_core::parser::ast::Program;
use ruxen_core::resolve::bootstrap::run_bootstrap_with_package_names;
use ruxen_core::resolve::Resolver;

use cranelift_codegen::ir::types::{self, Type};

/// FROZEN baseline of the hand-rolled `runtime_signature` ABI table as it
/// stood at the migration baseline (249 symbols). This is a VERBATIM copy
/// of `codegen/cranelift/runtime_sigs.rs::runtime_signature` BEFORE that
/// table was shrunk to the compiler-internal residual. It is embedded here
/// (not imported) so this parity guard keeps comparing derived widths
/// against the FULL original table even after production shrinks — a
/// shrink of the production residual cannot silently weaken the guard.
/// Do NOT "clean this up" to delegate to the production fn: the whole
/// point is that it is independent of it.
#[rustfmt::skip]
fn baseline_runtime_signature(name: &str) -> Option<(Vec<Type>, Option<Type>)> {
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
        "ruxen_fmt_formatter_new" => Some((vec![], Some(types::I64))),
        "ruxen_fmt_formatter_free" => Some((vec![types::I64], None)),
        "ruxen_fmt_formatter_write_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_write_char" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_buffer" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fmt_formatter_len" => Some((vec![types::I64], Some(types::I64))),
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
        "ruxen_fs_copy" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fs_remove_dir_all" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_canonicalize" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_write_atomic" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fs_read_link" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_fs_symlink" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_fs_metadata" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_modified" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_is_file" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_is_dir" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_is_symlink" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_metadata_free" => Some((vec![types::I64], None)),
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
        "ruxen_int_to_f" => Some((vec![types::I64], Some(types::F64))),
        "ruxen_float_to_i" => Some((vec![types::F64], Some(types::I64))),
        // String operations
        "ruxen_string_concat" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_from" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_push_str" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_deref_ptr" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_store_ptr" => Some((vec![types::I64, types::I64], None)),
        "ruxen_string_len" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_is_empty" => Some((vec![types::I64], Some(types::I8))),
        "ruxen_string_trim" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_to_lower" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_to_upper" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_chars" => Some((vec![types::I64], Some(types::I64))),
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
        "ruxen_string_split" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_push" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_into_bytes" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_string_eq" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_cmp" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_string_hash" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_thread_sleep_ns" => Some((vec![types::I64], None)),
        "ruxen_thread_yield" => Some((vec![], None)),
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
        "ruxen_string_free" => Some((vec![types::I64], None)),
        "ruxen_vec_free" => Some((vec![types::I64], None)),
        "ruxen_hash_free" => Some((vec![types::I64], None)),
        "ruxen_set_free" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_string_v" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_v_string" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_string_string" => Some((vec![types::I64], None)),
        "ruxen_hash_drop_v_vec" => Some((vec![types::I64], None)),
        "ruxen_set_drop_string" => Some((vec![types::I64], None)),
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
        "ruxen_hash_with_capacity" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_remove" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_hash_clear" => Some((vec![types::I64], None)),
        "ruxen_hash_keys" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_values" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_entries" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_hash_eq" => Some((vec![types::I64, types::I64], Some(types::I8))),
        "ruxen_hash_index" => Some((vec![types::I64, types::I64], Some(types::I64))),
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
        // std::regex
        "ruxen_regex_new" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_regex_compile_const" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_regex_is_match" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_regex_match" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_regex_scan" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_regex_replace" => Some((vec![types::I64, types::I64, types::I64], Some(types::I64))),
        "ruxen_regex_replace_all" => {
            Some((vec![types::I64, types::I64, types::I64], Some(types::I64)))
        }
        "ruxen_regex_split" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_regex_drop" => Some((vec![types::I64], None)),
        "ruxen_regex_error_message" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_regex_error_offset" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_regex_error_drop" => Some((vec![types::I64], None)),
        "ruxen_match_matched" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_match_start" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_match_end" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_match_group" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_match_named" => Some((vec![types::I64, types::I64], Some(types::I64))),
        "ruxen_match_groups" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_match_named_groups" => Some((vec![types::I64], Some(types::I64))),
        "ruxen_match_drop" => Some((vec![types::I64], None)),
        "ruxen_noop" => Some((vec![], None)),
        _ => None,
    }
}

/// The cranelift `(param widths, optional ret width)` of one FFI symbol,
/// derived from its `.rx`-declared `Ty`s the same way codegen Pass-0
/// declares the `Linkage::Import`: map each declared param `Ty` through
/// `ty_to_cranelift` (dropping zero-width `Unit`/`Never`), and the return
/// `Ty` likewise.
type AbiShape = (Vec<Type>, Option<Type>);

/// Drive a full stdlib bootstrap merge with an EMPTY user program and
/// collect, keyed by the LINKED C symbol (`c_symbol` if aliased, else
/// `ruxen_name`), the cranelift ABI shape derived from each
/// `HirFfiFunc`'s declared `param_types` / `return_type`.
///
/// `param_types` already has the instance-method receiver prepended (the
/// resolver does this in `register_class_lib_method_in`), so the derived
/// arity matches what codegen Pass-0 sees — and therefore what the
/// hand-rolled table must match for instance-method symbols like
/// `ruxen_string_len` (1 param = the `&String` receiver).
fn derived_abi_map() -> BTreeMap<String, AbiShape> {
    let mut diags = Vec::new();
    let bootstrap_packages = run_bootstrap_with_package_names(&mut diags);
    assert!(
        diags.is_empty(),
        "stdlib bootstrap emitted diagnostics: {diags:?}"
    );

    // Empty user program — we only want the merged-in stdlib FFI surface.
    let empty = Program {
        items: Vec::new(),
        span: ruxen_core::lexer::token::Span {
            start: 0,
            end: 0,
            line: 0,
            column: 0,
        },
    };
    let result = Resolver::new().resolve_with_bootstrap_packages(&empty, &bootstrap_packages);

    let mut map: BTreeMap<String, AbiShape> = BTreeMap::new();
    for lib in &result.program.ffi_libs {
        for f in &lib.functions {
            let linked = f.c_symbol.clone().unwrap_or_else(|| f.ruxen_name.clone());
            let params: Vec<Type> = f.param_types.iter().filter_map(ty_to_cranelift).collect();
            let ret: Option<Type> = f.return_type.as_ref().and_then(ty_to_cranelift);
            // First decl wins, mirroring `extern_symbol_table`'s
            // `or_insert_with` and the E0722 conflict guard (a matching
            // redeclaration is a no-op).
            map.entry(linked).or_insert((params, ret));
        }
    }
    map
}

/// Every symbol the hand-rolled `runtime_signature` table knows, paired
/// with its table ABI shape. The table also keys some convenience aliases
/// (`puts`, `print`, `args`, …) that are not `ruxen_*` C symbols; those
/// are never `.rx`-declared and fall into the residual by construction.
fn table_symbols() -> &'static [&'static str] {
    // Transcribed from runtime_sigs.rs as of the migration baseline. Kept
    // as an explicit list (not parsed) so a deletion from the table is a
    // visible diff here too.
    &[
        "puts",
        "ruxen_puts",
        "eputs",
        "ruxen_eputs",
        "print",
        "ruxen_print",
        "println",
        "eprintln",
        "read_line",
        "ruxen_read_line",
        "stdin",
        "ruxen_stdin",
        "stdout",
        "ruxen_stdout",
        "stderr",
        "ruxen_stderr",
        "ruxen_stdin_read_line",
        "ruxen_stdin_read_to_string",
        "ruxen_stdin_lines",
        "ruxen_fmt_formatter_new",
        "ruxen_fmt_formatter_free",
        "ruxen_fmt_formatter_write_str",
        "ruxen_fmt_formatter_write_char",
        "ruxen_fmt_formatter_buffer",
        "ruxen_fmt_formatter_len",
        "ruxen_fmt_formatter_new_with_spec",
        "ruxen_fmt_formatter_precision",
        "ruxen_float_to_string_prec",
        "ruxen_string_truncate_chars",
        "ruxen_stdout_write_str",
        "ruxen_stdout_flush",
        "ruxen_stderr_write_str",
        "ruxen_stderr_flush",
        "ruxen_env_init",
        "ruxen_env_args_count",
        "ruxen_env_args_at",
        "args",
        "ruxen_env_args",
        "ruxen_env_var",
        "ruxen_process_exit",
        "ruxen_fs_read_to_string",
        "ruxen_fs_write",
        "ruxen_fs_exists",
        "ruxen_fs_remove_file",
        "ruxen_fs_create_dir",
        "ruxen_fs_create_dir_all",
        "ruxen_fs_rename",
        "ruxen_fs_copy",
        "ruxen_fs_remove_dir_all",
        "ruxen_fs_canonicalize",
        "ruxen_fs_write_atomic",
        "ruxen_fs_read_link",
        "ruxen_fs_symlink",
        "ruxen_fs_metadata",
        "ruxen_metadata_len",
        "ruxen_metadata_modified",
        "ruxen_metadata_is_file",
        "ruxen_metadata_is_dir",
        "ruxen_metadata_is_symlink",
        "ruxen_metadata_free",
        "ruxen_command_new",
        "ruxen_command_arg",
        "ruxen_command_args",
        "ruxen_command_env",
        "ruxen_command_current_dir",
        "ruxen_command_status",
        "ruxen_command_output",
        "ruxen_command_drop",
        "ruxen_exit_status_code",
        "ruxen_exit_status_success",
        "ruxen_exit_status_free",
        "ruxen_output_stdout",
        "ruxen_output_stderr",
        "ruxen_output_status",
        "ruxen_output_drop",
        "ruxen_file_open",
        "ruxen_file_create",
        "ruxen_file_append",
        "ruxen_file_open_options",
        "ruxen_file_read",
        "ruxen_file_read_to_string",
        "ruxen_file_read_all",
        "ruxen_file_write",
        "ruxen_file_write_all",
        "ruxen_file_write_str",
        "ruxen_file_flush",
        "ruxen_file_seek",
        "ruxen_file_metadata",
        "ruxen_file_close",
        "ruxen_file_drop",
        "ruxen_open_options_new",
        "ruxen_open_options_read",
        "ruxen_open_options_write",
        "ruxen_open_options_append",
        "ruxen_open_options_truncate",
        "ruxen_open_options_create",
        "ruxen_open_options_create_new",
        "ruxen_print_int",
        "ruxen_int_to_string",
        "ruxen_float_to_string",
        "ruxen_bool_to_string",
        "ruxen_char_to_string",
        "ruxen_int_to_f",
        "ruxen_float_to_i",
        "ruxen_string_concat",
        "ruxen_string_from",
        "ruxen_string_push_str",
        "ruxen_deref_ptr",
        "ruxen_store_ptr",
        "ruxen_string_len",
        "ruxen_string_is_empty",
        "ruxen_string_trim",
        "ruxen_string_to_lower",
        "ruxen_string_to_upper",
        "ruxen_string_chars",
        "ruxen_string_new",
        "ruxen_string_with_capacity",
        "ruxen_string_as_str",
        "ruxen_string_to_string",
        "ruxen_string_bytes",
        "ruxen_string_trim_start",
        "ruxen_string_trim_end",
        "ruxen_string_find",
        "ruxen_string_splitn",
        "ruxen_string_clear",
        "ruxen_string_truncate",
        "ruxen_string_insert",
        "ruxen_string_insert_str",
        "ruxen_string_remove",
        "ruxen_string_parse_int",
        "ruxen_string_parse_float",
        "ruxen_parse_error_message",
        "ruxen_string_split",
        "ruxen_string_push",
        "ruxen_string_into_bytes",
        "ruxen_string_eq",
        "ruxen_string_cmp",
        "ruxen_string_hash",
        "ruxen_thread_sleep_ns",
        "ruxen_thread_yield",
        "ruxen_duration_from_secs",
        "ruxen_duration_from_millis",
        "ruxen_duration_from_micros",
        "ruxen_duration_from_nanos",
        "ruxen_duration_as_secs",
        "ruxen_duration_as_millis",
        "ruxen_duration_as_micros",
        "ruxen_duration_as_nanos",
        "ruxen_duration_add",
        "ruxen_duration_sub",
        "ruxen_instant_now",
        "ruxen_instant_elapsed",
        "ruxen_instant_duration_since",
        "ruxen_instant_sub",
        "ruxen_thread_sleep_duration",
        "ruxen_str_split",
        "ruxen_str_parse_uint",
        "ruxen_alloc",
        "ruxen_dealloc",
        "ruxen_realloc",
        "ruxen_string_free",
        "ruxen_vec_free",
        "ruxen_hash_free",
        "ruxen_set_free",
        "ruxen_hash_drop_string_v",
        "ruxen_hash_drop_v_string",
        "ruxen_hash_drop_string_string",
        "ruxen_hash_drop_v_vec",
        "ruxen_set_drop_string",
        "ruxen_vec_drop_string",
        "ruxen_vec_drop_vec",
        "ruxen_vec_from_iter",
        "ruxen_string_from_iter",
        "ruxen_vec_dedup",
        "ruxen_vec_set",
        "ruxen_panic",
        "ruxen_vec_new",
        "ruxen_vec_push",
        "ruxen_vec_pop",
        "ruxen_vec_len",
        "ruxen_vec_get",
        "ruxen_vec_get_opt",
        "ruxen_vec_get_mut",
        "ruxen_vec_get_mut_opt",
        "ruxen_vec_is_empty",
        "ruxen_vec_each",
        "ruxen_hash_new",
        "ruxen_hash_from_iter",
        "ruxen_hash_insert",
        "ruxen_hash_get",
        "ruxen_hash_contains_key",
        "ruxen_hash_len",
        "ruxen_hash_is_empty",
        "ruxen_set_new",
        "ruxen_set_from_iter",
        "ruxen_set_insert",
        "ruxen_set_contains",
        "ruxen_set_len",
        "ruxen_set_is_empty",
        "ruxen_hash_with_capacity",
        "ruxen_hash_remove",
        "ruxen_hash_clear",
        "ruxen_hash_keys",
        "ruxen_hash_values",
        "ruxen_hash_entries",
        "ruxen_hash_eq",
        "ruxen_hash_index",
        "ruxen_set_with_capacity",
        "ruxen_set_remove",
        "ruxen_set_clear",
        "ruxen_set_iter",
        "ruxen_set_eq",
        "ruxen_set_union",
        "ruxen_set_intersection",
        "ruxen_set_difference",
        "ruxen_option_unwrap_or",
        "ruxen_result_unwrap_or_else",
        "ruxen_result_try_op",
        "ruxen_result_expect",
        "ruxen_result_unwrap",
        "ruxen_option_expect",
        "ruxen_option_unwrap",
        "ruxen_result_ok",
        "ruxen_result_err",
        "ruxen_option_is_some",
        "ruxen_option_is_none",
        "ruxen_result_is_ok",
        "ruxen_result_is_err",
        "ruxen_regex_new",
        "ruxen_regex_compile_const",
        "ruxen_regex_is_match",
        "ruxen_regex_match",
        "ruxen_regex_scan",
        "ruxen_regex_replace",
        "ruxen_regex_replace_all",
        "ruxen_regex_split",
        "ruxen_regex_drop",
        "ruxen_regex_error_message",
        "ruxen_regex_error_offset",
        "ruxen_regex_error_drop",
        "ruxen_match_matched",
        "ruxen_match_start",
        "ruxen_match_end",
        "ruxen_match_group",
        "ruxen_match_named",
        "ruxen_match_groups",
        "ruxen_match_named_groups",
        "ruxen_match_drop",
        "ruxen_noop",
    ]
}

/// Known `.rx`-vs-hand-rolled-table width discrepancies, each verified
/// against the C runtime source (ground truth) and confirmed ABI-benign
/// or pre-existing. The migration makes these symbols DERIVED, so their
/// width becomes the derived one — which already equals the binding
/// `Linkage::Import` signature codegen Pass-0 emits today (the table only
/// ever influenced `coerce_call_args`, not the import). Pinned here as
/// derived-reality so the parity assertion stays meaningful for the other
/// 171 shared symbols.
///
///   * `ruxen_fs_exists` / `ruxen_regex_is_match`: `.rx` declares `-> Bool`
///     (I8); C returns `int64_t` (I64). The call reads the low byte —
///     correct for a 0/1 bool on little-endian. Exercised by e2e cases
///     531/533 (fs.exists) and 907 (regex is_match); passes today.
///   * `ruxen_string_push`: `.rx` declares `(c: Char)` (I32); C takes
///     `int64_t codepoint`. FOLLOW-UP BUG: the `.rx` param should be
///     `Int`. Exercised by e2e case 315 (`s.push(?\u{21})`); passes today
///     because the Char arg widens compatibly. `.rx` fix is out of scope.
///   * `ruxen_vec_each`: `.rx` declares `def each -> Int` with no params;
///     C is `(RuxenVec*, void(*)(int64_t)) -> void`. FOLLOW-UP BUG: the
///     `.rx` decl omits the callback param and declares a bogus `-> Int`.
///     The callback is supplied via closure lowering, not a plain FFI arg,
///     so the missing param is invisible at the C-call boundary. Exercised
///     by e2e cases 120/108/307/88; passes today. `.rx` fix is out of scope.
fn known_rx_vs_table_discrepancies() -> &'static [&'static str] {
    &[
        "ruxen_fs_exists",
        "ruxen_regex_is_match",
        "ruxen_string_push",
        "ruxen_vec_each",
    ]
}

/// The crux check: for every symbol present in BOTH the derived map and
/// the hand-rolled table — EXCEPT the documented `.rx`-vs-C discrepancies
/// above — the cranelift `(params, ret)` widths MUST be identical. A
/// mismatch is a real C-ABI miscompile: the declared `.rx` width disagrees
/// with the width the table (and the C source) expects. Symbols only in
/// the table are the compiler-internal residual; symbols only in the
/// derived map are `.rx`-declared helpers the table simply never listed.
#[test]
fn derived_abi_widths_match_handrolled_table_for_shared_symbols() {
    let derived = derived_abi_map();

    let mut shared = 0usize;
    let mut residual: Vec<String> = Vec::new();
    let mut derived_only_count = 0usize;
    let mut mismatches: Vec<String> = Vec::new();

    for &sym in table_symbols() {
        let table = match baseline_runtime_signature(sym) {
            Some(s) => s,
            None => {
                panic!("table_symbols lists {sym:?} but baseline_runtime_signature returned None")
            }
        };
        match derived.get(sym) {
            None => residual.push(sym.to_string()),
            Some(d) => {
                shared += 1;
                if *d != table && !known_rx_vs_table_discrepancies().contains(&sym) {
                    mismatches.push(format!("  {sym}: derived {d:?} != table {table:?}"));
                }
            }
        }
    }
    for k in derived.keys() {
        if baseline_runtime_signature(k).is_none() {
            derived_only_count += 1;
        }
    }

    residual.sort();
    eprintln!(
        "[runtime_abi_derivation] derived={} table={} shared={} \
         residual(table-only)={} derived-only={}",
        derived.len(),
        table_symbols().len(),
        shared,
        residual.len(),
        derived_only_count,
    );
    eprintln!("[runtime_abi_derivation] RESIDUAL (table symbols NOT .rx-derived):");
    for r in &residual {
        eprintln!("    {r}");
    }

    assert!(
        mismatches.is_empty(),
        "derived-vs-table ABI width mismatches ({}):\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
