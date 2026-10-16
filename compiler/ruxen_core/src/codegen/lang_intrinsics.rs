//! Language-intrinsic name → runtime-symbol mapping.
//!
//! This module is the home of everything in the legacy
//! `runtime_table::runtime_name` dispatcher that is NOT a per-class
//! 1:1 FFI alias. Two categories live here:
//!
//! 1. **Compiler-internal mangling.** `Fn(...)_call` /
//!    `Fn[...]_call` (closure invocation), `yield` (block-arg
//!    invocation placeholder), `&any Fn(...)_call` (dyn-erased
//!    closure dispatch). These are not stdlib FFI — they are
//!    intermediate codegen shapes that backends inline or rewrite
//!    elsewhere.
//!
//! 2. **Aliased clusters + inferred-type dispatch.**
//!    `Vec[T]_get_mut` / `Vec[T]_get_var` alias `ruxen_vec_get_opt`
//!    (canonical `get` is in `library/std/array/src/lib.rx`); E0722
//!    rejects duplicate aliases for the same `c_symbol` with different
//!    wire shapes, so they cannot ride the array.rx alias map. (The
//!    former `into_iter`/`iter_mut`/`to_vec`/`enumerate`/`as_slice`
//!    cluster sharing `ruxen_iter_to_vec` was deleted with the
//!    orphaned iterator machinery — Phase B / Milestone 2.) An abstract
//!    `?T..._<m>` callee (a receiver that is still a generic
//!    parameter) is an error: MIR never lets one reach codegen.
//!
//! 3. **Defensive fallback.** Unmangled `MyClass_my_method` shapes
//!    (user-defined types) fall through to a final `Ok(name)` that
//!    lets the link step resolve them.

use super::runtime::{extract_method_name, unresolved_method_error};

/// Map a mangled method name to its runtime C symbol when the name
/// matches one of the language-intrinsic or aliased-cluster
/// patterns described in the module docs.
///
/// Returns `Ok(symbol)` on a successful mapping, `Err(diagnostic)`
/// when a generic combinator has no implementation and a silent
/// passthrough would mask the bug. Returns `Ok(name)` (identity)
/// when the name doesn't match any pattern — the link step will
/// then resolve it against user-defined symbols.
pub fn runtime_name(name: &str) -> Result<&str, String> {
    // Compiler-injected pseudo-calls. These are not method calls;
    // the codegen treats them specially.
    //   - `yield` is the per-call placeholder for invoking the
    //     block argument; backends inline the closure call
    //     elsewhere.
    // (`super` never reaches codegen: MIR lowers it to the parent's
    // `<Parent>_init`, and resolve rejects it outside a subclass
    // `init` with E1127.)
    match name {
        "yield" => return Ok("ruxen_noop_passthrough"),
        // Compiler-internal MIR-synthesised callees emitted by the
        // derive-Display lowering in `mir/lower/derive.rs:85-106`.
        // These are NOT surface methods — they are precision /
        // truncation helpers the `:.<n>` format spec compiler
        // path inserts into the generated `_fmt` body. The user
        // never types `Float.to_string_prec(...)` directly, so
        // there's no place in a .rx class shell to attach them.
        // They belong with the other compiler-internal mangled
        // names (Fn(...)_call, super, yield) rather than in any
        // package's lib block.
        "Float_to_string_prec" => return Ok("ruxen_float_to_string_prec"),
        // (Scalar `to_s` / `to_f` / `to_i` are declared in scalar.rx and
        // `String#to_s` in string.rx; the MIR `ffi_alias_map` rewrites them
        // before codegen reaches here.)
        "String_truncate_chars" => return Ok("ruxen_string_truncate_chars"),
        // Phase E-rest 3 of #06.95: MIR-synthesised Formatter callees
        // from `mir/lower/interpolation.rs::emit_display_dispatch`.
        // `Formatter_new_with_spec(width, precision, align, fill)`
        // constructs a Formatter with the encoded format spec; the
        // synth `_fmt` body calls `Formatter_precision()` to read
        // the precision sentinel (`-1` = unset). Users never type
        // these — they're inserted by the `:.<n>` lowering path.
        // Same category as Float_to_string_prec; no class-body decl
        // attaches.
        "Formatter_new_with_spec" => return Ok("ruxen_fmt_formatter_new_with_spec"),
        "Formatter_precision" => return Ok("ruxen_fmt_formatter_precision"),
        // `String_clone` MIGRATED to `string.rx` (`def clone as
        // "ruxen_string_from"`). The E0722 conflict check now compares
        // post-self-prepend WIRE shapes, so the second alias of
        // `ruxen_string_from` (whose implicit `&self` is wire-identical
        // to `from`'s explicit `&String` param) is admitted. The MIR
        // `ffi_alias_map` rewrites `String_clone → ruxen_string_from`
        // before codegen consults `runtime_name`, so the explicit arm
        // here is no longer needed.
        // (One-string-type ADR: the `&str_*` mangled-callee aliases were
        // removed. There is one string type — a `&String` receiver peels to
        // `String` in `method_home_key`/`type_name`, so string method calls
        // mangle to `String_<m>` and resolve through string.rx's FFI alias
        // map, never a `&str_` prefix. The C runtime symbols themselves
        // (`ruxen_str_parse_uint`, …) are unchanged; only the dead dispatch
        // keys are gone.)
        _ => {}
    }

    let method = extract_method_name(name);

    // Function-type call: `Fn(...)_call` / `Fn[...]_call`. The
    // closure-invocation entry point — backends lower it to a real
    // indirect call against the captured function pointer, but at
    // the `runtime_name` layer we treat it as passthrough so the
    // call survives the dispatch table.
    if name.starts_with("Fn(") || name.starts_with("Fn[") {
        return Ok("ruxen_noop_passthrough");
    }

    // Phase 2 #06.9: belt-and-suspenders for dyn-erased closure
    // dispatch. The MIR lowerer (see `mir/lower/expr/method_call.rs`
    // `is_fn_call`) recognises `any Fn(...)` receivers and emits an
    // indirect call, so this arm should never fire in normal operation.
    if name.starts_with("any Fn(")
        || name.starts_with("any Fn[")
        || name.starts_with("&any Fn(")
        || name.starts_with("&any Fn[")
    {
        return Ok("ruxen_noop_passthrough");
    }

    // NOTE: the `VecIter` / `VecIntoIter` / `SplitIter` combinator
    // dispatch block was removed with the rest of the orphaned iterator
    // machinery (zero-Rust-stdlib migration, Phase B / Milestone 2).
    // Nothing produces those iterator wrapper types — `split`/`chars`/
    // `lines`/`bytes` return `Array`, and no `.rx`/fixture calls
    // `.iter`/`.into_iter`/`.to_vec`/`.enumerate`. The Ruby collectors
    // (`String.from_iter`/`to_set`/`to_h`) are unrelated and preserved
    // via their `ruxen_*_from_iter` symbols.

    // Array[...] / Vec[...] methods.
    //
    // `Array` is the Ruby-naming name
    // (docs/specs/syntax/ruby-naming.spec.md); `Vec` is the
    // legacy spelling, kept as an alias until sources finish
    // migrating.
    //
    // #06.8 T#14 migrated ~28 methods into
    // library/std/array/src/lib.rx as a `class Array do lib
    // "ruxen_runtime" ... end end` shell. The anchor branch in
    // `register_top_level_type_with_ffi` creates a parent DefId
    // for FFI bookkeeping but does NOT insert `Array` into
    // type-scope — `resolve_type_expr`'s hardcoded match arm
    // remains authoritative for `Ty::Array(_)`. MIR's
    // generic-stripping fallback (added in T#17) lets the
    // call-site mangle `Vec[Int]_push` reach the alias-map key
    // `Array_push` and rewrite to the C symbol.
    //
    // `get_mut` / `get_var` MIGRATED to `array.rx` (aliases of
    // `ruxen_vec_get_opt` alongside `get`). The E0722 check now compares
    // post-self-prepend WIRE shapes, so the `&var T` return — a pointer,
    // wire-identical to `get`'s `&T` — no longer conflicts; the MIR
    // `ffi_alias_map` rewrites them before codegen reaches here.
    if name.starts_with("Array") || name.starts_with("Vec") {
        return match method {
            // `map` / `select` / `reject` / `all?` / `any?` / `partition` /
            // `each_with_index` / `find` / `index` / `sort_by` / `reduce`
            // MIGRATED to real `.rx` bodies (Feature C). They now reach
            // codegen as genuine `Array_<m>` MIR functions, so they must
            // FALL THROUGH to the `Ok(name)` arm — the MIR function
            // definition supplies the symbol. (The old blocklist assumed
            // these were always MIR-inlined and never emitted a call; that
            // assumption no longer holds for the Array head.)
            //
            // `flat_map` / `collect_concat` joined that migrated set in
            // B07: they are now `mixin Enumerable[T]` defaults with real
            // bodies, so they too must fall through to `Ok(name)`. Leaving
            // `flat_map` blocked here made the emitted `Array_flat_map`
            // body unreachable and failed the build at its first call site.
            //
            // Still-unimplemented Vec combinators stay blocked: `min` /
            // `max` / `collect` / `flatten`. They need `T` ordering or
            // `to_s(T)`, so they wait on per-`T` monomorphization (E1).
            // (`zip` / `take` / `drop` / `chain` have FFI symbols and never
            // reach this arm — rewritten via the alias map first — but stay
            // listed defensively.)
            "min" | "max" | "collect" | "flatten" => Err(unresolved_method_error(name, "Vec")),
            _ => Ok(name),
        };
    }

    // Option[...] methods. `.map` is inlined at the MIR layer
    // (see `inline_option_map`); reaching here means the
    // inliner missed it, which is itself a bug worth surfacing.
    //
    // #06.8 T#17 migrated `unwrap_or`, `is_some`, `is_none`,
    // `ok_or` into library/std/option_result/src/lib.rx as a
    // `class Option do lib "ruxen_runtime" ... end end` shell.
    // MIR mangles the call site with the surface generic args
    // (`Option[Int]_unwrap_or`) and the ffi_alias_map carries
    // the generic-stripped key (`Option_unwrap_or`); the
    // lookup site in `mir/lower/expr/method_call.rs` peels the
    // `[...]` segment and retries when the exact-shape key
    // misses, so the migrated entries reach the C symbol via
    // the alias path BEFORE codegen consults the arms below.
    // The bang variants (`unwrap!`, `expect!`) are `alias`es of
    // `unwrap` / `expect` in option_result.rx, so MIR mangles the
    // canonical name and the alias map rewrites it.
    if name.starts_with("Option") || name.contains("Option[") {
        return match method {
            // Known unimplemented Option combinators. `map`
            // and `unwrap_or_else` are closure-inlined at MIR
            // level.
            "and_then" | "or" | "or_else" | "ok_or_else" | "filter" | "take" | "replace" => {
                Err(unresolved_method_error(name, "Option"))
            }
            _ => Ok(name),
        };
    }

    // Result[...] methods. `map`, `map_err`, and
    // `unwrap_or_else` are closure-inlined at MIR level
    // (`inline_result_map` / `inline_unwrap_or_else`);
    // reaching here for them indicates the call site lacked a
    // closure — that's a real bug worth surfacing.
    //
    // #06.8 T#17 migrated `unwrap_or`, `is_ok`, `is_err`,
    // `ok`, `err` into the same option_result.rx lib block, and
    // `expect!` / `unwrap!` are `alias`es there. `try_op` (the `?`
    // operator) is lowered inline by MIR and never reaches codegen.
    if name.starts_with("Result") || name.contains("Result[") {
        return match method {
            // Known unimplemented Result combinators.
            "and_then" | "or" | "or_else" | "ok_or" => Err(unresolved_method_error(name, "Result")),
            _ => Ok(name),
        };
    }

    // An abstract callee (`?T_method`, `?T3200_method`) dispatches on a
    // receiver whose type is still a generic parameter. MIR removes every
    // body that emits one (`prune_unmonomorphised_bodies`) or rejects the
    // program, so reaching here is a compiler bug: fail loudly.
    if name.starts_with('?') {
        return Err(unresolved_method_error(name, "?T"));
    }

    // Defensive fallback: forward the unmangled name to
    // link-time resolution. User-defined classes
    // (`MyClass_my_method`) and any method not covered by the
    // above intrinsic patterns land here. If no symbol exists
    // at link time the error surfaces cleanly as an
    // undefined-symbol diagnostic from `ld`.
    Ok(name)
}
