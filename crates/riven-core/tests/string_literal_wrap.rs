//! Regression guard for ROADMAP P0.7 — string literals must be wrapped
//! at MIR lowering via an implicit `riven_string_from` call so that any
//! later `String::drop` -> `free()` cannot double-free a pointer into
//! `.rodata`. The wrap lives in `mir/lower.rs::emit_owned_string_literal`.

use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::mir::nodes::MirInst;
use riven_core::parser::Parser;
use riven_core::typeck;

/// Lowering `let s = "hello"` must emit a `Call { callee: "riven_string_from", ... }`
/// so the local holds an owned heap-allocated `String`, not a pointer into `.rodata`.
#[test]
fn string_literal_lowers_through_string_from_wrapper() {
    let source = r#"
def main
  let s = "hello"
  puts s
end
"#;

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    assert!(
        result
            .diagnostics
            .iter()
            .all(|d| d.level != riven_core::diagnostics::DiagnosticLevel::Error),
        "unexpected typeck errors: {:?}",
        result.diagnostics
    );

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer.lower_program(&result.program).expect("lower");

    let main_fn = mir
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main fn in MIR");

    let mut saw_literal = false;
    let mut saw_string_from = false;

    for block in &main_fn.blocks {
        for inst in &block.instructions {
            match inst {
                MirInst::StringLiteral { value, .. } if value == "hello" => {
                    saw_literal = true;
                }
                MirInst::Call { callee, .. } if callee == "riven_string_from" => {
                    saw_string_from = true;
                }
                _ => {}
            }
        }
    }

    assert!(
        saw_literal,
        "expected MIR to contain a StringLiteral instruction for \"hello\""
    );
    assert!(
        saw_string_from,
        "P0.7 regression: string literal was NOT wrapped in riven_string_from. \
         Without the wrap, String::drop -> free() on the literal pointer would double-free."
    );
}
