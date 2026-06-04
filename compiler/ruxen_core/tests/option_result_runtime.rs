//! Integration harness: compile + run release-e2e Option/Result fixtures
//! and assert the stdout matches the expected output.  Used while
//! debugging generic-enum runtime issues.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR points at crates/ruxen-core
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run(name: &str) -> (bool, String, Option<i32>) {
    let root = workspace_root();
    let src_path = root.join(format!("tests/release-e2e/cases/{}.rx", name));
    let expected_path = root.join(format!("tests/release-e2e/expected/{}.out", name));
    let bin_path = root.join(format!("tmp/{}-{}-{}.bin", name, std::process::id(), ruxen_unique_id()));
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
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    if !errors.is_empty() {
        return (false, format!("typecheck errors: {:?}", errors), None);
    }

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = match lowerer.lower_program(&result.program) {
        Ok(m) => m,
        Err(e) => return (false, format!("MIR lowering failed: {}", e), None),
    };

    if let Err(e) = codegen::compile(&mir, bin_path.to_str().unwrap()) {
        return (false, format!("codegen: {}", e), None);
    }

    let output = match Command::new(&bin_path).output() {
        Ok(o) => o,
        Err(e) => return (false, format!("run: {}", e), None),
    };
    let _ = std::fs::remove_file(&bin_path);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let exit = output.status.code();
    let pass = output.status.success() && stdout == expected;
    if !pass {
        return (
            false,
            format!(
                "exit={:?} stdout=[{}] expected=[{}] stderr=[{}]",
                exit,
                stdout.escape_debug(),
                expected.escape_debug(),
                String::from_utf8_lossy(&output.stderr).escape_debug()
            ),
            exit,
        );
    }
    (true, String::new(), exit)
}

fn check(name: &str) {
    let (ok, msg, _) = compile_and_run(name);
    assert!(ok, "{}: {}", name, msg);
}

#[test]
fn e2e_18_enums_simple() {
    check("18_enums_simple");
}

#[test]
fn e2e_19_enums_data() {
    check("19_enums_data");
}

#[test]
fn e2e_23_option() {
    check("23_option");
}

#[test]
fn e2e_24_result() {
    check("24_result");
}

#[test]
fn e2e_25_question_op() {
    check("25_question_op");
}

#[test]
fn e2e_56_if_let_some() {
    check("56_if_let_some");
}

#[test]
fn e2e_73_enum_generic() {
    check("73_enum_generic");
}

#[test]
fn e2e_97_expect_ok() {
    check("97_expect_ok");
}

#[test]
fn e2e_98_unwrap_or() {
    check("98_unwrap_or");
}

#[test]
fn e2e_99_map_option() {
    check("99_map_option");
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
