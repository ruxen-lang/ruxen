//! Runtime function declarations and name mapping.
//!
//! Documents the C runtime functions available at link time and provides
//! the shared `runtime_name()` mapping used by both Cranelift and LLVM
//! backends.

/// Path to the C runtime source file, relative to the rivenc crate root.
pub const RUNTIME_C_SOURCE: &str = "runtime/runtime.c";

/// All runtime functions that the C runtime provides.
pub const RUNTIME_FUNCTIONS: &[&str] = &[
    "riven_puts",
    "riven_print",
    "riven_eputs",
    "riven_read_line",
    "riven_stdin",
    "riven_stdout",
    "riven_stderr",
    "riven_stdin_read_line",
    "riven_stdin_read_to_string",
    "riven_stdin_lines",
    "riven_stdout_write_str",
    "riven_stdout_flush",
    "riven_stderr_write_str",
    "riven_stderr_flush",
    // Phase 2 stdlib (#06.1): Stdout / Stderr convenience methods.
    "riven_stdout_print",
    "riven_stdout_println",
    "riven_stderr_eprint",
    "riven_stderr_eprintln",
    // Phase 2 stdlib (#06.A3): std::fmt::Formatter buffer surface.
    "riven_fmt_formatter_new",
    "riven_fmt_formatter_free",
    "riven_fmt_formatter_write_str",
    "riven_fmt_formatter_write_char",
    "riven_fmt_formatter_buffer",
    "riven_fmt_formatter_len",
    // Phase 2 stdlib (#06.D4): spec-aware constructor + precision
    // accessor + per-type precision helpers used by the synth `_fmt`
    // bodies.
    "riven_fmt_formatter_new_with_spec",
    "riven_fmt_formatter_precision",
    "riven_float_to_string_prec",
    "riven_string_truncate_chars",
    "riven_env_init",
    "riven_env_args_count",
    "riven_env_args_at",
    "riven_env_args",
    "riven_env_var",
    // Phase 2 stdlib (#06): env / fs additions.
    "riven_env_vars",
    "riven_env_current_dir",
    "riven_fs_is_file",
    "riven_fs_is_dir",
    "riven_fs_read_dir",
    // Phase 2 stdlib (#06): fs::metadata + Metadata accessor surface.
    "riven_fs_metadata",
    "riven_metadata_len",
    "riven_metadata_modified",
    "riven_metadata_is_file",
    "riven_metadata_is_dir",
    "riven_metadata_is_symlink",
    "riven_metadata_free",
    // Phase 2 stdlib (#06): std::process::Command builder + Output /
    // ExitStatus accessor surface. Wire layouts documented in
    // `runtime.c` at `riven_command_new`.
    "riven_command_new",
    "riven_command_arg",
    "riven_command_args",
    "riven_command_env",
    "riven_command_current_dir",
    "riven_command_status",
    "riven_command_output",
    "riven_command_drop",
    "riven_exit_status_code",
    "riven_exit_status_success",
    "riven_exit_status_free",
    "riven_output_stdout",
    "riven_output_stderr",
    "riven_output_status",
    "riven_output_drop",
    // Phase 2 stdlib (#06.5 T2): std::io::File + OpenOptions builder.
    // Wire layout documented in `runtime.c` at `RivenFile` /
    // `RivenOpenOptions`. SeekFrom is a user-level enum (no runtime
    // ctor) — its 16-byte tagged-value layout is the standard enum
    // shape; `riven_file_seek` reads tag and offset directly from the
    // SeekFrom value pointer.
    "riven_file_open",
    "riven_file_create",
    "riven_file_append",
    "riven_file_open_options",
    "riven_file_read",
    "riven_file_read_to_string",
    "riven_file_read_all",
    "riven_file_write",
    "riven_file_write_all",
    "riven_file_write_str",
    "riven_file_flush",
    "riven_file_seek",
    "riven_file_metadata",
    "riven_file_close",
    "riven_file_drop",
    "riven_open_options_new",
    "riven_open_options_read",
    "riven_open_options_write",
    "riven_open_options_append",
    "riven_open_options_truncate",
    "riven_open_options_create",
    "riven_open_options_create_new",
    "riven_process_exit",
    // std::process::run (Phase 3): fork+execvp a child, inherit stdio,
    // return exit code (or 128+signal on signal termination, 127 on
    // fork/exec failure).
    "riven_process_run",
    "riven_fs_read_to_string",
    "riven_fs_write",
    "riven_fs_exists",
    "riven_fs_remove_file",
    "riven_fs_create_dir",
    "riven_fs_create_dir_all",
    "riven_fs_rename",
    // Phase 2 stdlib (#06.5 T3): fs completeness.
    "riven_fs_copy",
    "riven_fs_remove_dir_all",
    "riven_fs_canonicalize",
    "riven_fs_write_atomic",
    "riven_fs_read_link",
    "riven_fs_symlink",
    "riven_print_int",
    "riven_print_float",
    "riven_int_to_string",
    "riven_float_to_string",
    "riven_bool_to_string",
    "riven_char_to_string",
    "riven_string_concat",
    "riven_string_from",
    "riven_string_to_upper",
    "riven_string_chars",
    "riven_string_eq",
    "riven_string_cmp",
    "riven_string_hash",
    "riven_thread_sleep_ns",
    "riven_thread_yield",
    // std::time (Phase 3): monotonic + realtime clocks, nanoseconds.
    "riven_time_now_ns",
    "riven_time_unix_ns",
    // Phase 2 stdlib (#06.5 T4): Duration / Instant scalar-wrapper
    // classes + free-function `std.thread.sleep(d)`. Wire layouts
    // (both 8 bytes) documented in runtime.c at `RivenDuration` /
    // `RivenInstant`. The `riven_thread_sleep_duration(d)` wrapper
    // delegates to the existing `riven_thread_sleep_ns(int64)` after
    // unpacking — keeps the Thread.sleep(int) entry point untouched.
    "riven_duration_from_secs",
    "riven_duration_from_millis",
    "riven_duration_from_micros",
    "riven_duration_from_nanos",
    "riven_duration_as_secs",
    "riven_duration_as_millis",
    "riven_duration_as_micros",
    "riven_duration_as_nanos",
    "riven_duration_add",
    "riven_duration_sub",
    "riven_instant_now",
    "riven_instant_elapsed",
    "riven_instant_duration_since",
    "riven_instant_sub",
    "riven_thread_sleep_duration",
    // std::path (Phase 3): Unix-style path manipulation. Empty-string
    // sentinel for parent/file_name/extension when no value applies.
    "riven_path_join",
    "riven_path_parent",
    "riven_path_file_name",
    "riven_path_extension",
    "riven_path_is_absolute",
    // std::net (Phase 3): minimum-viable TCP. fds surfaced as Int.
    "riven_tcp_connect",
    "riven_tcp_listen",
    "riven_tcp_accept",
    "riven_tcp_read",
    "riven_tcp_write",
    "riven_tcp_close",
    // std::signal (graceful-shutdown surface).
    "riven_signal_install_sigint",
    "riven_signal_received_sigint",
    "riven_string_contains",
    "riven_string_starts_with",
    "riven_string_ends_with",
    "riven_string_repeat",
    "riven_string_lines",
    "riven_string_replace",
    // String stdlib methods (#02 Phase 2 stdlib).
    "riven_string_new",
    "riven_string_with_capacity",
    "riven_string_as_str",
    "riven_string_to_string",
    "riven_string_bytes",
    "riven_string_trim_start",
    "riven_string_trim_end",
    "riven_string_find",
    "riven_string_splitn",
    "riven_string_clear",
    "riven_string_truncate",
    "riven_string_insert",
    "riven_string_insert_str",
    "riven_string_remove",
    "riven_string_parse_int",
    "riven_string_parse_float",
    "riven_parse_error_message",
    // String stdlib batch 2 (#02): split / push / into_bytes.
    "riven_string_split",
    "riven_string_push",
    "riven_string_into_bytes",
    "riven_string_from_iter",
    "riven_vec_pop",
    "riven_vec_sum",
    "riven_vec_count",
    "riven_vec_reverse",
    "riven_vec_first",
    "riven_vec_last",
    "riven_vec_clone",
    // Phase 2 stdlib (#05 batch 2): lazy iterator combinators
    // `take(n)` / `skip(n)` eager-materialise into a fresh `RivenVec*`.
    "riven_vec_take",
    "riven_vec_skip",
    // Phase 2 stdlib (#05 batch 3): `chain(other)` concatenates two
    // iterators into a fresh Vec; `zip(other)` materialises pair
    // tuples into a fresh Vec[(T,U)]. Both keep the v1 "iter == Vec"
    // invariant so downstream terminators see a `RivenVec*`.
    "riven_vec_chain",
    "riven_vec_zip",
    "riven_vec_contains_int",
    "riven_vec_sort",
    "riven_vec_join",
    // Vec[T] surface — Phase 2 stdlib batch 1 (#03).
    "riven_vec_with_capacity",
    "riven_vec_capacity",
    "riven_vec_clear",
    "riven_vec_truncate",
    "riven_vec_swap",
    "riven_vec_insert",
    "riven_vec_remove",
    "riven_vec_extend",
    "riven_vec_get_or_panic",
    "riven_vec_eq",
    "riven_vec_drop_string",
    "riven_vec_drop_vec",
    // Phase 2 stdlib batch 2 (#03): from_iter, dedup, set.
    "riven_vec_from_iter",
    "riven_vec_dedup",
    "riven_vec_set",
    "riven_hash_new",
    "riven_hash_from_iter",
    "riven_hash_insert",
    "riven_hash_get",
    "riven_hash_contains_key",
    "riven_hash_len",
    "riven_hash_is_empty",
    "riven_set_new",
    "riven_set_from_iter",
    "riven_set_insert",
    "riven_set_contains",
    "riven_set_len",
    "riven_set_is_empty",
    // Phase 2 stdlib (#04): HashMap[K,V] full surface.
    "riven_hash_with_capacity",
    "riven_hash_remove",
    "riven_hash_clear",
    "riven_hash_keys",
    "riven_hash_values",
    "riven_hash_iter",
    "riven_hash_eq",
    "riven_hash_index",
    // Phase 2 stdlib (#04): HashSet[T] full surface.
    "riven_set_with_capacity",
    "riven_set_remove",
    "riven_set_clear",
    "riven_set_iter",
    "riven_set_eq",
    "riven_set_union",
    "riven_set_intersection",
    "riven_set_difference",
    "riven_alloc",
    "riven_dealloc",
    "riven_realloc",
    "riven_string_free",
    "riven_vec_free",
    "riven_hash_free",
    // Phase 2 stdlib (#04 batch 2): set spine + per-element drop
    // selectors for HashMap[String, V] / [K, String] / [K, Vec[T]] /
    // HashSet[String]. Driven by `mir/lower.rs::insert_drops` based on
    // the static K/V/T types.
    "riven_set_free",
    "riven_hash_drop_string_v",
    "riven_hash_drop_v_string",
    "riven_hash_drop_string_string",
    "riven_hash_drop_v_vec",
    "riven_set_drop_string",
    "riven_panic",
    "riven_option_expect",
    "riven_option_unwrap",
    "riven_option_is_some",
    "riven_option_is_none",
    "riven_result_expect",
    "riven_result_unwrap",
    "riven_result_is_ok",
    "riven_result_is_err",
    "riven_result_ok",
    "riven_result_err",
];

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
         or add a real symbol to riven_runtime"
    )
}

/// Map Riven built-in function names to their runtime C names.
///
/// Handles both top-level functions (puts, eputs) and mangled method
/// names for built-in types (String_from, Vec_push, etc.).
///
/// Returns `Err(diagnostic)` when a generic method call cannot be resolved
/// to a real runtime symbol. This replaces the historical silent fallback
/// to `riven_noop_passthrough` which masked dozens of unimplemented methods
/// (`.fold`, `.sum`, `.collect`, `.map_err`, `.contains`, ...) behind a
/// no-op that happened to produce the expected output for some fixtures.
pub fn runtime_name(name: &str) -> Result<&str, String> {
    super::runtime_table::runtime_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_inferred_type_method_is_rejected() {
        // Historically `?T_xxx_totally_fake` would silently map to
        // `riven_noop_passthrough`. P0.5: it must error instead.
        let err = runtime_name("?T0_totally_fake_method").unwrap_err();
        assert!(
            err.contains("totally_fake_method"),
            "diagnostic should name the method: {err}"
        );
        assert!(
            err.contains("no runtime symbol"),
            "diagnostic should say no runtime symbol: {err}"
        );
    }

    #[test]
    fn unimplemented_vec_combinators_are_rejected() {
        // `sum` and `count` now resolve to `riven_vec_sum`/`riven_vec_count`
        // — see `implemented_vec_combinators_resolve`. The list here is
        // limited to combinators with no MIR inliner and no runtime symbol.
        for m in [
            "Vec[Int]_fold",
            "Vec[Int]_collect",
            "Vec[Int]_map",
            "Vec[Int]_filter",
        ] {
            assert!(
                runtime_name(m).is_err(),
                "expected `{m}` to be rejected (was {:?})",
                runtime_name(m)
            );
        }
    }

    #[test]
    fn implemented_vec_combinators_resolve() {
        assert_eq!(runtime_name("Vec[Int]_sum").unwrap(), "riven_vec_sum");
        assert_eq!(runtime_name("Vec[Int]_count").unwrap(), "riven_vec_count");
        assert_eq!(
            runtime_name("Vec[Int]_reverse").unwrap(),
            "riven_vec_reverse"
        );
        assert_eq!(runtime_name("Vec[Int]_first").unwrap(), "riven_vec_first");
        assert_eq!(runtime_name("Vec[Int]_last").unwrap(), "riven_vec_last");
    }

    #[test]
    fn implemented_string_predicates_resolve() {
        assert_eq!(
            runtime_name("String_contains").unwrap(),
            "riven_string_contains"
        );
        assert_eq!(
            runtime_name("String_starts_with").unwrap(),
            "riven_string_starts_with"
        );
        assert_eq!(
            runtime_name("String_ends_with").unwrap(),
            "riven_string_ends_with"
        );
        assert_eq!(
            runtime_name("String_repeat").unwrap(),
            "riven_string_repeat"
        );
    }

    #[test]
    fn iterator_passthrough_collectors_resolve() {
        // `iter`, `into_iter`, and `to_vec` are identity passthroughs in
        // the v1 runtime — they all map to `riven_iter_to_vec`.
        assert_eq!(runtime_name("Vec[Int]_iter").unwrap(), "riven_iter_to_vec",);
        assert_eq!(
            runtime_name("Vec[Int]_into_iter").unwrap(),
            "riven_iter_to_vec",
        );
        assert_eq!(
            runtime_name("Vec[Int]_to_vec").unwrap(),
            "riven_iter_to_vec",
        );
        assert_eq!(
            runtime_name("SplitIter_to_vec").unwrap(),
            "riven_iter_to_vec",
        );
    }

    #[test]
    fn unimplemented_result_combinators_are_rejected() {
        // `map_err`, `map`, and `unwrap_or_else` are closure-inlined at
        // MIR level — they don't reach `runtime_name`. The combinators
        // listed here have no MIR inliner and no runtime symbol yet.
        assert!(runtime_name("Result[Int,Err]_and_then").is_err());
        assert!(runtime_name("Result[Int,Err]_or").is_err());
        assert!(runtime_name("Result[Int,Err]_or_else").is_err());
        assert!(runtime_name("Result[Int,Err]_ok_or").is_err());
    }

    #[test]
    fn implemented_option_result_combinators_resolve() {
        assert_eq!(
            runtime_name("Result[Int,Err]_unwrap_or").unwrap(),
            "riven_result_unwrap_or",
        );
        assert_eq!(
            runtime_name("Option[Int]_ok_or").unwrap(),
            "riven_option_ok_or",
        );
    }

    #[test]
    fn known_runtime_symbols_resolve() {
        assert_eq!(runtime_name("puts").unwrap(), "riven_puts");
        assert_eq!(runtime_name("Vec[Int]_push").unwrap(), "riven_vec_push");
        assert_eq!(runtime_name("Vec[Int]_len").unwrap(), "riven_vec_len");
        assert_eq!(runtime_name("Hash[Int,Int]_get").unwrap(), "riven_hash_get");
        assert_eq!(
            runtime_name("HashMap[Int,Int]_get").unwrap(),
            "riven_hash_get"
        );
    }

    #[test]
    fn user_defined_methods_forward() {
        assert_eq!(
            runtime_name("MyClass_my_method").unwrap(),
            "MyClass_my_method"
        );
    }

    #[test]
    fn yield_super_and_str_identity_still_resolve() {
        assert_eq!(runtime_name("super").unwrap(), "riven_noop");
        assert_eq!(runtime_name("yield").unwrap(), "riven_noop_passthrough");
        assert_eq!(
            runtime_name("&str_as_str").unwrap(),
            "riven_noop_passthrough"
        );
    }

    #[test]
    fn stdio_top_level_and_methods_resolve() {
        // Top-level fns added in `13d34a6 fixing v1`.
        assert_eq!(runtime_name("read_line").unwrap(), "riven_read_line");
        assert_eq!(runtime_name("stdin").unwrap(), "riven_stdin");
        assert_eq!(runtime_name("stdout").unwrap(), "riven_stdout");
        assert_eq!(runtime_name("stderr").unwrap(), "riven_stderr");
        // Stdin/Stdout/Stderr methods.
        assert_eq!(
            runtime_name("Stdin_read_line").unwrap(),
            "riven_stdin_read_line"
        );
        assert_eq!(
            runtime_name("Stdout_write_str").unwrap(),
            "riven_stdout_write_str"
        );
        assert_eq!(runtime_name("Stderr_flush").unwrap(), "riven_stderr_flush");
    }

    #[test]
    fn thread_runtime_methods_resolve() {
        assert_eq!(
            runtime_name("Thread_sleep").unwrap(),
            "riven_thread_sleep_ns"
        );
        assert_eq!(
            runtime_name("Thread_yield_now").unwrap(),
            "riven_thread_yield"
        );
    }
}
