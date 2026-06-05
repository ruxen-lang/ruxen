//! Tests pinning that runtime_table falls through to the unmangled
//! identity arm for every method that has migrated to a class-body
//! `lib` decl in `library/std/*/src/lib.rx`. A `ruxen_*` mapping
//! reappearing in `runtime_name` for any of these would silently mask
//! the FFI-alias-map dispatch path.

use super::*;

#[test]
fn migrated_vec_combinators_fall_through_runtime_table() {
    // #06.8 T#14: every `Vec[T]_<m>` / `Array[T]_<m>` entry that
    // had a real `ruxen_vec_*` mapping was moved to a class-body
    // `lib` decl in `library/std/array/src/lib.rx`. MIR's
    // ffi_alias_map carries the generic-stripped key
    // (`Array_sum`); the lookup site in
    // `mir/lower/expr/method_call.rs` (and the array-literal /
    // field-access lowerers in `constructors.rs` /
    // `field_access.rs`) strip the `[Int]` segment and route the
    // call through the alias path before codegen would consult
    // runtime_table. The pin set below covers a representative
    // sample of the migrated surface; a `ruxen_*` mapping
    // reappearing here for any of them would silently mask the
    // alias path.
    for m in [
        "Vec[Int]_sum",
        "Vec[Int]_count",
        "Vec[Int]_reverse",
        "Vec[Int]_first",
        "Vec[Int]_last",
        "Vec[Int]_push",
        "Vec[Int]_pop",
        "Vec[Int]_len",
        "Vec[Int]_is_empty",
        "Vec[Int]_get",
        "Vec[Int]_clear",
        "Vec[Int]_extend",
        "Array[Int]_clone",
    ] {
        assert_eq!(
            runtime_name(m).unwrap(),
            m,
            "migrated method `{m}` must fall through runtime_table to its \
             unmapped form; the alias-map fallback in MIR carries the \
             generic-stripped `Array_<m>` key"
        );
    }
    // Aliased clusters that share a C symbol with a migrated
    // entry survive in runtime_table — `register_class_lib_method`'s
    // E0722 check rejects duplicate aliases for the same `c_symbol`
    // with different wire shapes.
    assert_eq!(
        runtime_name("Vec[Int]_get_mut").unwrap(),
        "ruxen_vec_get_opt"
    );
    assert_eq!(
        runtime_name("Vec[Int]_into_iter").unwrap(),
        "ruxen_iter_to_vec"
    );
    assert_eq!(
        runtime_name("Vec[Int]_to_vec").unwrap(),
        "ruxen_iter_to_vec"
    );
}

#[test]
fn migrated_string_methods_fall_through_runtime_table() {
    // #06.8 T#13: every `String_<m>` entry that previously lived in
    // `runtime_table/mod.rs` moved to a class-body `lib` decl in
    // `library/std/string/src/lib.rx`. Method calls now reach the C
    // symbol through MIR's `ffi_alias_map` rewrite BEFORE codegen
    // would consult `runtime_name`. The runtime_table lookup
    // therefore falls through to the bottom-of-fn `Ok(name)` arm
    // (the same arm that lets user-defined methods through
    // unmolested) — it must NOT return the legacy `ruxen_string_*`
    // mapping for migrated names, otherwise a stale entry could
    // mask the alias path and silently keep the old dispatch alive
    // across future refactors.
    //
    // `String_clone` is the only outlier: it aliases the SAME C
    // symbol as `String.from` (`ruxen_string_from`) but with an
    // instance-method receiver shape, which trips E0722 in
    // `register_class_lib_method`. Until that check is relaxed
    // to compare at the wire level, it stays as an explicit
    // runtime_table entry — pinned below alongside the migrated
    // set.
    for m in [
        "String_contains",
        "String_starts_with",
        "String_ends_with",
        "String_repeat",
        "String_trim",
        "String_len",
        "String_from",
        "String_push_str",
        "String_to_lower",
        "String_to_upper",
        "String_chars",
        "String_split",
        "String_new",
        "String_find",
        "String_remove",
        "String_parse_int",
        "ParseIntError_message",
        "ParseFloatError_message",
    ] {
        assert_eq!(
            runtime_name(m).unwrap(),
            m,
            "migrated method `{m}` must fall through runtime_table to its \
             unmapped form (the alias-map rewrites it earlier in MIR); \
             a specific `ruxen_*` mapping here would mask the alias path"
        );
    }
    assert_eq!(
        runtime_name("String_clone").unwrap(),
        "ruxen_string_from",
        "String_clone must stay in lang_intrinsics — E0722 keeps it \
         out of string.rx (see lang_intrinsics doc comment)"
    );
}

#[test]
fn iterator_passthrough_collectors_resolve() {
    // The non-migrated identity passthroughs (`into_iter`,
    // `iter_mut`, `to_vec`, `enumerate`, `as_slice`) all share
    // the `ruxen_iter_to_vec` C symbol with the migrated `iter`.
    // E0722 keeps us from declaring all five in
    // `library/std/array/src/lib.rx`, so the runtime_table arm is
    // still the source of truth for the aliases. `iter` ALONE
    // moved to the alias map.
    assert_eq!(
        runtime_name("Vec[Int]_into_iter").unwrap(),
        "ruxen_iter_to_vec",
    );
    assert_eq!(
        runtime_name("Vec[Int]_to_vec").unwrap(),
        "ruxen_iter_to_vec",
    );
    assert_eq!(
        runtime_name("SplitIter_to_vec").unwrap(),
        "ruxen_iter_to_vec",
    );
    // `Vec[Int]_iter` itself now falls through (migrated).
    assert_eq!(runtime_name("Vec[Int]_iter").unwrap(), "Vec[Int]_iter");
}

#[test]
fn migrated_option_result_combinators_fall_through_runtime_table() {
    // #06.8 T#17 moved `Option_{unwrap_or, present?, nil?,
    // ok_or}` and `Result_{unwrap_or, ok?, err?, ok, err}`
    // into library/std/option_result/src/lib.rx. The lookup site in
    // `mir/lower/expr/method_call.rs` peels the surface
    // `[Int,Err]` generic args and consults `ffi_alias_map` with
    // the generic-stripped key, so the alias rewrite reaches the
    // C symbol BEFORE codegen consults `runtime_name`. The
    // runtime_table lookup therefore falls through to the bottom
    // `Ok(name)` arm — a `ruxen_*` mapping at this layer would
    // mask the alias path.
    for m in [
        "Result[Int,Err]_unwrap_or",
        "Option[Int]_ok_or",
        "Option[String]_present?",
        "Option[String]_nil?",
        "Result[Int,IoError]_ok?",
        "Result[Int,IoError]_err?",
        "Result[Int,IoError]_ok",
        "Result[Int,IoError]_err",
    ] {
        assert_eq!(
            runtime_name(m).unwrap(),
            m,
            "migrated method `{m}` must fall through runtime_table to its \
             unmapped form; the alias-map fallback in MIR carries the \
             generic-stripped `Option_<m>` / `Result_<m>` key"
        );
    }
    // Surviving bang variants — the `!` is part of the surface
    // method name but isn't yet accepted inside a `def NAME as`
    // lib decl, so these still need the runtime_table entry.
    assert_eq!(
        runtime_name("Option[Int]_unwrap!").unwrap(),
        "ruxen_option_unwrap"
    );
    assert_eq!(
        runtime_name("Result[Int,IoError]_unwrap!").unwrap(),
        "ruxen_result_unwrap"
    );
}

#[test]
fn migrated_collection_methods_fall_through_runtime_table() {
    // After the Wave 2 self-hosting sequence, every collection-
    // method mapping that used to live in runtime_table has
    // moved to a class-body lib decl in library/std/<pkg>/src/lib.rx.
    // This test pins that runtime_table no longer carries a
    // `ruxen_*` mapping for any of them — falling through to
    // the bottom `Ok(name)` arm is the contract that prevents
    // a stale entry from masking the alias path.
    //
    // | Surface mangle              | Migration |
    // |-----------------------------|-----------|
    // | puts                        | io.rx (Wave 2)   |
    // | Vec[Int]_push / _len        | array.rx (T#14)  |
    // | Hash[K,V]_get / Map[..]_get | map.rx (T#15)    |
    // | HashMap[..]_get             | map.rx (T#15)    |
    // | Set[..]_insert / contains   | set.rx (T#16)    |
    // | HashSet[..]_insert          | set.rx (T#16)    |
    for name in [
        "puts",
        "Vec[Int]_push",
        "Vec[Int]_len",
        "Hash[Int,Int]_get",
        "Map[Int,Int]_get",
        "HashMap[Int,Int]_get",
        "Set[Int]_insert",
        "Set[Int]_contains",
        "HashSet[Int]_insert",
    ] {
        assert_eq!(
            runtime_name(name).unwrap(),
            name,
            "post-migration `{name}` must fall through runtime_table; \
             a `ruxen_*` mapping here would mask the alias-map path"
        );
    }
}

#[test]
fn stdio_top_level_fall_through_after_io_migration() {
    // #06.8 Wave 2 (io.rx): the top-level stdio fns `read_line`
    // / `stdin` / `stdout` / `stderr` moved to a `lib
    // "ruxen_runtime"` block in `library/std/io/src/lib.rx`. Their
    // runtime_table entries were deleted at the same time, so
    // method calls now reach the C symbols through MIR's
    // ffi_alias_map; runtime_table falls through to the
    // bottom-of-fn `Ok(name)` arm.
    for name in ["read_line", "stdin", "stdout", "stderr"] {
        assert_eq!(
            runtime_name(name).unwrap(),
            name,
            "post-io-migration `{name}` must fall through runtime_table; \
             a `ruxen_*` mapping here would mask the alias path"
        );
    }
    // #06.95 Phase E Slice B.2 migrated the Stdin / Stdout /
    // Stderr CLASS METHODS into class-body `lib "runtime/stdio.c"`
    // blocks on the matching shells in `library/std/io/src/lib.rx`.
    // The FFI alias map rewrites the mangled callee
    // (`Stdin_read_line` → `ruxen_stdin_read_line`) at MIR-lowering
    // time, so runtime_table now falls through to the unmangled
    // `Ok(name)` arm in `lang_intrinsics`. A specific mapping
    // returning here would mask the alias path.
    for name in [
        "Stdin_read_line",
        "Stdin_read_to_string",
        "Stdin_lines",
        "Stdout_write_str",
        "Stdout_flush",
        "Stdout_print",
        "Stdout_println",
        "Stderr_write_str",
        "Stderr_flush",
        "Stderr_eprint",
        "Stderr_eprintln",
    ] {
        assert_eq!(
            runtime_name(name).unwrap(),
            name,
            "post-Slice-B.2 `{name}` must fall through runtime_table; \
             a `ruxen_*` mapping here would mask the alias path"
        );
    }
}
