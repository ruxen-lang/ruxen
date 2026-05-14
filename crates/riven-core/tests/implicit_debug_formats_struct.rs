//! Integration test: `@[derive(Debug)]` on a struct synthesizes a
//! `to_debug` method that interpolation dispatches to, so that
//! `puts "#{p}"` prints `Point { x: 1, y: 2 }` rather than a raw
//! pointer address.

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::path::PathBuf;
use std::process::Command;

/// Resolve the workspace root by walking up from the crate manifest dir
/// until we find the directory that contains both the `tests/` and
/// `crates/` subtrees.
fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if p.join("tests/release-e2e/cases").is_dir() {
            return p;
        }
        if !p.pop() {
            panic!(
                "unable to locate workspace root from {}",
                env!("CARGO_MANIFEST_DIR")
            );
        }
    }
}

fn compile_and_run(rvn_path: PathBuf, out_basename: &str) -> (String, String, Option<i32>) {
    let source = std::fs::read_to_string(&rvn_path)
        .unwrap_or_else(|e| panic!("read {} failed: {}", rvn_path.display(), e));

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
    assert!(errors.is_empty(), "type errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");

    // Sanity check: the synthesized formatter must be in the program.
    assert!(
        mir.functions.iter().any(|f| f.name == "Point_to_debug"),
        "MIR should contain synthesized Point_to_debug; got: {:?}",
        mir.functions.iter().map(|f| &f.name).collect::<Vec<_>>()
    );

    let out_dir = std::env::temp_dir();
    let out_path = out_dir.join(out_basename);
    let out_str = out_path.to_string_lossy().to_string();
    codegen::compile(&mir, &out_str).expect("codegen failed");

    let output = Command::new(&out_str)
        .output()
        .expect("failed to run compiled binary");

    let _ = std::fs::remove_file(&out_str);
    let _ = std::fs::remove_file(format!("{}.o", out_str));

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.code())
}

#[test]
fn struct_with_derive_debug_prints_named_fields() {
    let root = workspace_root();
    let rvn = root.join("tests/release-e2e/cases/139_implicit_debug_format.rvn");
    let (stdout, stderr, code) = compile_and_run(rvn, "riven_derive_debug_test_bin");

    assert_eq!(
        stdout.trim(),
        "Point { x: 1, y: 2 }",
        "stdout mismatch (exit={:?}, stderr={:?})",
        code,
        stderr
    );
}
