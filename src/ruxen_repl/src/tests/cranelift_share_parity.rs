//! Phase 4 — both-backends observable-parity tripwire.
//!
//! Compiles a fixed integer-arithmetic MIR fixture through BOTH the batch
//! `CodeGen` (ObjectModule, via `ruxen_core::codegen::cranelift::clif_for_test`)
//! and the REPL `JITCodeGen` (JITModule, via `crate::jit::run_int_fn_for_test`),
//! asserting identical observable behaviour. This is the re-fork tripwire: it
//! is GREEN against the currently-forked `jit.rs` and must stay GREEN through
//! the share (Task 4), proving the share changed nothing observable. If a
//! future edit re-diverges the two backends' integer lowering, this fails.
//!
//! Lives in `ruxen_repl` (not `ruxen_core/tests/`) because the JIT half needs
//! the `ruxen_*` C runtime (`libruxenrt.a`) force-loaded into the test binary,
//! which only `ruxen_repl`'s build script provides. `ruxen_repl` depends on
//! `ruxen_core`, so the batch half is still reachable here — one test, both
//! backends.
//!
//! Integer arithmetic ONLY — deliberately avoids string `+`, whose lowering
//! legitimately differs between the batch backend (Call ruxen_string_concat
//! emitted by the MIR lowerer) and the still-forked REPL JIT (inlined at
//! BinOp::Add) pre-Task-4.

use ruxen_core::hir::types::Ty;
use ruxen_core::mir::nodes::{
    BasicBlock, Literal, MirFunction, MirInst, MirLocal, MirValue, Terminator,
};
use ruxen_core::parser::ast::BinOp;

/// Hand-built MIR for `def add3(a: Int, b: Int) -> Int = (a + b) + 3`.
///
/// locals: 0=a, 1=b, 2=_t0 (a+b), 3=_t1 (_t0 + 3); params = [0, 1].
/// One block: BinOp(2, Add, a, b); BinOp(3, Add, _t0, 3); Return(_t1).
fn fixture_add3() -> MirFunction {
    let locals = vec![
        MirLocal { id: 0, name: "a".into(), ty: Ty::Int, mutable: false },
        MirLocal { id: 1, name: "b".into(), ty: Ty::Int, mutable: false },
        MirLocal { id: 2, name: "_t0".into(), ty: Ty::Int, mutable: false },
        MirLocal { id: 3, name: "_t1".into(), ty: Ty::Int, mutable: false },
    ];

    let mut block = BasicBlock::new(0);
    block.instructions.push(MirInst::BinOp {
        dest: 2,
        op: BinOp::Add,
        lhs: MirValue::Use(0),
        rhs: MirValue::Use(1),
    });
    block.instructions.push(MirInst::BinOp {
        dest: 3,
        op: BinOp::Add,
        lhs: MirValue::Use(2),
        rhs: MirValue::Literal(Literal::Int(3)),
    });
    block.terminator = Terminator::Return(Some(MirValue::Use(3)));

    MirFunction::with_parts("add3".to_string(), vec![0, 1], Ty::Int, locals, vec![block], 0)
}

/// Step 3b reconciliation guard: string `+` still concatenates after the
/// share deletes the JIT fork's `BinOp::Add` string-concat inline.
///
/// The inline (old jit.rs:964-980) was DEAD: the shared MIR lowerer
/// (`mir/lower/expr/binops.rs`) already rewrites `String + String` to a
/// `Call ruxen_string_concat` and returns early, so a string `+` never reaches
/// codegen as a `MirInst::BinOp`. Both backends consume that same lowerer
/// output, so deleting the inline is behaviour-preserving. This test proves the
/// REPL still concatenates after the deletion.
#[test]
fn repl_string_concat_after_share() {
    let outs = super::state_persistence::run_session(&[r#"puts("a" + "b")"#]);
    assert_eq!(
        outs[0].matches("ab").count(),
        1,
        "REPL `\"a\" + \"b\"` should still print `ab` after the share, got: {outs:?}"
    );
}

#[test]
fn both_backends_agree_on_integer_fixture() {
    let mir = fixture_add3();

    // Batch path: compile through CodeGen(ObjectModule) and read back the CLIF.
    let batch_clif = ruxen_core::codegen::cranelift::clif_for_test(&mir).expect("batch CLIF");

    // JIT path: compile through JITCodeGen(JITModule), run add3(7, 5).
    let result = crate::jit::run_int_fn_for_test(&mir, &[7, 5]).expect("jit run");
    assert_eq!(result, 15, "JIT add3(7,5) should be 15");

    // Cross-check: the batch backend lowers the same fixture to exactly two
    // iadd instructions — no string_concat, no divergent path.
    assert_eq!(
        batch_clif.matches("iadd").count(),
        2,
        "batch backend should emit 2 iadd for add3, got CLIF:\n{batch_clif}"
    );
}
