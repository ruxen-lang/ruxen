//! MIR span-population tests for DWARF line info (P0.14).
//!
//! Proves the lowering half of the debug-info pipeline: HIR spans are threaded
//! onto `MirFunction::def_span` (the function's definition line) and
//! `BasicBlock::inst_spans` (per-instruction source lines). This runs in the
//! default (Cranelift) test suite; it inspects the MIR directly and never
//! reaches the LLVM backend, so it is independent of that backend's unrelated
//! `DataAddr` gap in this worktree. The codegen half (MIR spans to emitted
//! DWARF) is covered by `tests/llvm_debug_info.rs`.

use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// `add` is defined on source line 1 and its body `a + b` is on line 2.
const SOURCE: &str = "\
def add(a: Int, b: Int) -> Int
  a + b
end

def main
  let result = add(10, 20)
  puts \"#{result}\"
end
";

#[test]
fn lowering_records_function_and_instruction_spans() {
    let mut lexer = Lexer::new(SOURCE);
    let tokens = lexer.tokenize().expect("lex failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse failed");
    let type_result = typeck::type_check(&program);
    assert!(
        !type_result
            .diagnostics
            .iter()
            .any(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error),
        "unexpected type errors: {:?}",
        type_result.diagnostics
    );
    let mut lowerer = Lowerer::new(&type_result.symbols);
    let mir = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering failed");

    let add = mir
        .functions
        .iter()
        .find(|f| f.name == "add")
        .expect("`add` function should be present in the lowered MIR");

    // The function's definition line is recorded for its DISubprogram.
    let def_span = add
        .def_span
        .as_ref()
        .expect("`add` should carry a def_span");
    assert_eq!(def_span.line, 1, "`add` is defined on source line 1");

    // At least one instruction in `add` carries the body line (`a + b`, line 2).
    let body_lines: Vec<u32> = add
        .blocks
        .iter()
        .flat_map(|b| {
            (0..b.instructions.len()).filter_map(|i| b.span_at(i).map(|s| s.line))
        })
        .collect();
    assert!(
        body_lines.contains(&2),
        "expected an instruction span at source line 2 (`a + b`); got {body_lines:?}"
    );

    // Synthesized functions (derives, drop glue) must NOT carry a def_span, so
    // the debug emitter skips them rather than mis-attributing a source line.
    // (Only a sanity check; a user program may lower zero synthesized fns.)
    for f in &mir.functions {
        if f.name.contains("_fmt") || f.name.ends_with("_drop_noop") {
            assert!(
                f.def_span.is_none(),
                "synthesized function `{}` should have no def_span",
                f.name
            );
        }
    }
}
