//! Method-name → runtime-symbol-name mapping table.
//!
//! Phase E of #06.75 carved this out of `codegen/runtime.rs` (where it
//! lived as the 515-LOC `runtime_name` function).  This `match name { … }`
//! is the source of truth for which user-facing method calls
//! (`File_open`, `Command_status`, `HashMap_insert`, …) bind to which
//! `riven_*` C symbol.
//!
//! Future follow-up (tracked separately): split this single
//! `runtime_name` function into per-namespace files (`io.rs`, `fs.rs`,
//! `net.rs`, `time.rs`, `process.rs`, `collections.rs`).  The arm
//! groupings below already mark the cut lines.

use super::runtime::{extract_method_name, unresolved_method_error};

pub fn runtime_name(name: &str) -> Result<&str, String> {
    // Direct, well-known symbols. These are the only call names that can
    // safely resolve to `riven_noop*` helpers — they are language-level
    // constructs (closure invocation, super-call, &str/&str identity)
    // rather than method calls that pretend to do work.
    match name {
        // Top-level I/O / printing.
        "puts" => return Ok("riven_puts"),
        "eputs" => return Ok("riven_eputs"),
        "print" => return Ok("riven_print"),
        "println" => return Ok("riven_puts"),
        "eprintln" => return Ok("riven_eputs"),
        "read_line" => return Ok("riven_read_line"),
        "stdin" => return Ok("riven_stdin"),
        "stdout" => return Ok("riven_stdout"),
        "stderr" => return Ok("riven_stderr"),
        // Top-level env / fs.
        "args" => return Ok("riven_env_args"),
        // ruby-naming.spec.md §3.14: `env.var` renamed to `env.get`
        // because `var` is a reserved keyword. The internal C symbol
        // keeps its legacy `riven_env_var` name.
        "get" => return Ok("riven_env_var"),
        // Phase 2 stdlib (#06): env / fs additions.
        "vars" => return Ok("riven_env_vars"),
        "current_dir" => return Ok("riven_env_current_dir"),
        "is_file" => return Ok("riven_fs_is_file"),
        "is_dir" => return Ok("riven_fs_is_dir"),
        "read_dir" => return Ok("riven_fs_read_dir"),
        "metadata" => return Ok("riven_fs_metadata"),
        "read_to_string" => return Ok("riven_fs_read_to_string"),
        "write" => return Ok("riven_fs_write"),
        "exists" => return Ok("riven_fs_exists"),
        "remove_file" => return Ok("riven_fs_remove_file"),
        "create_dir" => return Ok("riven_fs_create_dir"),
        "create_dir_all" => return Ok("riven_fs_create_dir_all"),
        "rename" => return Ok("riven_fs_rename"),
        // Phase 2 stdlib (#06.5 T3): fs completeness.
        "copy" => return Ok("riven_fs_copy"),
        "remove_dir_all" => return Ok("riven_fs_remove_dir_all"),
        "canonicalize" => return Ok("riven_fs_canonicalize"),
        "write_atomic" => return Ok("riven_fs_write_atomic"),
        "read_link" => return Ok("riven_fs_read_link"),
        "symlink" => return Ok("riven_fs_symlink"),
        "exit" => return Ok("riven_process_exit"),
        // `process_run` removed in #06.5 T5.5 — superseded by
        // `Command_{status,output}`. The C symbol `riven_process_run`
        // is still linked (it is the implementation behind
        // `riven_command_status`); the resolver-visible Riven name
        // is gone. See docs/specs/stdlib/process.spec.md.
        // String methods.
        "String_from" => return Ok("riven_string_from"),
        "String_push_str" => return Ok("riven_string_push_str"),
        "String_len" => return Ok("riven_string_len"),
        "String_is_empty" => return Ok("riven_string_is_empty"),
        "String_trim" => return Ok("riven_string_trim"),
        "String_to_lower" => return Ok("riven_string_to_lower"),
        "String_to_upper" => return Ok("riven_string_to_upper"),
        "String_chars" => return Ok("riven_string_chars"),
        "String_clone" => return Ok("riven_string_from"),
        "String_contains" => return Ok("riven_string_contains"),
        "String_starts_with" => return Ok("riven_string_starts_with"),
        "String_ends_with" => return Ok("riven_string_ends_with"),
        "String_repeat" => return Ok("riven_string_repeat"),
        "String_lines" => return Ok("riven_string_lines"),
        "String_replace" => return Ok("riven_string_replace"),
        // Phase 2 stdlib additions (#02).
        "String_new" => return Ok("riven_string_new"),
        "String_with_capacity" => return Ok("riven_string_with_capacity"),
        "String_as_str" => return Ok("riven_string_as_str"),
        "String_to_string" => return Ok("riven_string_to_string"),
        "String_bytes" => return Ok("riven_string_bytes"),
        "String_trim_start" => return Ok("riven_string_trim_start"),
        "String_trim_end" => return Ok("riven_string_trim_end"),
        "String_find" => return Ok("riven_string_find"),
        "String_splitn" => return Ok("riven_string_splitn"),
        "String_clear" => return Ok("riven_string_clear"),
        "String_truncate" => return Ok("riven_string_truncate"),
        "String_insert" => return Ok("riven_string_insert"),
        "String_insert_str" => return Ok("riven_string_insert_str"),
        "String_remove" => return Ok("riven_string_remove"),
        "String_parse_int" => return Ok("riven_string_parse_int"),
        "String_parse_float" => return Ok("riven_string_parse_float"),
        "ParseIntError_message" => return Ok("riven_parse_error_message"),
        "ParseFloatError_message" => return Ok("riven_parse_error_message"),
        // Phase 2 stdlib batch 2 (#02).
        "String_split" => return Ok("riven_string_split"),
        "String_push" => return Ok("riven_string_push"),
        "String_into_bytes" => return Ok("riven_string_into_bytes"),
        "String_from_iter" => return Ok("riven_string_from_iter"),
        // &str methods.
        "&str_split" => return Ok("riven_str_split"),
        "&str_parse_uint" => return Ok("riven_str_parse_uint"),
        "&str_len" => return Ok("riven_string_len"),
        "&str_is_empty" => return Ok("riven_string_is_empty"),
        "&str_trim" => return Ok("riven_string_trim"),
        "&str_to_lower" => return Ok("riven_string_to_lower"),
        "&str_to_upper" => return Ok("riven_string_to_upper"),
        "&str_chars" => return Ok("riven_string_chars"),
        "&str_contains" => return Ok("riven_string_contains"),
        "&str_starts_with" => return Ok("riven_string_starts_with"),
        "&str_ends_with" => return Ok("riven_string_ends_with"),
        "&str_lines" => return Ok("riven_string_lines"),
        "&str_replace" => return Ok("riven_string_replace"),
        "&str_bytes" => return Ok("riven_string_bytes"),
        "&str_trim_start" => return Ok("riven_string_trim_start"),
        "&str_trim_end" => return Ok("riven_string_trim_end"),
        "&str_find" => return Ok("riven_string_find"),
        "&str_splitn" => return Ok("riven_string_splitn"),
        "&str_parse_int" => return Ok("riven_string_parse_int"),
        "&str_parse_float" => return Ok("riven_string_parse_float"),
        "&str_to_string" => return Ok("riven_string_to_string"),
        // `&str → &str` is a true semantic identity, not a stub.
        "&str_as_str" => return Ok("riven_noop_passthrough"),
        // I/O type methods (Stdin/Stdout/Stderr/IoError).
        // Phase 2 #06.5: `IoError` is a tagged enum (see runtime.c
        // for the wire format). The previous noop-passthrough worked
        // only while the payload literally was the message string;
        // with proper variants we need a real dispatcher.
        "IoError_message" => return Ok("riven_io_error_get_message"),
        // Phase 2 #06.5 T1: `.kind() -> IoErrorKind` returns the
        // discriminant as a sibling 20-unit-variant enum (same wire
        // format — 16 bytes, tag at offset 0). See
        // `riven_io_error_kind` in runtime.c.
        "IoError_kind" => return Ok("riven_io_error_kind"),
        "Stdin_read_line" => return Ok("riven_stdin_read_line"),
        "Stdin_read_to_string" => return Ok("riven_stdin_read_to_string"),
        "Stdin_lines" => return Ok("riven_stdin_lines"),
        "Stdout_write_str" => return Ok("riven_stdout_write_str"),
        "Stdout_flush" => return Ok("riven_stdout_flush"),
        "Stderr_write_str" => return Ok("riven_stderr_write_str"),
        "Stderr_flush" => return Ok("riven_stderr_flush"),
        // Phase 2 stdlib (#06.1): no-Result print convenience methods.
        "Stdout_print" => return Ok("riven_stdout_print"),
        "Stdout_println" => return Ok("riven_stdout_println"),
        "Stderr_eprint" => return Ok("riven_stderr_eprint"),
        "Stderr_eprintln" => return Ok("riven_stderr_eprintln"),
        // Phase 2 stdlib (#06): std::fs::Metadata accessor methods.
        "Metadata_len" => return Ok("riven_metadata_len"),
        "Metadata_modified" => return Ok("riven_metadata_modified"),
        "Metadata_is_file" => return Ok("riven_metadata_is_file"),
        "Metadata_is_dir" => return Ok("riven_metadata_is_dir"),
        "Metadata_is_symlink" => return Ok("riven_metadata_is_symlink"),
        "Metadata_free" => return Ok("riven_metadata_free"),
        // Phase 2 stdlib (#06): std::process::Command builder + Output /
        // ExitStatus accessors. `Command_new` is dispatched via the
        // collection-ctor fast path in mir/lower.rs (alongside
        // Vec_new / Hash_new / Formatter_new); the rest go through the
        // standard `{Type}_{method}` regular method-call path.
        "Command_new" => return Ok("riven_command_new"),
        "Command_arg" => return Ok("riven_command_arg"),
        "Command_args" => return Ok("riven_command_args"),
        "Command_env" => return Ok("riven_command_env"),
        "Command_current_dir" => return Ok("riven_command_current_dir"),
        "Command_status" => return Ok("riven_command_status"),
        "Command_output" => return Ok("riven_command_output"),
        "Command_drop" => return Ok("riven_command_drop"),
        "ExitStatus_code" => return Ok("riven_exit_status_code"),
        "ExitStatus_success" => return Ok("riven_exit_status_success"),
        "ExitStatus_free" => return Ok("riven_exit_status_free"),
        "Output_stdout" => return Ok("riven_output_stdout"),
        "Output_stderr" => return Ok("riven_output_stderr"),
        "Output_status" => return Ok("riven_output_status"),
        "Output_drop" => return Ok("riven_output_drop"),
        // Phase 2 stdlib (#06.5 T2): File / OpenOptions surface.
        // `File_open/create/append/open_options` are static-style
        // constructors that go through the standard `{Type}_{method}`
        // mangling path. `File_drop` is registered in the
        // user_drop_classes set (mir/lower/collect.rs) so the MIR
        // emits it before the spine dealloc at scope exit.
        "File_open" => return Ok("riven_file_open"),
        "File_create" => return Ok("riven_file_create"),
        "File_append" => return Ok("riven_file_append"),
        "File_open_options" => return Ok("riven_file_open_options"),
        "File_read" => return Ok("riven_file_read"),
        "File_read_to_string" => return Ok("riven_file_read_to_string"),
        "File_read_all" => return Ok("riven_file_read_all"),
        "File_write" => return Ok("riven_file_write"),
        "File_write_all" => return Ok("riven_file_write_all"),
        "File_write_str" => return Ok("riven_file_write_str"),
        "File_flush" => return Ok("riven_file_flush"),
        "File_seek" => return Ok("riven_file_seek"),
        "File_metadata" => return Ok("riven_file_metadata"),
        "File_close" => return Ok("riven_file_close"),
        "File_drop" => return Ok("riven_file_drop"),
        "OpenOptions_new" => return Ok("riven_open_options_new"),
        "OpenOptions_read" => return Ok("riven_open_options_read"),
        "OpenOptions_write" => return Ok("riven_open_options_write"),
        "OpenOptions_append" => return Ok("riven_open_options_append"),
        "OpenOptions_truncate" => return Ok("riven_open_options_truncate"),
        "OpenOptions_create" => return Ok("riven_open_options_create"),
        "OpenOptions_create_new" => return Ok("riven_open_options_create_new"),
        // Phase 2 stdlib (#06.A3): std::fmt::Formatter methods.
        "Formatter_new" => return Ok("riven_fmt_formatter_new"),
        "Formatter_free" => return Ok("riven_fmt_formatter_free"),
        "Formatter_write_str" => return Ok("riven_fmt_formatter_write_str"),
        "Formatter_write_char" => return Ok("riven_fmt_formatter_write_char"),
        "Formatter_buffer" => return Ok("riven_fmt_formatter_buffer"),
        "Formatter_len" => return Ok("riven_fmt_formatter_len"),
        // Phase 2 stdlib (#06.D4): spec-aware constructor + precision
        // accessor + per-type precision helpers.
        "Formatter_new_with_spec" => return Ok("riven_fmt_formatter_new_with_spec"),
        "Formatter_precision" => return Ok("riven_fmt_formatter_precision"),
        "Float_to_string_prec" => return Ok("riven_float_to_string_prec"),
        "String_truncate_chars" => return Ok("riven_string_truncate_chars"),
        "Thread_sleep" => return Ok("riven_thread_sleep_ns"),
        "Thread_yield_now" => return Ok("riven_thread_yield"),
        // std::time top-level functions (resolved before module-prefixing).
        // `now_ns` removed in #06.5 T5.5 — superseded by `Instant.now`.
        // The C symbol `riven_time_now_ns` is still linked (it is the
        // implementation behind `riven_instant_now`); the resolver-
        // visible Riven name is gone. See docs/specs/stdlib/time.spec.md.
        "unix_ns" => return Ok("riven_time_unix_ns"),
        // Phase 2 stdlib (#06.5 T4): Duration / Instant scalar-wrapper
        // classes. `Duration_from_*` and `Instant_now` are static-style
        // constructors that go through the "collection-ctor fast path"
        // in mir/lower/expr/method_call.rs (alongside File_open,
        // Command_new), so they receive no synthetic `self`. The rest
        // are regular instance methods on the {Type}_{method}
        // mangling path. `sleep` is the top-level free fn in the new
        // `std.thread` module.
        "Duration_from_secs" => return Ok("riven_duration_from_secs"),
        "Duration_from_millis" => return Ok("riven_duration_from_millis"),
        "Duration_from_micros" => return Ok("riven_duration_from_micros"),
        "Duration_from_nanos" => return Ok("riven_duration_from_nanos"),
        "Duration_as_secs" => return Ok("riven_duration_as_secs"),
        "Duration_as_millis" => return Ok("riven_duration_as_millis"),
        "Duration_as_micros" => return Ok("riven_duration_as_micros"),
        "Duration_as_nanos" => return Ok("riven_duration_as_nanos"),
        "Duration_add" => return Ok("riven_duration_add"),
        "Duration_sub" => return Ok("riven_duration_sub"),
        "Instant_now" => return Ok("riven_instant_now"),
        "Instant_elapsed" => return Ok("riven_instant_elapsed"),
        "Instant_duration_since" => return Ok("riven_instant_duration_since"),
        "Instant_sub" => return Ok("riven_instant_sub"),
        "sleep" => return Ok("riven_thread_sleep_duration"),
        // std::path top-level functions.
        "path_join" => return Ok("riven_path_join"),
        "path_parent" => return Ok("riven_path_parent"),
        "path_file_name" => return Ok("riven_path_file_name"),
        "path_extension" => return Ok("riven_path_extension"),
        "path_is_absolute" => return Ok("riven_path_is_absolute"),
        // Phase 2 stdlib (#06.5 T5): std::net::TcpListener /
        // TcpStream class surface. Static-style constructors
        // (`TcpListener_bind`, `TcpStream_connect`) are routed
        // through the collection-ctor fast path in
        // mir/lower/expr/method_call.rs (alongside File_open / ...).
        // Instance methods go through the standard `{Type}_{method}`
        // mangling. `*_drop` are emitted by the user_drop_classes
        // scope-exit pass — see mir/lower/collect.rs.
        "TcpListener_bind" => return Ok("riven_tcp_listener_bind"),
        "TcpListener_accept" => return Ok("riven_tcp_listener_accept"),
        "TcpListener_local_addr" => return Ok("riven_tcp_listener_local_addr"),
        "TcpListener_set_nonblocking" => return Ok("riven_tcp_listener_set_nonblocking"),
        "TcpListener_close" => return Ok("riven_tcp_listener_close"),
        "TcpListener_drop" => return Ok("riven_tcp_listener_drop"),
        "TcpStream_connect" => return Ok("riven_tcp_stream_connect"),
        "TcpStream_read" => return Ok("riven_tcp_stream_read"),
        "TcpStream_write" => return Ok("riven_tcp_stream_write"),
        "TcpStream_peer_addr" => return Ok("riven_tcp_stream_peer_addr"),
        "TcpStream_shutdown" => return Ok("riven_tcp_stream_shutdown"),
        "TcpStream_close" => return Ok("riven_tcp_stream_close"),
        "TcpStream_drop" => return Ok("riven_tcp_stream_drop"),
        "TcpStream_set_read_timeout" => return Ok("riven_tcp_stream_set_read_timeout"),
        "TcpStream_set_write_timeout" => return Ok("riven_tcp_stream_set_write_timeout"),
        // std::signal — graceful-shutdown surface.
        "signal_install_sigint" => return Ok("riven_signal_install_sigint"),
        "signal_received_sigint" => return Ok("riven_signal_received_sigint"),
        // Phase 2 stdlib (#06.5 T8): std::rand — kernel CSPRNG-backed
        // free fns. Backend selection (Linux getrandom / macOS
        // SecRandomCopyBytes / fallback /dev/urandom) happens in
        // library/runtime/io/rand.c under compile-time `#if`.
        "random_bytes" => return Ok("riven_rand_random_bytes"),
        "random_u64" => return Ok("riven_rand_random_u64"),
        "random_fill" => return Ok("riven_rand_random_fill"),
        // Compiler-injected pseudo-calls. These are not method calls; the
        // codegen treats them specially.
        //   - `super` is a parent-init dispatch in a constructor.
        //   - `yield` is the per-call placeholder for invoking the block
        //     argument; backends inline the closure call elsewhere.
        "super" => return Ok("riven_noop"),
        "yield" => return Ok("riven_noop_passthrough"),
        _ => {}
    }

    let method = extract_method_name(name);

    // Function-type call: `Fn(...)_call` / `Fn[...]_call`. This is the
    // closure-invocation entry point — backends lower it to a real
    // indirect call against the captured function pointer, but at the
    // `runtime_name` layer we treat it as passthrough so the call survives
    // the dispatch table.
    if name.starts_with("Fn(") || name.starts_with("Fn[") {
        return Ok("riven_noop_passthrough");
    }

    // VecIter_, VecIntoIter_, SplitIter_ — iterator combinators.
    // Historically every method here silently no-opped. Only forward
    // user-defined-style names (which downstream link checks will reject
    // if missing); reject anything that *looks* like a known stdlib
    // combinator we haven't actually implemented.
    if name.starts_with("VecIter")
        || name.starts_with("VecIntoIter")
        || name.starts_with("SplitIter")
    {
        return match method {
            // Identity passthroughs: every iterator producer in the v1
            // runtime already hands back a `RivenVec *`, so `to_vec`
            // and `enumerate` are no-ops at the runtime layer. The
            // for-loop lowering (`HirExprKind::For`) detects the
            // `(i, x)` tuple binding shape and synthesises the index
            // counter directly, so `enumerate` only needs to survive
            // type-checking + codegen — no real iterator transform.
            "to_vec" | "enumerate" => Ok("riven_iter_to_vec"),
            "sum" => Ok("riven_vec_sum"),
            "count" => Ok("riven_vec_count"),
            "reverse" => Ok("riven_vec_reverse"),
            "first" => Ok("riven_vec_first"),
            "last" => Ok("riven_vec_last"),
            "clone" => Ok("riven_vec_clone"),
            "contains" => Ok("riven_vec_contains_int"),
            "sort" => Ok("riven_vec_sort"),
            "join" => Ok("riven_vec_join"),
            // Phase 2 stdlib (#05 batch 2): lazy combinators
            // `take(n)` / `skip(n)` eager-materialise into a fresh
            // `RivenVec *` via the `riven_vec_take` / `riven_vec_skip`
            // helpers. Closure-taking terminators (`fold`, `all`,
            // `any`) inline at MIR — the runtime never sees them, so
            // they are intentionally absent from this dispatch table.
            "take" => Ok("riven_vec_take"),
            "skip" => Ok("riven_vec_skip"),
            // Phase 2 stdlib (#05 batch 3): `chain(other)` /
            // `zip(other)` eager-materialise into fresh `RivenVec*`s
            // via the runtime helpers below. `collect_vec` is the v1
            // type-specific shorthand for `collect[Vec[T]]` — since
            // every `*Iter` is already a `RivenVec*` at runtime, the
            // collector is the same identity passthrough as `to_vec`.
            "chain" => Ok("riven_vec_chain"),
            "zip" => Ok("riven_vec_zip"),
            "collect_vec" => Ok("riven_iter_to_vec"),
            // Known unimplemented combinators — refuse rather than no-op.
            "filter" | "find" | "position" | "partition" | "fold" | "min" | "max" | "any"
            | "all" | "collect" | "map" | "reduce" | "flat_map" | "flatten" => {
                Err(unresolved_method_error(name, "Iter"))
            }
            // Anything else falls through to link-time resolution.
            _ => Ok(name),
        };
    }

    // Map[...] / HashMap[...] / Hash[...] methods.
    //
    // `Map` is the Ruby-naming name (docs/specs/syntax/ruby-naming.spec.md);
    // `HashMap` / `Hash` are legacy spellings retained as aliases until
    // sources finish migrating.
    if name.starts_with("Map[")
        || name.starts_with("Map_")
        || name.starts_with("HashMap[")
        || name.starts_with("HashMap_")
        || name.starts_with("Hash[")
        || name.starts_with("Hash_")
    {
        return match method {
            "new" => Ok("riven_hash_new"),
            "from_iter" => Ok("riven_hash_from_iter"),
            // Phase 2 stdlib (#04): full HashMap surface.
            "with_capacity" => Ok("riven_hash_with_capacity"),
            "insert" => Ok("riven_hash_insert"),
            "get" => Ok("riven_hash_get"),
            "remove" => Ok("riven_hash_remove"),
            "clear" => Ok("riven_hash_clear"),
            "keys" => Ok("riven_hash_keys"),
            "values" => Ok("riven_hash_values"),
            "iter" => Ok("riven_hash_iter"),
            "contains_key" => Ok("riven_hash_contains_key"),
            "len" => Ok("riven_hash_len"),
            "is_empty" => Ok("riven_hash_is_empty"),
            _ => Ok(name),
        };
    }

    // Set[...] / HashSet[...] (alias) methods.
    if name.starts_with("Set[")
        || name.starts_with("Set_")
        || name.starts_with("HashSet[")
        || name.starts_with("HashSet_")
    {
        return match method {
            "new" => Ok("riven_set_new"),
            "from_iter" => Ok("riven_set_from_iter"),
            // Phase 2 stdlib (#04): full HashSet surface.
            "with_capacity" => Ok("riven_set_with_capacity"),
            "insert" => Ok("riven_set_insert"),
            "remove" => Ok("riven_set_remove"),
            "clear" => Ok("riven_set_clear"),
            "iter" => Ok("riven_set_iter"),
            "contains" => Ok("riven_set_contains"),
            "len" => Ok("riven_set_len"),
            "is_empty" => Ok("riven_set_is_empty"),
            "union" => Ok("riven_set_union"),
            "intersection" => Ok("riven_set_intersection"),
            "difference" => Ok("riven_set_difference"),
            _ => Ok(name),
        };
    }

    // Array[...] / Vec[...] methods.
    //
    // `Array` is the Ruby-naming name (docs/specs/syntax/ruby-naming.spec.md);
    // `Vec` is the legacy spelling, kept as an alias until sources finish
    // migrating.
    if name.starts_with("Array") || name.starts_with("Vec") {
        return match method {
            "new" => Ok("riven_vec_new"),
            "with_capacity" => Ok("riven_vec_with_capacity"),
            "push" => Ok("riven_vec_push"),
            "pop" => Ok("riven_vec_pop"),
            "len" => Ok("riven_vec_len"),
            "capacity" => Ok("riven_vec_capacity"),
            "get" | "get_mut" | "get_var" => Ok("riven_vec_get_opt"),
            "is_empty" => Ok("riven_vec_is_empty"),
            "each" => Ok("riven_vec_each"),
            // Iterator producers + the identity collector are
            // passthroughs — every iterator in the v1 runtime is
            // already represented by a `RivenVec *`, so `vec.iter`,
            // `vec.into_iter`, and `iter.to_vec` are all no-ops.
            "iter" | "into_iter" | "iter_mut" | "to_vec" | "enumerate" | "as_slice" => {
                Ok("riven_iter_to_vec")
            }
            "sum" => Ok("riven_vec_sum"),
            "count" => Ok("riven_vec_count"),
            "reverse" => Ok("riven_vec_reverse"),
            "first" => Ok("riven_vec_first"),
            "last" => Ok("riven_vec_last"),
            "clone" => Ok("riven_vec_clone"),
            "contains" => Ok("riven_vec_contains_int"),
            "sort" => Ok("riven_vec_sort"),
            "join" => Ok("riven_vec_join"),
            // Phase 2 stdlib batch 1 (#03): mutators, conversions.
            "clear" => Ok("riven_vec_clear"),
            "truncate" => Ok("riven_vec_truncate"),
            "swap" => Ok("riven_vec_swap"),
            "insert" => Ok("riven_vec_insert"),
            "remove" => Ok("riven_vec_remove"),
            "extend" => Ok("riven_vec_extend"),
            // Phase 2 stdlib batch 2 (#03): from_iter, dedup.
            "from_iter" => Ok("riven_vec_from_iter"),
            "dedup" => Ok("riven_vec_dedup"),
            // Known unimplemented Vec methods — historically no-opped.
            "map" | "filter" | "fold" | "min" | "max" | "any" | "all" | "collect" | "find"
            | "position" | "partition" | "reduce" | "zip" | "take" | "skip" | "chain"
            | "flat_map" | "flatten" => Err(unresolved_method_error(name, "Vec")),
            _ => Ok(name),
        };
    }

    // Option[...] methods. `.map` is inlined at the MIR layer (see
    // `inline_option_map`); reaching here means the inliner missed it,
    // which is itself a bug worth surfacing.
    if name.starts_with("Option") || name.contains("Option[") {
        return match method {
            "unwrap_or" => Ok("riven_option_unwrap_or"),
            "expect!" => Ok("riven_option_expect"),
            "unwrap!" => Ok("riven_option_unwrap"),
            "is_some" => Ok("riven_option_is_some"),
            "is_none" => Ok("riven_option_is_none"),
            "ok_or" => Ok("riven_option_ok_or"),
            // Known unimplemented Option combinators. `map` and
            // `unwrap_or_else` are closure-inlined at MIR level.
            "and_then" | "or" | "or_else" | "ok_or_else" | "filter" | "take" | "replace" => {
                Err(unresolved_method_error(name, "Option"))
            }
            _ => Ok(name),
        };
    }

    // Result[...] methods. `map`, `map_err`, and `unwrap_or_else` are
    // closure-inlined at MIR level (`inline_result_map` /
    // `inline_unwrap_or_else`); reaching here for them indicates the
    // call site lacked a closure — that's a real bug worth surfacing.
    if name.starts_with("Result") || name.contains("Result[") {
        return match method {
            "try_op" => Ok("riven_result_try_op"),
            "expect!" => Ok("riven_result_expect"),
            "unwrap!" => Ok("riven_result_unwrap"),
            "is_ok" => Ok("riven_result_is_ok"),
            "is_err" => Ok("riven_result_is_err"),
            "ok" => Ok("riven_result_ok"),
            "err" => Ok("riven_result_err"),
            "unwrap_or" => Ok("riven_result_unwrap_or"),
            // Known unimplemented Result combinators.
            "and_then" | "or" | "or_else" | "ok_or" => Err(unresolved_method_error(name, "Result")),
            _ => Ok(name),
        };
    }

    // Inferred-type method calls (e.g. `?T..._method` from generics that
    // weren't fully resolved at typecheck). The historical `_ =>
    // riven_noop_passthrough` fallback here was the worst silent-failure
    // path; it accepted *any* method on an inferred type and quietly
    // returned the receiver.
    if name.starts_with("?T") || name.starts_with("?") {
        return match method {
            // Result/Option combinators with real symbols.
            "try_op" => Ok("riven_result_try_op"),
            "unwrap_or" => Ok("riven_option_unwrap_or"),
            "unwrap_or_else" => Ok("riven_result_unwrap_or_else"),
            // String operations.
            "clone" => Ok("riven_string_from"),
            "from" => Ok("riven_string_from"),
            "push_str" => Ok("riven_string_push_str"),
            "trim" => Ok("riven_string_trim"),
            "to_lower" => Ok("riven_string_to_lower"),
            // Vec/collection operations with real symbols.
            "len" => Ok("riven_vec_len"),
            "is_empty" => Ok("riven_vec_is_empty"),
            "push" => Ok("riven_vec_push"),
            "pop" => Ok("riven_vec_pop"),
            "get" | "get_mut" | "get_var" => Ok("riven_vec_get_opt"),
            "each" => Ok("riven_vec_each"),
            // User-defined methods commonly used in fixtures — forward
            // to link-time resolution (a missing impl will surface as a
            // link error against the unmangled name).
            "message" | "summary" | "is_actionable" | "is_done" | "weight" | "id" | "title_ref"
            | "priority_ref" | "deadline_ref" | "serialize" | "is_overdue" | "to_display"
            | "assign" | "complete" | "cancel" | "to_string" | "to_s" => Ok(name),
            // Anything else: refuse. This is the P0.5 change — the old
            // `_ => "riven_noop_passthrough"` masked unimplemented stdlib
            // methods (.map, .map_err, .ok_or, .filter, .find, .fold,
            // .sum, .count, .collect, ...) behind a silent identity.
            _ => Err(unresolved_method_error(name, "?T")),
        };
    }

    // Generic type parameter methods (e.g., `T_assign`, `E_message`):
    // forward to link-time resolution.
    if let Some(pos) = name.find('_') {
        let prefix = &name[..pos];
        if prefix.len() <= 2 && !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_uppercase())
        {
            return Ok(name);
        }
    }

    Ok(name)
}
