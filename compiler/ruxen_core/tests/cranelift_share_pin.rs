//! Phase 4 — Cranelift-share pin tests.
//!
//! These tests pin the *public surface* of the shared Cranelift core and the
//! *observable parity* of the two backends (batch ObjectModule + REPL
//! JITModule). They are the re-fork tripwire: if a helper regresses to
//! `pub(super)`, or if a backend's lowering drifts, these fail.

// ── Task 2: module-agnostic helpers must be reachable from outside the crate ──
//
// The REPL JIT backend (a separate crate) depends on these. If any goes back
// to pub(super), this fails to compile — re-fork prevention at the type level.
// These 8 helpers take only &mut FunctionBuilder (or pure values) and never
// touch the module, so they are shareable by visibility alone.
#[test]
fn shared_helpers_are_public() {
    use ruxen_core::codegen::cranelift::{
        cmpop_to_floatcc, cmpop_to_intcc, coerce_value, coerce_value_signed, emit_binop,
        is_string_typed_value, simple_type_size, ty_to_cranelift,
    };
    use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
    use cranelift_codegen::ir::types::Type;
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
