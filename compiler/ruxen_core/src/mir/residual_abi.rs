//! The compiler-emitted runtime ABI residual, shared by both backends.
//!
//! The C ABI of every `ruxen_*` symbol a stdlib `.rx` `lib` block declares
//! is DERIVED from that declaration (`abi::FnAbi::of_ffi`). This table holds
//! only the symbols codegen or MIR lowering emits on its own and that at least one
//! bootstrap (the wasm subset is the smallest) does not declare in a `.rx`
//! `lib` block, so there is no declared `Ty` to derive from:
//! allocation, `==`/`<=>`/`**`/hash lowering, B73 keyed constructors, index
//! and pointer-indirection helpers, drop glue, panic, the formatter-synthesis
//! surface, the hosted `main` prologue and regex-literal compilation.
//!
//! `abi::CalleeIndex` reads this one table for every backend. Adding a
//! stdlib runtime function never touches it: declare the function in its
//! package's `.rx` and `.c` instead. The guard
//! `tests/it/runtime_abi_derivation.rs::residual_abi_table_holds_only_undeclared_emitted_symbols`
//! fails when an entry is declared by `.rx` or no longer emitted.

/// One compiler-emitted runtime function: its C symbol and signature.
#[derive(Debug)]
pub struct ResidualFn {
    /// The linked C symbol.
    pub name: &'static str,
    /// Parameter slots, in order.
    pub params: &'static [AbiClass],
    /// Return slot; `None` for a `void` function.
    pub ret: Option<AbiClass>,
    /// How the function takes each of `params` (the loan check reads it
    /// as it reads a `.rx` declaration's `FfiFuncDecl::param_modes`).
    pub modes: &'static [ParamMode],
}

use super::abi::AbiClass;
use super::nodes::ParamMode;
use AbiClass::{Ptr, F64, I32, I64};
use ParamMode::{Move as V, Mut as M, Read as R};

const fn f(
    name: &'static str,
    params: &'static [AbiClass],
    ret: Option<AbiClass>,
    modes: &'static [ParamMode],
) -> ResidualFn {
    ResidualFn {
        name,
        params,
        ret,
        modes,
    }
}

/// Every runtime symbol the compiler emits that no `.rx` lib block declares
/// on every target.
pub const RESIDUAL_RUNTIME_FNS: &[ResidualFn] = &[
    // Hosted `main` prologue (`mir/lower/function.rs`).
    f("ruxen_env_init", &[I32, Ptr], None, &[R, R]),
    // `a ** b` and `a <=> b` lowering (`mir/lower/expr/binops.rs`).
    f("ruxen_int_pow", &[I64, I64], Some(I64), &[R, R]),
    f("ruxen_float_pow", &[F64, F64], Some(F64), &[R, R]),
    f("ruxen_int_cmp", &[I64, I64], Some(I64), &[R, R]),
    f("ruxen_float_cmp", &[F64, F64], Some(I64), &[R, R]),
    // Formatter synthesis surface (`codegen/lang_intrinsics.rs`).
    f(
        "ruxen_float_to_string_prec",
        &[F64, I64],
        Some(Ptr),
        &[R, R],
    ),
    f(
        "ruxen_string_truncate_chars",
        &[Ptr, I64],
        Some(Ptr),
        &[R, R],
    ),
    f(
        "ruxen_fmt_formatter_new_with_spec",
        &[I64, I64, I64, I64],
        Some(Ptr),
        &[R, R, R, R],
    ),
    f("ruxen_fmt_formatter_precision", &[Ptr], Some(I64), &[R]),
    // `==` on strings, derive ordering, `collect`, and `h[k]`. (Derive
    // hashing's `ruxen_string_hash` is `String#hash_code`'s `.rx` decl.)
    // `ruxen_string_eq` is `.rx`-declared only by `std.async_fs`, which the
    // wasm bootstrap omits, so it stays here.
    f("ruxen_string_eq", &[Ptr, Ptr], Some(I64), &[R, R]),
    f("ruxen_string_cmp", &[Ptr, Ptr], Some(I64), &[R, R]),
    f("ruxen_string_from_iter", &[Ptr], Some(Ptr), &[M]),
    f("ruxen_hash_index", &[Ptr, I64], Some(I64), &[R, R]),
    // `a[i]` / `a[i] = v`, inlined closures and `collect`.
    f("ruxen_vec_get", &[Ptr, I64], Some(I64), &[R, R]),
    f("ruxen_vec_get_or_panic", &[Ptr, I64], Some(I64), &[R, R]),
    f("ruxen_vec_set", &[Ptr, I64, I64], None, &[M, R, V]),
    // A `&var T` argument naming an Array element (`mir/lower/expr/refmut_place.rs`).
    f("ruxen_vec_slot_addr", &[Ptr, I64], Some(Ptr), &[R, R]),
    f("ruxen_vec_from_iter", &[Ptr], Some(Ptr), &[M]),
    // Allocation and panic.
    f("ruxen_alloc", &[I64], Some(Ptr), &[R]),
    f("ruxen_dealloc", &[Ptr], None, &[V]),
    f("ruxen_panic", &[Ptr], None, &[R]),
    // Drop glue chosen by the MIR drop pass (`mir/lower/drops/`).
    // The trailing I64s are element descriptors (`RUXEN_ELEM_*`).
    f("ruxen_string_free", &[Ptr], None, &[V]),
    f("ruxen_vec_drop", &[Ptr, I64], None, &[V, R]),
    f("ruxen_hash_drop", &[Ptr, I64, I64], None, &[V, R, R]),
    f("ruxen_set_drop", &[Ptr, I64], None, &[V, R]),
    // Regex literals (`mir/lower/expr/mod.rs`).
    f("ruxen_regex_compile_const", &[Ptr, Ptr], Some(Ptr), &[R, R]),
];
