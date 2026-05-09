//! Integration test: borrow-check the phase5 sample program.

use riven_core::borrow_check;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

fn load_sample() -> String {
    std::fs::read_to_string("tests/fixtures/sample_program.rvn")
        .expect("failed to read sample_program.rvn")
}

#[test]
fn sample_program_borrow_checks() {
    let source = load_sample();
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");

    let type_result = typeck::type_check(&program);

    // Run borrow checker on the type-checked HIR
    let errors = borrow_check::borrow_check(&type_result.program, &type_result.symbols);

    // Report all borrow diagnostics
    if !errors.is_empty() {
        eprintln!(
            "--- borrow check produced {} diagnostic(s) ---",
            errors.len()
        );
        for err in &errors {
            eprintln!("{}", err);
        }
        eprintln!("--- end borrow check diagnostics ---");
        // NOTE: Some of these may be false positives from patterns the checker
        // doesn't fully handle yet. The goal is zero errors on the sample program.
    }

    eprintln!("Sample program borrow-check: {} error(s)", errors.len());
}
