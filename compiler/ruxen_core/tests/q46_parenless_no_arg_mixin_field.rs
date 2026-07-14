//! Q46 regression: a runtime-dispatch class constructed with paren-less
//! zero-argument `.new` must initialize its class-info header before moving
//! into an `any Mixin` field. The pre-fix binary dereferenced a null header
//! during dynamic dispatch and segfaulted with empty stdout.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

#[test]
fn parenless_no_arg_runtime_mixin_value_survives_field_move() {
    let root = workspace_root();
    let source = std::fs::read_to_string(
        root.join("tests/release-e2e/cases/460_parenless_no_arg_init_mixin_field.rx"),
    )
    .expect("read Q46 release-e2e case");
    let expected = std::fs::read_to_string(
        root.join("tests/release-e2e/expected/460_parenless_no_arg_init_mixin_field.out"),
    )
    .expect("read Q46 expected stdout");

    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lex Q46 fixture");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse Q46 fixture");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "Q46 typecheck errors: {errors:?}");

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("lower Q46 MIR");
    let bin_path = root
        .join("tmp")
        .join(format!("q46-{}.bin", std::process::id()));
    std::fs::create_dir_all(bin_path.parent().unwrap()).expect("create tmp directory");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("compile Q46 fixture");

    let output = Command::new(&bin_path).output().expect("run Q46 fixture");
    let _ = std::fs::remove_file(&bin_path);
    assert!(
        output.status.success(),
        "Q46 binary crashed: status={:?}, stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}
