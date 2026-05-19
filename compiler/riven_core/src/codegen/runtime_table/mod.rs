//! Method-name → runtime-symbol-name mapping table — DEPRECATED, being
//! dissolved by #06.95 Phase E.
//!
//! Phase E of #06.95 is in the process of dissolving this module.
//! Slice A moved the trailing dispatcher logic (closure-call
//! passthroughs, VecIter / Array / Option / Result combinator
//! routing, `?T` inferred-type fallback, generic-T forwarding,
//! defensive `Ok(name)`) to `codegen::lang_intrinsics`. The
//! remaining plain-alias arms below are migrating per-package into
//! `library/std/<pkg>/src/lib.rvn` `class Foo do lib "runtime/X.c"
//! ... end end` shells, one slice at a time. When the match below is
//! down to just the inner-type-discriminator entries
//! (`BufReader_new_file`, `BufWriter_into_inner_tcp`, …), those move
//! to `lang_intrinsics` too and this file goes away.
//!
//! See plan in `docs/prompts/v1/06_phase2_stdlib_io_fmt.md` Phase E.

pub fn runtime_name(name: &str) -> Result<&str, String> {
    // Direct, well-known symbols. These are the only call names that can
    // safely resolve to `riven_noop*` helpers — they are language-level
    // constructs (closure invocation, super-call, &str/&str identity)
    // rather than method calls that pretend to do work.
    match name {
        // Top-level I/O / printing entries were here. Wave 2 (#06.8)
        // moved puts / eputs / print / println / eprintln / read_line
        // / stdin / stdout / stderr to library/std/io/src/lib.rvn as a
        // `lib "riven_runtime"` block. The println / eprintln aliases
        // (sharing the riven_puts / riven_eputs C symbols) are
        // preserved verbatim in the .rvn lib block.
        // Top-level env entries were here — Wave 2 (#06.8) moved
        // args / get / vars / current_dir to library/std/env/src/lib.rvn
        // as `lib "riven_runtime" def NAME as "riven_env_..."` aliases.
        // Top-level fs entries were here. Wave 2 (#06.8) moved all
        // seventeen to library/std/fs/src/lib.rvn as a `lib "riven_runtime"`
        // block with `def NAME as "riven_fs_NAME"` aliases. The FFI
        // alias map rewrites callees at MIR-lowering time before
        // codegen consults this table.
        // `exit` was here — Wave 2 (#06.8) moved the C-symbol binding
        // to `lib "riven_runtime" def exit as "riven_process_exit"`
        // in library/std/process/src/lib.rvn. The FFI alias map rewrites
        // the callee at MIR-lowering time before codegen consults
        // this table.
        // `process_run` removed in #06.5 T5.5 — superseded by
        // `Command_{status,output}`. The C symbol `riven_process_run`
        // is still linked (it is the implementation behind
        // `riven_command_status`); the resolver-visible Riven name
        // is gone. See docs/specs/stdlib/process.spec.md.
        // String methods.
        //
        // #06.8 T#13 migrated the bulk of these into
        // library/std/string/src/lib.rvn as a `class String do lib
        // "riven_runtime" ... end end` block. MIR's ffi_alias_map is
        // populated from the bootstrap merge BEFORE codegen consults
        // this table, so any callee that has a matching .rvn lib
        // decl never reaches the arms below — the entries would be
        // dead. Two String entries remain because they don't fit
        // the FFI-alias mold:
        //
        // * `String_clone` aliases the SAME C symbol as `String.from`
        //   (`riven_string_from`) but with an instance-method
        //   receiver shape. Two FFI decls aliasing the same C symbol
        //   with different wire shapes trip the E0722 conflict check
        //   in `register_class_lib_method`. Until that check is
        //   relaxed to compare at the wire level
        //   (post-instance-self-prepend), this stays as a
        //   runtime_table fallback.
        // * `String_from_iter` is consumed by the
        //   `iter.collect[String]()` lowering, not by a surface
        //   `.from_iter(...)` method call — there's no `.rvn`
        //   class-body decl to attach it to.
        "String_clone" => return Ok("riven_string_from"),
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
        // `unix_ns` was here — Wave 2 (#06.8) moved the C-symbol
        // binding to `lib "riven_runtime" def unix_ns as
        // "riven_time_unix_ns"` in library/std/time/src/lib.rvn.
        // Phase 2 stdlib (#06.5 T4): Duration / Instant scalar-wrapper
        // classes. `Duration_from_*` and `Instant_now` are static-style
        // constructors that go through the "collection-ctor fast path"
        // in mir/lower/expr/method_call.rs (alongside File_open,
        // Command_new), so they receive no synthetic `self`. The rest
        // are regular instance methods on the {Type}_{method}
        // mangling path. `sleep` is the top-level free fn in the new
        // `std.thread` module.
        // Duration / Instant methods migrated to
        // library/std/time/src/lib.rvn (#06.8 T#20 follow-through). MIR's
        // ffi_alias_map carries the parent-name-keyed entries; the
        // static-ctor fast path's alias-map lookup (T#14) rewrites
        // the `Duration_from_secs` / `Instant_now` callees before
        // codegen consults runtime_table. The 4-layer routing
        // (static-ctor / field-access / general method dispatch /
        // ArrayLiteral) is uniform across all sites.
        "sleep" => return Ok("riven_thread_sleep_duration"),
        // std::path entries were here — Wave 2 (#06.8) migration moved
        // the C-symbol bindings to library/std/path/src/lib.rvn. See the
        // std::rand comment below for the FFI alias rewrite path that
        // makes the runtime_table lookup unnecessary.
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
        // Phase 2 stdlib (#06.5 T6): std::io::BufReader[R] /
        // BufWriter[W] over the closed set {File, TcpStream}. The
        // static ctors (`new`, `with_capacity`) carry a `_file` /
        // `_tcp` suffix picked at MIR lowering from args[0|1].ty;
        // the `into_inner` instance method does the same on the
        // receiver's generic_args[0]. Other instance methods
        // (`read_line`, `read`, `write*`, `flush`) branch on the
        // 1-byte `kind` tag inside the runtime spine.
        "BufReader_new_file" => return Ok("riven_bufreader_new_file"),
        "BufReader_new_tcp" => return Ok("riven_bufreader_new_tcp"),
        "BufReader_with_capacity_file" => return Ok("riven_bufreader_with_capacity_file"),
        "BufReader_with_capacity_tcp" => return Ok("riven_bufreader_with_capacity_tcp"),
        "BufReader_read_line" => return Ok("riven_bufreader_read_line"),
        "BufReader_read" => return Ok("riven_bufreader_read"),
        "BufReader_into_inner_file" => return Ok("riven_bufreader_into_inner_file"),
        "BufReader_into_inner_tcp" => return Ok("riven_bufreader_into_inner_tcp"),
        "BufReader_drop" => return Ok("riven_bufreader_drop"),
        "BufWriter_new_file" => return Ok("riven_bufwriter_new_file"),
        "BufWriter_new_tcp" => return Ok("riven_bufwriter_new_tcp"),
        "BufWriter_with_capacity_file" => return Ok("riven_bufwriter_with_capacity_file"),
        "BufWriter_with_capacity_tcp" => return Ok("riven_bufwriter_with_capacity_tcp"),
        "BufWriter_write" => return Ok("riven_bufwriter_write"),
        "BufWriter_write_all" => return Ok("riven_bufwriter_write_all"),
        "BufWriter_write_str" => return Ok("riven_bufwriter_write_str"),
        "BufWriter_flush" => return Ok("riven_bufwriter_flush"),
        "BufWriter_into_inner_file" => return Ok("riven_bufwriter_into_inner_file"),
        "BufWriter_into_inner_tcp" => return Ok("riven_bufwriter_into_inner_tcp"),
        "BufWriter_drop" => return Ok("riven_bufwriter_drop"),
        // std::signal — graceful-shutdown surface.
        "signal_install_sigint" => return Ok("riven_signal_install_sigint"),
        "signal_received_sigint" => return Ok("riven_signal_received_sigint"),
        // std::rand entries were here — Wave 2 (#06.8) migration moved
        // the C-symbol binding to `lib "riven_runtime" def random_bytes
        // as "riven_rand_random_bytes" …` in library/std/rand/src/lib.rvn.
        // The FFI alias map populated during MIR lowering rewrites
        // call-site callees from the Riven name to the C symbol BEFORE
        // codegen consults this table, so the table entries became
        // dead code.
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

    // Phase 2 #06.9: belt-and-suspenders for dyn-erased closure
    // dispatch. The MIR lowerer (see `mir/lower/expr/method_call.rs`
    // `is_fn_call`) recognises `any Fn(...)` / `?T*` receivers and
    // emits an indirect call, so this arm should never fire in normal
    // operation. It exists so a missed lowering path produces a
    // deterministic noop-passthrough instead of a hard codegen error
    // — easier to root-cause when a regression slips in. The `?T*`
    // form is the `Ty::Infer(N)` Display mangling: an unresolved
    // type variable leaking into a `.call(...)` site.
    if name.starts_with("any Fn(")
        || name.starts_with("any Fn[")
        || name.starts_with("&any Fn(")
        || name.starts_with("&any Fn[")
        || (name.starts_with("?T") && name.ends_with("_call"))
    {
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

    // Map / HashMap / Hash methods — fully migrated to
    // library/std/map/src/lib.rvn (#06.8 T#15). No runtime_table arm:
    // the alias-map key `Map_<m>` (registered by the bootstrap
    // `class Map` shell) wins through `resolve_ffi_alias_callee`'s
    // generic-stripping fallback, and the bottom-of-fn `Ok(name)`
    // arm catches anything that escapes.
    //
    // Set / HashSet methods — fully migrated to
    // library/std/set/src/lib.rvn (#06.8 T#16). Same shape as Map.

    // Array[...] / Vec[...] methods.
    //
    // `Array` is the Ruby-naming name (docs/specs/syntax/ruby-naming.spec.md);
    // `Vec` is the legacy spelling, kept as an alias until sources finish
    // migrating.
    //
    // #06.8 T#14 migrated ~28 methods into library/std/array/src/lib.rvn
    // as a `class Array do lib "riven_runtime" ... end end` shell.
    // The anchor branch in `register_top_level_type_with_ffi` creates
    // a parent DefId for FFI bookkeeping but does NOT insert
    // `Array` into type-scope — `resolve_type_expr`'s hardcoded
    // match arm remains authoritative for `Ty::Array(_)`. MIR's
    // generic-stripping fallback (added in T#17) lets the call-site
    // mangle `Vec[Int]_push` reach the alias-map key `Array_push`
    // and rewrite to the C symbol.
    //
    // Two aliased clusters stay below as runtime_table fallbacks
    // because they share a C symbol with a migrated entry and the
    // E0722 check rejects duplicate aliases for the same `c_symbol`
    // with different wire shapes:
    //
    //   * `get_mut`, `get_var` → `riven_vec_get_opt` (canonical
    //     spelling `get` is in array.rvn)
    //   * `into_iter`, `iter_mut`, `to_vec`, `enumerate`, `as_slice`
    //     → `riven_iter_to_vec` (canonical spelling `iter` is in
    //     array.rvn)
    if name.starts_with("Array") || name.starts_with("Vec") {
        return match method {
            "get_mut" | "get_var" => Ok("riven_vec_get_opt"),
            // Iterator producers + the identity collector are
            // passthroughs — every iterator in the v1 runtime is
            // already represented by a `RivenVec *`, so
            // `vec.into_iter`, `iter.to_vec`, etc. are all no-ops.
            "into_iter" | "iter_mut" | "to_vec" | "enumerate" | "as_slice" => {
                Ok("riven_iter_to_vec")
            }
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
    //
    // #06.8 T#17 migrated `unwrap_or`, `is_some`, `is_none`, `ok_or`
    // into library/std/option_result/src/lib.rvn as a `class Option do
    // lib "riven_runtime" ... end end` shell. MIR mangles the call
    // site with the surface generic args (`Option[Int]_unwrap_or`)
    // and the ffi_alias_map carries the generic-stripped key
    // (`Option_unwrap_or`); the lookup site in
    // `mir/lower/expr/method_call.rs` peels the `[...]` segment and
    // retries when the exact-shape key misses, so the migrated
    // entries reach the C symbol via the alias path BEFORE codegen
    // consults the arms below. The bang variants (`unwrap!`,
    // `expect!`) can't ride the alias map yet — the `!` is part of
    // the method-name surface but the parser doesn't accept it
    // inside a `def NAME as ...` lib decl. Those stay here as
    // explicit fallbacks.
    if name.starts_with("Option") || name.contains("Option[") {
        return match method {
            "expect!" => Ok("riven_option_expect"),
            "unwrap!" => Ok("riven_option_unwrap"),
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
    //
    // #06.8 T#17 migrated `unwrap_or`, `is_ok`, `is_err`, `ok`,
    // `err` into the same option_result.rvn lib block. The remaining
    // arms below are non-aliasable (bang surface names, `try_op`
    // which is also a MIR-special) and stay here.
    if name.starts_with("Result") || name.contains("Result[") {
        return match method {
            "try_op" => Ok("riven_result_try_op"),
            "expect!" => Ok("riven_result_expect"),
            "unwrap!" => Ok("riven_result_unwrap"),
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
