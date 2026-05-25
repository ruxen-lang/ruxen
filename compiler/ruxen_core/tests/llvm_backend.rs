//! LLVM backend comparison tests.
//!
//! Compiles each test fixture through both the Cranelift and LLVM backends
//! and asserts that they produce identical output.

#![cfg(feature = "llvm")]

use std::process::Command;

use ruxen_core::codegen::{self, Backend};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::{borrow_check, typeck};

/// Compile source code to a temp executable using the given backend.
fn compile_to_exe(source: &str, backend: Backend) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let output_path = format!(
        "{}/ruxen_test_{}_{}.exe",
        std::env::temp_dir().display(),
        std::process::id(),
        id
    );

    // Compile pipeline
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse failed");
    let type_result = typeck::type_check(&program);
    assert!(
        !type_result
            .diagnostics
            .iter()
            .any(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error),
        "type errors"
    );
    let borrow_errors = borrow_check::borrow_check(&type_result.program, &type_result.symbols);
    assert!(borrow_errors.is_empty(), "borrow errors");
    let mut lowerer = ruxen_core::mir::lower::Lowerer::new(&type_result.symbols);
    let mir_program = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering failed");
    codegen::compile_with_options(&mir_program, &output_path, false, &[], backend)
        .expect("codegen failed");

    output_path
}

/// Run an executable and capture its stdout.
fn run_exe(path: &str) -> (String, i32) {
    let output = Command::new(path)
        .output()
        .unwrap_or_else(|e| panic!("Failed to run {}: {}", path, e));
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let exit_code = output.status.code().unwrap_or(-1);
    // Clean up
    let _ = std::fs::remove_file(path);
    (stdout, exit_code)
}

/// Compile and run through both backends, assert identical output.
fn assert_backends_identical(fixture_path: &str) {
    let source = std::fs::read_to_string(fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", fixture_path, e));

    let cl_exe = compile_to_exe(&source, Backend::Cranelift);
    let llvm_exe = compile_to_exe(&source, Backend::Llvm { opt_level: 0 });

    let (cl_out, cl_exit) = run_exe(&cl_exe);
    let (llvm_out, llvm_exit) = run_exe(&llvm_exe);

    assert_eq!(
        cl_out, llvm_out,
        "Output differs for {}\nCranelift:\n{}\nLLVM:\n{}",
        fixture_path, cl_out, llvm_out
    );
    assert_eq!(
        cl_exit, llvm_exit,
        "Exit code differs for {} (Cranelift={}, LLVM={})",
        fixture_path, cl_exit, llvm_exit
    );
}

macro_rules! backend_test {
    ($name:ident, $fixture:expr) => {
        #[test]
        fn $name() {
            assert_backends_identical(concat!("tests/fixtures/", $fixture));
        }
    };
}

backend_test!(hello_identical, "hello.rx");
backend_test!(arithmetic_identical, "arithmetic.rx");
backend_test!(control_flow_identical, "control_flow.rx");
backend_test!(functions_identical, "functions.rx");
backend_test!(enums_identical, "enums.rx");
backend_test!(enum_data_identical, "enum_data.rx");
backend_test!(classes_identical, "classes.rx");
backend_test!(simple_class_identical, "simple_class.rx");
backend_test!(class_methods_identical, "class_methods.rx");
backend_test!(string_interp_identical, "string_interp.rx");
backend_test!(mini_sample_identical, "mini_sample.rx");
backend_test!(tasklist_identical, "tasklist.rx");
backend_test!(sample_program_identical, "sample_program.rx");
backend_test!(drop_owned_struct_identical, "drop_owned_struct.rx");

/// Test that all optimization levels produce correct output.
#[test]
fn all_opt_levels_correct() {
    let source = std::fs::read_to_string("tests/fixtures/arithmetic.rx").unwrap();

    let cl_exe = compile_to_exe(&source, Backend::Cranelift);
    let (baseline, _) = run_exe(&cl_exe);

    for opt in [0u8, 1, 2, 3] {
        let llvm_exe = compile_to_exe(&source, Backend::Llvm { opt_level: opt });
        let (result, exit) = run_exe(&llvm_exe);
        assert_eq!(baseline, result, "Output mismatch at -O{}", opt);
        assert_eq!(exit, 0, "Non-zero exit at -O{}", opt);
    }
}

/// Test that LLVM IR verification passes for all fixtures.
#[test]
fn llvm_ir_verifies_all_fixtures() {
    for fixture in std::fs::read_dir("tests/fixtures").unwrap() {
        let path = fixture.unwrap().path();
        if path.extension().map_or(false, |e| e == "rx") {
            let source = std::fs::read_to_string(&path).unwrap();
            // If compile_to_exe succeeds, verification passed (it's called inside)
            let exe = compile_to_exe(&source, Backend::Llvm { opt_level: 0 });
            let _ = std::fs::remove_file(&exe);
        }
    }
}
