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
        // `String_clone` + `String_from_iter` moved to
        // `codegen::lang_intrinsics` in #06.95 Phase E Slice B.5 —
        // both are compiler-internal mangled names (one is the
        // E0722-locked alias for `String.from`, one is the
        // MIR-synthesised `iter.collect[String]()` callee) that
        // have no surface method to attach to in string.rvn.
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
        // `&str_as_str` moved to `codegen::lang_intrinsics` in Slice A
        // (it's an identity passthrough, not a `&str` method); the
        // delegation at the bottom of this fn handles it now.
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
        // Stdin / Stdout / Stderr methods migrated to
        // library/std/io/src/lib.rvn `class Stdin/Stdout/Stderr do lib
        // "runtime/stdio.c" ... end end` in #06.95 Phase E Slice B.2.
        // The FFI alias map rewrites `Std{in,out,err}_<m>` callees
        // at MIR-lowering time before codegen consults this table.
        // Metadata accessor methods migrated to
        // library/std/fs/src/lib.rvn `class Metadata do lib
        // "runtime/fs.c" ... end end` in #06.95 Phase E Slice B.1.
        // The FFI alias map rewrites `Metadata_<m>` callees at
        // MIR-lowering time before codegen consults this table.
        // Phase 2 stdlib (#06): std::process::Command builder + Output /
        // ExitStatus accessors. `Command_new` is dispatched via the
        // collection-ctor fast path in mir/lower.rs (alongside
        // Vec_new / Hash_new / Formatter_new); the rest go through the
        // standard `{Type}_{method}` regular method-call path.
        // ExitStatus accessors migrated to library/std/process/src/lib.rvn
        // `class ExitStatus do lib "runtime/process.c" ... end end`
        // in #06.95 Phase E Slice B.3. The FFI alias map rewrites
        // `ExitStatus_<m>` callees at MIR-lowering time.
        // Command / Output stay here (not in user_drop_classes
        // would have been safe but the broader migration triggered
        // double-free; see Slice B.3 first-attempt revert).
        // Phase 2 stdlib (#06.5 T2): File / OpenOptions surface.
        // `File_open/create/append/open_options` are static-style
        // constructors that go through the standard `{Type}_{method}`
        // mangling path. `File_drop` is registered in the
        // user_drop_classes set (mir/lower/collect.rs) so the MIR
        // emits it before the spine dealloc at scope exit.
        "File_metadata" => return Ok("riven_file_metadata"),
        "OpenOptions_new" => return Ok("riven_open_options_new"),
        "OpenOptions_read" => return Ok("riven_open_options_read"),
        "OpenOptions_write" => return Ok("riven_open_options_write"),
        "OpenOptions_append" => return Ok("riven_open_options_append"),
        "OpenOptions_truncate" => return Ok("riven_open_options_truncate"),
        "OpenOptions_create" => return Ok("riven_open_options_create"),
        "OpenOptions_create_new" => return Ok("riven_open_options_create_new"),
        // Phase 2 stdlib (#06.A3): std::fmt::Formatter methods.
        // Phase 2 stdlib (#06.D4): spec-aware constructor + precision
        // accessor + per-type precision helpers.
        "Formatter_new_with_spec" => return Ok("riven_fmt_formatter_new_with_spec"),
        "Formatter_precision" => return Ok("riven_fmt_formatter_precision"),
        // `Float_to_string_prec` and `String_truncate_chars` moved
        // to `codegen::lang_intrinsics` in #06.95 Phase E Slice B.5
        // — they are MIR-synthesised callees from
        // `mir/lower/derive.rs:85-106` (the `:.<n>` precision format
        // path), not surface methods. No .rvn class shell can attach
        // to them. The lang_intrinsics arms now own the mapping.
        // `Thread_sleep -> riven_thread_sleep_ns` is the bare-int
        // convenience overload (`Thread.sleep(0)`). Stays in
        // runtime_table — see the comment on `class Thread` in
        // library/std/sync/src/lib.rvn for why the Duration and Int
        // overloads can't both ride the alias map yet.
        "Thread_sleep" => return Ok("riven_thread_sleep_ns"),
        // `Thread_yield_now` migrated to library/std/sync/src/lib.rvn
        // `class Thread do lib "runtime/time.c" ... end end` in
        // #06.95 Phase E Slice B.4.
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
        "TcpListener_set_nonblocking" => return Ok("riven_tcp_listener_set_nonblocking"),
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
        // std::signal — graceful-shutdown surface.
        // std::rand entries were here — Wave 2 (#06.8) migration moved
        // the C-symbol binding to `lib "riven_runtime" def random_bytes
        // as "riven_rand_random_bytes" …` in library/std/rand/src/lib.rvn.
        // The FFI alias map populated during MIR lowering rewrites
        // call-site callees from the Riven name to the C symbol BEFORE
        // codegen consults this table, so the table entries became
        // dead code.
        // Compiler-injected pseudo-calls (`super`, `yield`) and the
        // closure-call / iterator-combinator / inferred-type
        // dispatchers all moved to `codegen::lang_intrinsics` in
        // Phase E Slice A. Plain-alias arms above stay here until
        // their per-package migration into `library/std/<pkg>/src/
        // lib.rvn` lands.
        _ => {}
    }

    // Delegate everything that did not match a plain-alias arm to
    // the language-intrinsic dispatcher. That covers the closure
    // / `super` / `yield` / `?T` / iterator-combinator / generic-T
    // forwarding paths plus the defensive `Ok(name)` bottom arm.
    super::lang_intrinsics::runtime_name(name)
}
