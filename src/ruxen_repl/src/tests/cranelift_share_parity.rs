//! Phase 4 — both-backends observable-parity tripwire.
//!
//! Compiles a fixed integer-arithmetic MIR fixture through BOTH the batch
//! `CodeGen` (ObjectModule, via `ruxen_core::codegen::cranelift::clif_for_test`)
//! and the REPL `JITCodeGen` (JITModule, via `crate::jit::clif_for_test`),
//! asserting the two backends emit BYTE-IDENTICAL CLIF — the same observable on
//! both sides, not "batch IR vs JIT runtime number". A behavioural cross-check
//! (`run_int_fn_for_test`) confirms the JIT also executes correctly. This is the
//! re-fork tripwire: it must stay GREEN through the share, proving the share
//! changed nothing in the emitted IR. If a future edit re-diverges the two
//! backends' integer lowering, the CLIF equality assertion fails.
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

/// Post-lowering MIR for `def cat(a: String, b: String) -> String = a + b`.
///
/// This is the EXACT shape `mir/lower/expr/binops.rs:60` produces for a
/// `String + String`: the `+` is rewritten to a `Call ruxen_string_concat`
/// BEFORE codegen, so a string `+` never reaches the backend as a
/// `MirInst::BinOp`. The fixture therefore contains a `Call`, not a `BinOp::Add`
/// — pinning the old REPL JIT's `BinOp::Add` string-concat inline (deleted in
/// the share) as dead at the IR level: there is no `iadd` to inline over.
///
/// locals: 0=a, 1=b, 2=_t0 (concat result); params = [0, 1].
fn fixture_cat() -> MirFunction {
    let locals = vec![
        MirLocal { id: 0, name: "a".into(), ty: Ty::String, mutable: false },
        MirLocal { id: 1, name: "b".into(), ty: Ty::String, mutable: false },
        MirLocal { id: 2, name: "_t0".into(), ty: Ty::String, mutable: false },
    ];

    let mut block = BasicBlock::new(0);
    block.instructions.push(MirInst::Call {
        dest: Some(2),
        callee: "ruxen_string_concat".to_string(),
        args: vec![MirValue::Use(0), MirValue::Use(1)],
    });
    block.terminator = Terminator::Return(Some(MirValue::Use(2)));

    MirFunction::with_parts("cat".to_string(), vec![0, 1], Ty::String, locals, vec![block], 0)
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
    // JIT path: compile through JITCodeGen(JITModule) and read back the CLIF
    // (same translate-into-ctx → display() seam as the batch side).
    let jit_clif = crate::jit::clif_for_test(&mir).expect("jit CLIF");

    // Structural parity: the two backends must emit byte-identical IR for the
    // same MIR. This is the real re-fork tripwire — comparing the SAME
    // observable (CLIF text) on both sides, not "batch IR vs JIT runtime".
    assert_eq!(
        jit_clif, batch_clif,
        "JIT and batch backends must emit identical CLIF for add3.\n\
         === BATCH ===\n{batch_clif}\n=== JIT ===\n{jit_clif}"
    );

    // Pin the expected structure so a future edit that drifts BOTH backends
    // in lockstep (identical-but-wrong) still trips: exactly two iadd, no call.
    assert_eq!(
        batch_clif.matches("iadd").count(),
        2,
        "add3 should lower to exactly 2 iadd, got CLIF:\n{batch_clif}"
    );

    // Behavioural cross-check: the JIT actually runs the fixture correctly.
    let result = crate::jit::run_int_fn_for_test(&mir, &[7, 5]).expect("jit run");
    assert_eq!(result, 15, "JIT add3(7,5) should be 15");
}

/// IR-level proof that string `+` is a `Call`, not an inlined add: the dead
/// REPL-JIT string-concat inline (deleted in the share) had nothing to inline,
/// because the MIR lowerer (`mir/lower/expr/binops.rs:60`) rewrites
/// `String + String` to a `Call ruxen_string_concat` BEFORE codegen.
#[test]
fn string_concat_is_a_call_not_an_iadd() {
    let mir = fixture_cat();
    let batch_clif = ruxen_core::codegen::cranelift::clif_for_test(&mir).expect("batch CLIF");

    // Zero integer adds — a string `+` never reaches codegen as a BinOp::Add,
    // so there is no `iadd` for any inline to have produced.
    assert_eq!(
        batch_clif.matches("iadd").count(),
        0,
        "string concat must emit NO iadd, got CLIF:\n{batch_clif}"
    );
    // Exactly one `call` — the `ruxen_string_concat` invocation the lowerer
    // emits (the symbol resolves to a Cranelift `fn0` ref in CLIF text, but the
    // fixture's MIR callee is literally `ruxen_string_concat`).
    assert_eq!(
        batch_clif.matches("call ").count(),
        1,
        "string concat must emit exactly one call, got CLIF:\n{batch_clif}"
    );
}
