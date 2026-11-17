//! The batch `CodeGen` and the REPL JIT are one driver
//! (`ruxen_core::codegen::cranelift::CodeGen<M>`, F-29 P2), so their CLIF
//! is identical by construction. What stays checked here: the integer
//! fixture's CLIF structure (`clif_for_test`), the JIT running it, and
//! string `+` reaching codegen as a call.
//!
//! Lives in `ruxen_repl` (not `ruxen_core/tests/`) because the JIT half needs
//! the `ruxen_*` C runtime (`libruxenrt.a`) force-loaded into the test binary,
//! which only `ruxen_repl`'s build script provides.

use ruxen_core::hir::types::Ty;
use ruxen_core::mir::abi::CalleeIndex;
use ruxen_core::mir::nodes::{
    BasicBlock, Literal, LocalKind, MirFunction, MirInst, MirLocal, MirProgram, MirValue,
    Terminator,
};
use ruxen_core::parser::ast::BinOp;

/// A program of the one function `func`, with its callee index.
fn program_of(func: MirFunction) -> MirProgram {
    let mut program = MirProgram::new();
    program.functions.push(func);
    program.callees = CalleeIndex::for_program(&program);
    program
}

/// Hand-built MIR for `def add3(a: Int, b: Int) -> Int = (a + b) + 3`.
///
/// locals: 0=a, 1=b, 2=_t0 (a+b), 3=_t1 (_t0 + 3); params = [0, 1].
/// One block: BinOp(2, Add, a, b); BinOp(3, Add, _t0, 3); Return(_t1).
fn fixture_add3() -> MirFunction {
    let locals = vec![
        MirLocal {
            id: 0,
            name: "a".into(),
            ty: Ty::Int,
            mutable: false,
            kind: LocalKind::User,
        },
        MirLocal {
            id: 1,
            name: "b".into(),
            ty: Ty::Int,
            mutable: false,
            kind: LocalKind::User,
        },
        MirLocal {
            id: 2,
            name: "_t0".into(),
            ty: Ty::Int,
            mutable: false,
            kind: LocalKind::User,
        },
        MirLocal {
            id: 3,
            name: "_t1".into(),
            ty: Ty::Int,
            mutable: false,
            kind: LocalKind::User,
        },
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

    MirFunction::with_parts(
        "add3".to_string(),
        vec![0, 1],
        Ty::Int,
        locals,
        vec![block],
        0,
    )
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
        MirLocal {
            id: 0,
            name: "a".into(),
            ty: Ty::String,
            mutable: false,
            kind: LocalKind::User,
        },
        MirLocal {
            id: 1,
            name: "b".into(),
            ty: Ty::String,
            mutable: false,
            kind: LocalKind::User,
        },
        MirLocal {
            id: 2,
            name: "_t0".into(),
            ty: Ty::String,
            mutable: false,
            kind: LocalKind::User,
        },
    ];

    let mut block = BasicBlock::new(0);
    block.instructions.push(MirInst::Call {
        dest: Some(2),
        callee: "ruxen_string_concat".to_string(),
        args: vec![MirValue::Use(0), MirValue::Use(1)],
    });
    block.terminator = Terminator::Return(Some(MirValue::Use(2)));

    MirFunction::with_parts(
        "cat".to_string(),
        vec![0, 1],
        Ty::String,
        locals,
        vec![block],
        0,
    )
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
fn integer_fixture_lowers_and_runs() {
    let program = program_of(fixture_add3());

    // Pin the structure so a drift in the shared lowering trips: exactly
    // two iadd, no call.
    let clif = ruxen_core::codegen::cranelift::clif_for_test(&program, "add3").expect("CLIF");
    assert_eq!(
        clif.matches("iadd").count(),
        2,
        "add3 should lower to exactly 2 iadd, got CLIF:\n{clif}"
    );

    // The JIT runs it.
    let mut jit = crate::jit::JITCodeGen::new().expect("create JIT");
    jit.compile_program(&program).expect("JIT-compile add3");
    let code_ptr = jit.function_ptr("add3").expect("add3 address");
    // SAFETY: `code_ptr` is the finalized entry of `add3`, an
    // `fn(Int, Int) -> Int`; `jit` owns the code and outlives the call.
    let add3: extern "C" fn(i64, i64) -> i64 = unsafe { std::mem::transmute(code_ptr) };
    assert_eq!(add3(7, 5), 15, "JIT add3(7,5) should be 15");
}

/// IR-level proof that string `+` is a `Call`, not an inlined add: the dead
/// REPL-JIT string-concat inline (deleted in the share) had nothing to inline,
/// because the MIR lowerer (`mir/lower/expr/binops.rs:60`) rewrites
/// `String + String` to a `Call ruxen_string_concat` BEFORE codegen.
#[test]
fn string_concat_is_a_call_not_an_iadd() {
    let program = program_of(fixture_cat());
    let batch_clif =
        ruxen_core::codegen::cranelift::clif_for_test(&program, "cat").expect("batch CLIF");

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
