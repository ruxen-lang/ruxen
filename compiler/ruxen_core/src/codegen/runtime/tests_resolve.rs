//! Tests for `runtime_name` resolution behaviour: rejection of unknown
//! generic methods, identity passthrough for user-defined methods, and
//! the small `super` / `yield` / `&str_as_str` / `Thread_*` arms.

use super::*;

#[test]
fn unknown_inferred_type_method_is_rejected() {
    // Historically `?T_xxx_totally_fake` would silently map to
    // `ruxen_noop_passthrough`. P0.5: it must error instead.
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
    // `sum` and `count` now resolve to `ruxen_vec_sum`/`ruxen_vec_count`
    // — see `implemented_vec_combinators_resolve`. The list here is
    // limited to combinators with no MIR inliner and no runtime symbol.
    for m in [
        "Vec[Int]_reduce",
        "Vec[Int]_collect",
        "Vec[Int]_map",
        "Vec[Int]_select",
    ] {
        assert!(
            runtime_name(m).is_err(),
            "expected `{m}` to be rejected (was {:?})",
            runtime_name(m)
        );
    }
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
fn user_defined_methods_forward() {
    assert_eq!(
        runtime_name("MyClass_my_method").unwrap(),
        "MyClass_my_method"
    );
}

#[test]
fn yield_super_and_str_identity_still_resolve() {
    assert_eq!(runtime_name("super").unwrap(), "ruxen_noop");
    assert_eq!(runtime_name("yield").unwrap(), "ruxen_noop_passthrough");
    assert_eq!(
        runtime_name("&str_as_str").unwrap(),
        "ruxen_noop_passthrough"
    );
}

#[test]
fn thread_runtime_methods_resolve() {
    // Phase E-rest 4b of #06.95: `Thread_sleep` now falls through
    // because `library/std/sync/src/lib.rx` carries the only
    // `def self.sleep as "ruxen_thread_sleep_duration"` lib decl.
    // The alias map rewrites `Thread_sleep` to that C symbol
    // before codegen consults `runtime_name`, so the lookup here
    // returns the unmangled identity. (The bare-int overload
    // `Thread.sleep(0)` still works because the C runtime null-
    // checks the Duration pointer — see commit 6e36eb8.)
    assert_eq!(runtime_name("Thread_sleep").unwrap(), "Thread_sleep");
    // `Thread.yield_now` migrated to library/std/sync/src/lib.rx
    // in #06.95 Phase E Slice B.4 — the alias map rewrites
    // `Thread_yield_now` before codegen consults runtime_table,
    // so the lookup here falls through to the unmangled identity
    // arm.
    assert_eq!(
        runtime_name("Thread_yield_now").unwrap(),
        "Thread_yield_now"
    );
}
