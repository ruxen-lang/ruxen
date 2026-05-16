//! Regression test: `derive PartialEq` on a struct must generate an `eq()`
//! method (and `==` codegen) that returns `true` for structurally equal
//! values and `false` otherwise — not pointer equality, not always-false.
//!
//! Surfaced during Tier-1 e2e probing (see CEO ruling 2026-04-24).

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::process::Command;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run(source: &str, name: &str) -> (String, Option<i32>) {
    let root = workspace_root();
    let bin_path = root.join(format!("tmp/{}.bin", name));
    let _ = std::fs::create_dir_all(root.join("tmp"));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen failed");

    let output = Command::new(&bin_path).output().expect("run binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    (stdout, output.status.code())
}

#[test]
fn derive_partial_eq_compares_fields_struct() {
    let source = rvn("derive_partial_eq_compares_fields_struct");
    let (stdout, exit) = compile_and_run(&source, "derive_partial_eq_basic");
    assert_eq!(exit, Some(0), "non-zero exit; stdout={}", stdout);
    assert_eq!(
        stdout, "ab=true\nac=false\n",
        "structurally equal P.new(1,2) instances should compare equal; \
         differing fields should compare unequal"
    );
}
