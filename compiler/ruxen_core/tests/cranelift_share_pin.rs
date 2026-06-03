//! Phase 4 — Cranelift-share public-surface pin tests.
//!
//! These tests pin the *public surface* of the shared Cranelift core: the
//! helpers the REPL JIT backend depends on must stay `pub` + re-exported. If
//! any regresses to `pub(super)`, this fails to compile — re-fork prevention
//! at the type level.
//!
//! The *both-backends observable-parity* tripwire (the integer-fixture test
//! that drives BOTH the batch ObjectModule path and the REPL JITModule path)
//! lives in `src/ruxen_repl/src/tests/cranelift_share_parity.rs`, NOT here.
//! Rationale: the JIT half needs the `ruxen_*` C runtime (`libruxenrt.a`)
//! force-loaded into the test binary; only `ruxen_repl`'s build script emits
//! that link directive (`-force_load`), and `cargo:rustc-link-arg` does not
//! propagate to a downstream crate's test binary. `ruxen_repl` already depends
//! on `ruxen_core`, so the parity test reaches `clif_for_test` from there and
//! still asserts cross-backend parity in ONE test.

// ── Task 2: module-agnostic helpers must be reachable from outside the crate ──
//
// The REPL JIT backend (a separate crate) depends on these. If any goes back
// to pub(super), this fails to compile. These 8 helpers take only
// &mut FunctionBuilder (or pure values) and never touch the module, so they
// are shareable by visibility alone.
#[test]
fn shared_helpers_are_public() {
    use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
    use cranelift_codegen::ir::types::Type;
    use ruxen_core::codegen::cranelift::{
        cmpop_to_floatcc, cmpop_to_intcc, coerce_value, coerce_value_signed, emit_binop,
        is_string_typed_value, simple_type_size, ty_to_cranelift,
    };
    use ruxen_core::hir::types::Ty;
    use ruxen_core::mir::nodes::CmpOp;

    // Reference each via an explicit fn-pointer coercion so the import is
    // load-bearing; this is a pure compile-time "these are pub + re-exported"
    // assertion, not runtime logic.
    let _ = ty_to_cranelift as fn(&Ty) -> Option<Type>;
    let _ = cmpop_to_intcc as fn(CmpOp) -> IntCC;
    let _ = cmpop_to_floatcc as fn(CmpOp) -> FloatCC;
    let _ = simple_type_size as fn(&Ty) -> usize;
    // The remaining four take a &mut FunctionBuilder; naming them is enough to
    // assert visibility without spelling the full Cranelift value types.
    let _ = (emit_binop, coerce_value, coerce_value_signed, is_string_typed_value);
}
