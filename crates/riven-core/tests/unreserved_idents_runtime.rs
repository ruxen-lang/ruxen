//! Regression test for P0.12 / TEC-13: `actor`, `send`, and `receive`
//! must no longer be reserved keywords. This compiles and runs the
//! `135_unreserved_idents` fixture and asserts the stdout matches the
//! expected output, proving that user code can define a function named
//! `send`, a function named `receive`, and a class named `Mailbox`
//! without the lexer hijacking those identifiers.

use riven_core::codegen;
use riven_core::diagnostics::DiagnosticLevel;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn e2e_135_unreserved_idents() {
    let root = workspace_root();
    let src_path = root.join("tests/release-e2e/cases/135_unreserved_idents.rvn");
    let expected_path = root.join("tests/release-e2e/expected/135_unreserved_idents.out");
    let bin_path = root.join("tmp/135_unreserved_idents.bin");
    let _ = std::fs::create_dir_all(root.join("tmp"));

    let source = std::fs::read_to_string(&src_path)
        .unwrap_or_else(|e| panic!("read {}: {}", src_path.display(), e));
    let expected = std::fs::read_to_string(&expected_path).unwrap_or_default();

    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");

    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(
        output.status.success(),
        "non-zero exit: stdout=[{}] stderr=[{}]",
        stdout.escape_debug(),
        String::from_utf8_lossy(&output.stderr).escape_debug()
    );
    assert_eq!(
        stdout,
        expected,
        "stdout mismatch: got=[{}] expected=[{}]",
        stdout.escape_debug(),
        expected.escape_debug()
    );
}
