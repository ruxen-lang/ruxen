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
    let _ = (
        emit_binop,
        coerce_value,
        coerce_value_signed,
        is_string_typed_value,
    );
}

// ── Task 4: the env / M-carrying helpers must also be reachable ───────────────
//
// build_signature / translate_instruction / translate_terminator carry
// <M: Module>; def_local / use_local / gen_value / coerce_call_args take no
// env and so are plain pub. The REPL JIT backend instantiates the M-carrying
// ones at M = JITModule. This test asserts all seven are pub + re-exported
// (instantiated here at M = ObjectModule). A regression to pub(super) breaks
// the cross-crate share and fails to compile here.
#[test]
fn shared_m_helpers_are_public() {
    use ruxen_core::codegen::cranelift::{
        build_signature, coerce_call_args, def_local, gen_value, translate_instruction,
        translate_terminator, use_local, TranslationEnv,
    };

    // M-carrying: instantiable at a concrete M.
    let _ = build_signature::<cranelift_object::ObjectModule>;
    let _ = translate_instruction::<cranelift_object::ObjectModule>;
    let _ = translate_terminator::<cranelift_object::ObjectModule>;
    // Zero-M: plain pub fns.
    let _ = (def_local, use_local, gen_value, coerce_call_args);
    // The env struct itself must be pub (jit.rs constructs it directly).
    fn _assert_env_is_nameable<'a, M: cranelift_module::Module>(_e: &TranslationEnv<'a, M>) {}
}
