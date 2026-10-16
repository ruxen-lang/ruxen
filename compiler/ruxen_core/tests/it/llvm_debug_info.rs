//! DWARF/debug line-info tests for the LLVM backend (P0.14).
//!
//! Proves that a debug (`opt_level == 0`) LLVM build emits DWARF line info:
//! a compile unit, a `DISubprogram` per function at its source line, and a
//! `DILocation` per instruction carrying the source line recorded on the MIR
//! (`MirFunction::def_span` / `BasicBlock::inst_spans`). Also proves the gate:
//! an optimized (`opt_level == 2`) build emits none of it.
//!
//! We assert on the module's textual LLVM IR (via `CodeGen::emit_llvm_ir`),
//! the hermetic equivalent of running `llvm-dwarfdump` on the linked object,
//! no linker or debugger needed.
//!
//! The MIR is hand-built rather than lowered from source: in this worktree the
//! LLVM backend cannot yet lower the `DataAddr` class-info sections that the
//! (bootstrap-merged) async stdlib synthesizes, so *any* front-end program
//! trips that pre-existing gap before reaching debug-info emission. Building a
//! self-contained function keeps this test focused on the DWARF output and
//! independent of that unrelated backend limitation. Lowering-side span
//! population is covered by `tests/mir_debug_spans.rs`.

use ruxen_core::codegen::llvm::CodeGen;
use ruxen_core::hir::types::Ty;
use ruxen_core::lexer::token::Span;
use ruxen_core::mir::nodes::{
    BasicBlock, Literal, LocalKind, MirFunction, MirInst, MirLocal, MirProgram, MirValue,
    Terminator,
};
use ruxen_core::parser::ast::BinOp;

fn int_local(id: u32, name: &str) -> MirLocal {
    MirLocal {
        id,
        name: name.to_string(),
        ty: Ty::Int,
        mutable: false,
        kind: LocalKind::User,
    }
}

/// Build a two-function program equivalent to:
///
/// ```text
/// (line  1) def add(a: Int, b: Int) -> Int
/// (line  2)   seven() + a + b
/// (line  3) end
///
/// (line 10) def seven -> Int
/// (line 11)   7
/// (line 12) end
/// ```
///
/// `add`'s definition is on source line 1 and its body (including a *call* to
/// `seven`) is on line 2. The call instruction is deliberate: LLVM's verifier
/// requires every call in a function that has debug info to carry a `!dbg`
/// location, so a passing `module.verify()` proves calls are covered too.
fn add_program() -> MirProgram {
    // seven() -> Int { 7 }, defined on line 10.
    let mut seven_block = BasicBlock::new(0);
    seven_block.terminator = Terminator::Return(Some(MirValue::Literal(Literal::Int(7))));
    let mut seven = MirFunction::with_parts(
        "seven".to_string(),
        vec![],
        Ty::Int,
        vec![],
        vec![seven_block],
        0,
    );
    seven.def_span = Some(Span::new(0, 0, 10, 1));

    // add(a, b) -> Int { c = seven(); s = a + b; c + s }, body on line 2.
    let body_span = Some(Span::new(0, 0, 2, 3));
    let mut add_block = BasicBlock::new(0);
    // c = seven()  — a Call, so the "call needs !dbg" verifier rule is exercised.
    add_block.push_inst(
        MirInst::Call {
            dest: Some(2),
            callee: "seven".to_string(),
            args: vec![],
        },
        body_span.clone(),
    );
    // s = a + b
    add_block.push_inst(
        MirInst::BinOp {
            dest: 3,
            op: BinOp::Add,
            lhs: MirValue::Use(0),
            rhs: MirValue::Use(1),
        },
        body_span.clone(),
    );
    // sum = c + s
    add_block.push_inst(
        MirInst::BinOp {
            dest: 4,
            op: BinOp::Add,
            lhs: MirValue::Use(2),
            rhs: MirValue::Use(3),
        },
        body_span,
    );
    add_block.terminator = Terminator::Return(Some(MirValue::Use(4)));
    let mut add = MirFunction::with_parts(
        "add".to_string(),
        vec![0, 1],
        Ty::Int,
        vec![
            int_local(0, "a"),
            int_local(1, "b"),
            int_local(2, "c"),
            int_local(3, "s"),
            int_local(4, "sum"),
        ],
        vec![add_block],
        0,
    );
    // `add` is defined on source line 1.
    add.def_span = Some(Span::new(0, 0, 1, 1));

    MirProgram {
        functions: vec![add, seven],
        entry: None,
        ffi_libs: vec![],
        vtables: vec![],
        class_infos: vec![],
        wasm_exports: vec![],
    }
}

#[test]
fn debug_build_emits_dwarf_line_info() {
    let program = add_program();
    let codegen = CodeGen::new(0).expect("CodeGen::new failed");
    let ir = codegen.emit_llvm_ir(&program).expect("emit_llvm_ir failed");

    // A compile unit and the module flag that stops LLVM stripping debug info.
    assert!(
        ir.contains("!llvm.dbg.cu"),
        "expected a debug compile unit (!llvm.dbg.cu) in:\n{ir}"
    );
    assert!(
        ir.contains("\"Debug Info Version\""),
        "expected the \"Debug Info Version\" module flag in:\n{ir}"
    );

    // A function scope (DISubprogram) for `add`, at its definition line 1.
    assert!(
        ir.contains("!DISubprogram("),
        "expected at least one !DISubprogram in:\n{ir}"
    );
    assert!(
        ir.contains("name: \"add\""),
        "expected a DISubprogram named \"add\" in:\n{ir}"
    );
    assert!(
        ir.contains("name: \"seven\""),
        "expected a DISubprogram named \"seven\" in:\n{ir}"
    );

    // Per-instruction source locations. The body instructions are recorded at
    // source line 2, so a DILocation at line 2 proves the span-to-DWARF path.
    // Because `add` calls `seven`, a passing `emit_llvm_ir` (which verifies the
    // module) also proves the call instruction carries a `!dbg` location.
    assert!(
        ir.contains("!DILocation("),
        "expected at least one !DILocation in:\n{ir}"
    );
    assert!(
        ir.contains("!DILocation(line: 2,"),
        "expected a DILocation at source line 2 (`add` body) in:\n{ir}"
    );
}

#[test]
fn release_build_emits_no_debug_info() {
    // The gate: an optimized build (opt_level != 0) must emit no debug info.
    let program = add_program();
    let codegen = CodeGen::new(2).expect("CodeGen::new failed");
    let ir = codegen.emit_llvm_ir(&program).expect("emit_llvm_ir failed");
    assert!(
        !ir.contains("!DILocation("),
        "optimized build must not emit DILocation:\n{ir}"
    );
    assert!(
        !ir.contains("!DISubprogram("),
        "optimized build must not emit DISubprogram:\n{ir}"
    );
    assert!(
        !ir.contains("!llvm.dbg.cu"),
        "optimized build must not emit a debug compile unit:\n{ir}"
    );
}
