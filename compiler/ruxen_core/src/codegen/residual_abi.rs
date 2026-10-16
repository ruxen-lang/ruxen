//! The compiler-emitted runtime ABI residual, shared by both backends.
//!
//! The C ABI of every `ruxen_*` symbol a stdlib `.rx` `lib` block declares
//! is DERIVED from that declaration (Cranelift Pass-0 via `ty_to_cranelift`,
//! LLVM `declare_ffi_function` via `ty_to_llvm`). This table holds only the
//! symbols codegen or MIR lowering emits on its own and that at least one
//! bootstrap (the wasm subset is the smallest) does not declare in a `.rx`
//! `lib` block, so there is no declared `Ty` to derive from:
//! allocation, `==`/`<=>`/`**`/hash lowering, B73 keyed constructors, index
//! and pointer-indirection helpers, drop glue, panic, the formatter-synthesis
//! surface, the hosted `main` prologue and regex-literal compilation.
//!
//! `cranelift/runtime_sigs.rs` and `llvm/runtime_decl.rs` both read this one
//! table. Adding a stdlib runtime function never touches it: declare the
//! function in its package's `.rx` and `.c` instead. The guard
//! `tests/it/runtime_abi_derivation.rs::residual_abi_table_holds_only_undeclared_emitted_symbols`
//! fails when an entry is declared by `.rx` or no longer emitted.

/// A residual parameter or return slot, before a backend picks its machine
/// type. `Ptr` and `I64` are both 64 bits on Cranelift's native targets but
/// differ on wasm32, where LLVM lowers `Ptr` to a 32-bit pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbiTy {
    /// C `int8_t` (a Ruxen `Bool` result).
    I8,
    /// C `int`.
    I32,
    /// C `int64_t`, including erased uniform-word slots.
    I64,
    /// C `double`.
    F64,
    /// Any C pointer.
    Ptr,
}

/// One compiler-emitted runtime function: its C symbol and signature.
#[derive(Debug)]
pub struct ResidualFn {
    /// The linked C symbol.
    pub name: &'static str,
    /// Parameter slots, in order.
    pub params: &'static [AbiTy],
    /// Return slot; `None` for a `void` function.
    pub ret: Option<AbiTy>,
}

use AbiTy::{Ptr, F64, I32, I64};

const fn f(name: &'static str, params: &'static [AbiTy], ret: Option<AbiTy>) -> ResidualFn {
    ResidualFn { name, params, ret }
}

/// Every runtime symbol the compiler emits that no `.rx` lib block declares
/// on every target.
pub const RESIDUAL_RUNTIME_FNS: &[ResidualFn] = &[
    // Hosted `main` prologue (`mir/lower/function.rs`).
    f("ruxen_env_init", &[I32, Ptr], None),
    // `a ** b` and `a <=> b` lowering (`mir/lower/expr/binops.rs`).
    f("ruxen_int_pow", &[I64, I64], Some(I64)),
    f("ruxen_float_pow", &[F64, F64], Some(F64)),
    f("ruxen_int_cmp", &[I64, I64], Some(I64)),
    f("ruxen_float_cmp", &[F64, F64], Some(I64)),
    // Formatter synthesis surface (`codegen/lang_intrinsics.rs`).
    f("ruxen_float_to_string_prec", &[F64, I64], Some(Ptr)),
    f("ruxen_string_truncate_chars", &[Ptr, I64], Some(Ptr)),
    f(
        "ruxen_fmt_formatter_new_with_spec",
        &[I64, I64, I64, I64],
        Some(Ptr),
    ),
    f("ruxen_fmt_formatter_precision", &[Ptr], Some(I64)),
    // `==` on strings, derive ordering, `collect`, and `h[k]`. (Derive
    // hashing's `ruxen_string_hash` is `String#hash_code`'s `.rx` decl.)
    // `ruxen_string_eq` is `.rx`-declared only by `std.async_fs`, which the
    // wasm bootstrap omits, so it stays here.
    f("ruxen_string_eq", &[Ptr, Ptr], Some(I64)),
    f("ruxen_string_cmp", &[Ptr, Ptr], Some(I64)),
    f("ruxen_string_from_iter", &[Ptr], Some(Ptr)),
    f("ruxen_hash_index", &[Ptr, I64], Some(I64)),
    // B73 key-kind protocol: MIR rewrites `ruxen_hash_new` and friends to
    // these and appends `(kind, hash_fn, eq_fn)` (`mir/lower/key_desc.rs`).
    f("ruxen_hash_new_keyed", &[I64, I64, I64], Some(Ptr)),
    f(
        "ruxen_hash_with_capacity_keyed",
        &[I64, I64, I64, I64],
        Some(Ptr),
    ),
    f(
        "ruxen_hash_from_iter_keyed",
        &[Ptr, I64, I64, I64],
        Some(Ptr),
    ),
    f("ruxen_set_new_keyed", &[I64, I64, I64], Some(Ptr)),
    f(
        "ruxen_set_with_capacity_keyed",
        &[I64, I64, I64, I64],
        Some(Ptr),
    ),
    f(
        "ruxen_set_from_iter_keyed",
        &[Ptr, I64, I64, I64],
        Some(Ptr),
    ),
    f("ruxen_hash_ensure_key_kind", &[Ptr, I64, I64, I64], None),
    f("ruxen_set_ensure_key_kind", &[Ptr, I64, I64, I64], None),
    // `a[i]` / `a[i] = v`, inlined closures and `collect`.
    f("ruxen_vec_get", &[Ptr, I64], Some(I64)),
    f("ruxen_vec_set", &[Ptr, I64, I64], None),
    f("ruxen_vec_from_iter", &[Ptr], Some(Ptr)),
    // Allocation and panic.
    f("ruxen_alloc", &[I64], Some(Ptr)),
    f("ruxen_dealloc", &[Ptr], None),
    f("ruxen_panic", &[Ptr], None),
    // Drop glue chosen by the MIR drop pass (`mir/lower/drops.rs`).
    // The trailing I64s are element descriptors (`RUXEN_ELEM_*`).
    f("ruxen_string_free", &[Ptr], None),
    f("ruxen_vec_drop", &[Ptr, I64], None),
    f("ruxen_hash_drop", &[Ptr, I64, I64], None),
    f("ruxen_set_drop", &[Ptr, I64], None),
    // Regex literals (`mir/lower/expr/mod.rs`).
    f("ruxen_regex_compile_const", &[Ptr, Ptr], Some(Ptr)),
];

/// The residual signature of `name`, or `None` when the compiler does not
/// emit it on its own (a `.rx`-declared symbol takes its derived ABI).
pub fn residual_fn(name: &str) -> Option<&'static ResidualFn> {
    RESIDUAL_RUNTIME_FNS.iter().find(|r| r.name == name)
}
