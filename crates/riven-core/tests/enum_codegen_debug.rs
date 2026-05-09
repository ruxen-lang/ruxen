//! Integration tests: compile milestone fixtures to executables and verify output.

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::process::Command;

fn compile_fixture(name: &str) -> String {
    let source = std::fs::read_to_string(format!("tests/fixtures/{}.rvn", name))
        .unwrap_or_else(|e| panic!("failed to read {}.rvn: {}", name, e));

    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");
    let result = typeck::type_check(&program);

    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "type errors in {}: {:?}", name, errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");

    let output_path = format!("tests/fixtures/{}_test", name);
    codegen::compile(&mir, &output_path).expect("codegen failed");
    output_path
}

fn run_fixture(name: &str, expected: &str) {
    let binary = compile_fixture(name);
    let output = Command::new(format!("./{}", binary))
        .output()
        .expect("failed to run binary");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Clean up
    let _ = std::fs::remove_file(&binary);

    assert_eq!(
        stdout.trim(),
        expected,
        "fixture '{}': expected {:?}, got {:?}",
        name,
        expected,
        stdout.trim()
    );
}

/// Milestone 5: enum match expression returns correct string.
#[test]
fn test_enum_output() {
    run_fixture("enums", "green");
}

/// Milestone 6: class definition compiles alongside arithmetic that prints 7.
#[test]
fn test_classes_output() {
    run_fixture("classes", "7");
}

/// Enum data payloads: enum with data fields compiles and produces correct output.
#[test]
fn test_enum_data_output() {
    run_fixture("enum_data", "75\n24");
}

/// TaskList test: class with method calls.
#[test]
fn test_tasklist() {
    let binary = compile_fixture("tasklist");
    let output = Command::new(format!("./{}", binary))
        .output()
        .expect("failed to run binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = std::fs::remove_file(&binary);
    eprintln!("=== STDOUT ===\n{}", stdout);
    eprintln!("=== STDERR ===\n{}", stderr);
    eprintln!("=== EXIT CODE: {:?} ===", output.status.code());
    assert!(
        output.status.success(),
        "tasklist exited with {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout,
        stderr
    );
}

/// Mini sample: stripped down version of sample program.
#[test]
fn test_mini_sample() {
    let binary = compile_fixture("mini_sample");
    let output = Command::new(format!("./{}", binary))
        .output()
        .expect("failed to run binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let _ = std::fs::remove_file(&binary);
    eprintln!("=== STDOUT ===\n{}", stdout);
    eprintln!("=== STDERR ===\n{}", stderr);
    eprintln!("=== EXIT CODE: {:?} ===", output.status.code());
    assert!(
        output.status.success(),
        "mini sample exited with {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout,
        stderr
    );
}

/// String interpolation test.
#[test]
fn test_string_interp() {
    run_fixture("string_interp", "Creating tasks...\nThe answer is 42\nDone");
}

/// Class with methods: class methods and enum matching.
#[test]
fn test_class_methods() {
    run_fixture("class_methods", "Starting...\nCreated task\nhigh priority");
}

/// Simple class: allocates a class and prints.
#[test]
fn test_simple_class() {
    run_fixture("simple_class", "Starting...\nCreated point");
}

/// Full sample program: compiles and runs without crashing.
#[test]
fn test_sample_program_compiles_and_runs() {
    let binary = compile_fixture("sample_program");
    let output = Command::new(format!("./{}", binary))
        .output()
        .expect("failed to run binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // Clean up
    let _ = std::fs::remove_file(&binary);

    // Print output for debugging
    eprintln!("=== STDOUT ===\n{}", stdout);
    eprintln!("=== STDERR ===\n{}", stderr);
    eprintln!("=== EXIT CODE: {:?} ===", output.status.code());

    // The program should exit successfully (code 0).
    assert!(
        output.status.success(),
        "sample program exited with {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout,
        stderr
    );

    // It should produce at least some output (the "Creating tasks..." line).
    assert!(
        stdout.contains("Creating tasks"),
        "Expected 'Creating tasks' in output, got: {}",
        stdout
    );
}
