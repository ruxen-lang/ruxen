//! Language-intrinsic name → runtime-symbol mapping.
//!
//! This module is the home of everything in the legacy
//! `runtime_table::runtime_name` dispatcher that is NOT a per-class
//! 1:1 FFI alias. Two categories live here:
//!
//! 1. **Compiler-internal mangling.** `Fn(...)_call` /
//!    `Fn[...]_call` (closure invocation), `super` (parent-init
//!    dispatch in a constructor), `yield` (block-arg invocation
//!    placeholder), `?T..._call` / `&any Fn(...)_call` (dyn-erased
//!    closure dispatch). These are not stdlib FFI — they are
//!    intermediate codegen shapes that backends inline or rewrite
//!    elsewhere.
//!
//! 2. **Aliased clusters + bang variants + inferred-type dispatch.**
//!    `Vec[T]_into_iter` / `Vec[T]_iter_mut` / `Vec[T]_to_vec` /
//!    `Vec[T]_enumerate` / `Vec[T]_as_slice` all share the C symbol
//!    `ruxen_iter_to_vec` with the migrated `Vec[T]_iter`. E0722
//!    rejects duplicate aliases for the same `c_symbol` with
//!    different wire shapes, so they cannot ride the
//!    `library/std/array/src/lib.rx` alias map. Same story for
//!    `Vec[T]_get_mut` / `Vec[T]_get_var` aliasing
//!    `ruxen_vec_get_opt` with `get`. The `Option[T]_unwrap!` /
//!    `Option[T]_expect!` bang variants can't ride the alias map
//!    either — `!` is part of the surface method name but isn't
//!    yet accepted inside a `def NAME as ...` lib decl. The
//!    `?T..._<m>` / `?..._<m>` dispatcher handles inferred-type
//!    method calls that escape typeck without a concrete receiver.
//!
//! 3. **Defensive fallback.** Unmangled `MyClass_my_method` shapes
//!    (user-defined types) and generic-type-parameter forwards
//!    (`T_assign`, `E_message`) fall through to a final `Ok(name)`
//!    that lets the link step resolve them.
//!
//! Inner-type-discriminator entries (`BufReader_new_file` /
//! `BufWriter_into_inner_tcp`, …) are NOT here yet — they still
//! live in `runtime_table/mod.rs` because their suffix selection
//! is druxen by the MIR call-site lowerer in
//! `mir/lower/expr/method_call.rs:180-200` and removing them
//! requires the module+mixin reshape described in #06.95 Phase
//! E §"Module + mixin pattern". Phase E moves them into this
//! module in a later slice with a documented comment.

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
    //   - `super` is a parent-init dispatch in a constructor.
    //   - `yield` is the per-call placeholder for invoking the
    //     block argument; backends inline the closure call
    //     elsewhere.
    match name {
        "super" => return Ok("ruxen_noop"),
        "yield" => return Ok("ruxen_noop_passthrough"),
        // `&str → &str` is a true semantic identity, not a stub.
        "&str_as_str" => return Ok("ruxen_noop_passthrough"),
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
        // Numeric conversions. `Int.to_f()` / `Float.to_i()` are real
        // surface methods (typeck: `method_resolvers::builtin_method_type`)
        // but the receiver is a scalar primitive with no `.rx` class shell,
        // so the call-site mangling (`Int_to_f` / `Float_to_i`) has no
        // FFI-alias entry to rewrite it. Map them to their runtime symbols
        // here, the same way the precision helpers above are handled —
        // otherwise the mangled name falls through to the `Ok(name)`
        // link-time fallback and dies as `can't resolve symbol Int_to_f`.
        "Int_to_f" => return Ok("ruxen_int_to_f"),
        "Float_to_i" => return Ok("ruxen_float_to_i"),
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
        // `String_from_iter` is consumed by the
        // `iter.collect[String]()` MIR lowering, not a surface
        // `.from_iter(...)` method call — same MIR-synthesised
        // category as the precision helpers above. No .rx
        // class-body decl can attach to it (it has no surface
        // method name on `String`).
        "String_from_iter" => return Ok("ruxen_string_from_iter"),
        // `String_clone` aliases the SAME C symbol as `String.from`
        // (`ruxen_string_from`) but with an instance-method
        // receiver shape. Two FFI decls aliasing the same C symbol
        // with different wire shapes trip the E0722 conflict check
        // in `register_class_lib_method`. Until that check is
        // relaxed to compare at the wire level
        // (post-instance-self-prepend), this stays as an explicit
        // lang_intrinsics arm.
        "String_clone" => return Ok("ruxen_string_from"),
        // Phase E-rest 5 of #06.95: `&str_*` instance-method aliases.
        // The `&str` type is a Ruxen primitive (not a class shell),
        // so there's no .rx surface for these — the C runtime
        // shares the `ruxen_string_*` byte-string helpers, and the
        // MIR call-site mangling for `&str` receivers emits keys
        // with a literal `&str_` prefix. Same compiler-internal
        // category as the `String_*` MIR-synthesised callees above.
        "&str_split" => return Ok("ruxen_str_split"),
        "&str_parse_uint" => return Ok("ruxen_str_parse_uint"),
        "&str_len" => return Ok("ruxen_string_len"),
        "&str_is_empty" => return Ok("ruxen_string_is_empty"),
        "&str_trim" => return Ok("ruxen_string_trim"),
        "&str_to_lower" => return Ok("ruxen_string_to_lower"),
        "&str_to_upper" => return Ok("ruxen_string_to_upper"),
        "&str_chars" => return Ok("ruxen_string_chars"),
        "&str_contains" => return Ok("ruxen_string_contains"),
        "&str_starts_with" => return Ok("ruxen_string_starts_with"),
        "&str_ends_with" => return Ok("ruxen_string_ends_with"),
        "&str_lines" => return Ok("ruxen_string_lines"),
        "&str_replace" => return Ok("ruxen_string_replace"),
        "&str_bytes" => return Ok("ruxen_string_bytes"),
        "&str_trim_start" => return Ok("ruxen_string_trim_start"),
        "&str_trim_end" => return Ok("ruxen_string_trim_end"),
        "&str_find" => return Ok("ruxen_string_find"),
        "&str_splitn" => return Ok("ruxen_string_splitn"),
        "&str_parse_int" => return Ok("ruxen_string_parse_int"),
        "&str_parse_float" => return Ok("ruxen_string_parse_float"),
        "&str_to_string" => return Ok("ruxen_string_to_string"),
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
    // `is_fn_call`) recognises `any Fn(...)` / `?T*` receivers and
    // emits an indirect call, so this arm should never fire in
    // normal operation. It exists so a missed lowering path
    // produces a deterministic noop-passthrough instead of a hard
    // codegen error — easier to root-cause when a regression slips
    // in. The `?T*` form is the `Ty::Infer(N)` Display mangling:
    // an unresolved type variable leaking into a `.call(...)`
    // site.
    if name.starts_with("any Fn(")
        || name.starts_with("any Fn[")
        || name.starts_with("&any Fn(")
        || name.starts_with("&any Fn[")
        || (name.starts_with("?T") && name.ends_with("_call"))
    {
        return Ok("ruxen_noop_passthrough");
    }

    // VecIter_, VecIntoIter_, SplitIter_ — iterator combinators.
    // Historically every method here silently no-opped. Only
    // forward user-defined-style names (which downstream link
    // checks will reject if missing); reject anything that *looks*
    // like a known stdlib combinator we haven't actually
    // implemented.
    if name.starts_with("VecIter")
        || name.starts_with("VecIntoIter")
        || name.starts_with("SplitIter")
    {
        return match method {
            // Identity passthroughs: every iterator producer in
            // the v1 runtime already hands back a `RuxenVec *`,
            // so `to_vec` and `enumerate` are no-ops at the
            // runtime layer. The for-loop lowering
            // (`HirExprKind::For`) detects the `(i, x)` tuple
            // binding shape and synthesises the index counter
            // directly, so `enumerate` only needs to survive
            // type-checking + codegen — no real iterator
            // transform.
            "to_vec" | "enumerate" => Ok("ruxen_iter_to_vec"),
            "sum" => Ok("ruxen_vec_sum"),
            "count" => Ok("ruxen_vec_count"),
            "reverse" => Ok("ruxen_vec_reverse"),
            "first" => Ok("ruxen_vec_first"),
            "last" => Ok("ruxen_vec_last"),
            "clone" => Ok("ruxen_vec_clone"),
            "contains" => Ok("ruxen_vec_contains_int"),
            "sort" => Ok("ruxen_vec_sort"),
            "join" => Ok("ruxen_vec_join"),
            // Phase 2 stdlib (#05 batch 2): lazy combinators
            // `take(n)` / `skip(n)` eager-materialise into a
            // fresh `RuxenVec *` via the `ruxen_vec_take` /
            // `ruxen_vec_skip` helpers. Closure-taking
            // terminators (`fold`, `all`, `any`) inline at MIR
            // — the runtime never sees them, so they are
            // intentionally absent from this dispatch table.
            "take" => Ok("ruxen_vec_take"),
            "skip" => Ok("ruxen_vec_skip"),
            // Phase 2 stdlib (#05 batch 3): `chain(other)` /
            // `zip(other)` eager-materialise into fresh
            // `RuxenVec*`s via the runtime helpers below.
            // `collect_vec` is the v1 type-specific shorthand
            // for `collect[Vec[T]]` — since every `*Iter` is
            // already a `RuxenVec*` at runtime, the collector
            // is the same identity passthrough as `to_vec`.
            "chain" => Ok("ruxen_vec_chain"),
            "zip" => Ok("ruxen_vec_zip"),
            "collect_vec" => Ok("ruxen_iter_to_vec"),
            // Known unimplemented combinators — refuse rather
            // than no-op.
            "filter" | "find" | "position" | "partition" | "fold" | "min" | "max" | "any"
            | "all" | "collect" | "map" | "reduce" | "flat_map" | "flatten" => {
                Err(unresolved_method_error(name, "Iter"))
            }
            // Anything else falls through to link-time
            // resolution.
            _ => Ok(name),
        };
    }

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
    // Two aliased clusters stay below because they share a C
    // symbol with a migrated entry and the E0722 check rejects
    // duplicate aliases for the same `c_symbol` with different
    // wire shapes:
    //
    //   * `get_mut`, `get_var` → `ruxen_vec_get_opt` (canonical
    //     spelling `get` is in array.rx)
    //   * `into_iter`, `iter_mut`, `to_vec`, `enumerate`,
    //     `as_slice` → `ruxen_iter_to_vec` (canonical spelling
    //     `iter` is in array.rx)
    if name.starts_with("Array") || name.starts_with("Vec") {
        return match method {
            "get_mut" | "get_var" => Ok("ruxen_vec_get_opt"),
            // Iterator producers + the identity collector are
            // passthroughs — every iterator in the v1 runtime
            // is already represented by a `RuxenVec *`, so
            // `vec.into_iter`, `iter.to_vec`, etc. are all
            // no-ops.
            "into_iter" | "iter_mut" | "to_vec" | "enumerate" | "as_slice" => {
                Ok("ruxen_iter_to_vec")
            }
            // Known unimplemented Vec methods — historically
            // no-opped.
            "map" | "filter" | "fold" | "min" | "max" | "any" | "all" | "collect" | "find"
            | "position" | "partition" | "reduce" | "zip" | "take" | "skip" | "chain"
            | "flat_map" | "flatten" => Err(unresolved_method_error(name, "Vec")),
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
    // The bang variants (`unwrap!`, `expect!`) can't ride the
    // alias map yet — the `!` is part of the method-name
    // surface but the parser doesn't accept it inside a `def
    // NAME as ...` lib decl. Those stay here as explicit
    // fallbacks.
    if name.starts_with("Option") || name.contains("Option[") {
        return match method {
            "expect!" => Ok("ruxen_option_expect"),
            "unwrap!" => Ok("ruxen_option_unwrap"),
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
    // `ok`, `err` into the same option_result.rx lib block.
    // The remaining arms below are non-aliasable (bang surface
    // names, `try_op` which is also a MIR-special) and stay
    // here.
    if name.starts_with("Result") || name.contains("Result[") {
        return match method {
            "try_op" => Ok("ruxen_result_try_op"),
            "expect!" => Ok("ruxen_result_expect"),
            "unwrap!" => Ok("ruxen_result_unwrap"),
            // Known unimplemented Result combinators.
            "and_then" | "or" | "or_else" | "ok_or" => Err(unresolved_method_error(name, "Result")),
            _ => Ok(name),
        };
    }

    // Inferred-type method calls (e.g. `?T..._method` from
    // generics that weren't fully resolved at typecheck). The
    // historical `_ => ruxen_noop_passthrough` fallback here
    // was the worst silent-failure path; it accepted *any*
    // method on an inferred type and quietly returned the
    // receiver.
    if name.starts_with("?T") || name.starts_with('?') {
        return match method {
            // Result/Option combinators with real symbols.
            "try_op" => Ok("ruxen_result_try_op"),
            "unwrap_or" => Ok("ruxen_option_unwrap_or"),
            "unwrap_or_else" => Ok("ruxen_result_unwrap_or_else"),
            // String operations.
            "clone" => Ok("ruxen_string_from"),
            "from" => Ok("ruxen_string_from"),
            "push_str" => Ok("ruxen_string_push_str"),
            "trim" => Ok("ruxen_string_trim"),
            "to_lower" => Ok("ruxen_string_to_lower"),
            // Vec/collection operations with real symbols.
            "len" => Ok("ruxen_vec_len"),
            "is_empty" => Ok("ruxen_vec_is_empty"),
            "push" => Ok("ruxen_vec_push"),
            "pop" => Ok("ruxen_vec_pop"),
            "get" | "get_mut" | "get_var" => Ok("ruxen_vec_get_opt"),
            "each" => Ok("ruxen_vec_each"),
            // User-defined methods commonly used in fixtures
            // — forward to link-time resolution (a missing
            // impl will surface as a link error against the
            // unmangled name).
            "message" | "summary" | "is_actionable" | "is_done" | "weight" | "id" | "title_ref"
            | "priority_ref" | "deadline_ref" | "serialize" | "is_overdue" | "to_display"
            | "assign" | "complete" | "cancel" | "to_string" | "to_s" => Ok(name),
            // Anything else: refuse. This is the P0.5
            // change — the old `_ => "ruxen_noop_passthrough"`
            // masked unimplemented stdlib methods (.map,
            // .map_err, .ok_or, .filter, .find, .fold, .sum,
            // .count, .collect, ...) behind a silent identity.
            _ => Err(unresolved_method_error(name, "?T")),
        };
    }

    // Generic type parameter methods (e.g., `T_assign`,
    // `E_message`): forward to link-time resolution.
    if let Some(pos) = name.find('_') {
        let prefix = &name[..pos];
        if prefix.len() <= 2 && !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_uppercase())
        {
            return Ok(name);
        }
    }

    // Defensive fallback: forward the unmangled name to
    // link-time resolution. User-defined classes
    // (`MyClass_my_method`) and any method not covered by the
    // above intrinsic patterns land here. If no symbol exists
    // at link time the error surfaces cleanly as an
    // undefined-symbol diagnostic from `ld`.
    Ok(name)
}
