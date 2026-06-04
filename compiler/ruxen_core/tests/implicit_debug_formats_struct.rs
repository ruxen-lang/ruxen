//! Integration test: `@[derive(Debug)]` on a struct synthesizes a
//! `to_debug` method that interpolation dispatches to, so that
//! `puts "#{p}"` prints `Point { x: 1, y: 2 }` rather than a raw
//! pointer address.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
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

fn compile_and_run(rx_path: PathBuf, out_basename: &str) -> (String, String, Option<i32>) {
    let source = std::fs::read_to_string(&rx_path)
        .unwrap_or_else(|e| panic!("read {} failed: {}", rx_path.display(), e));

    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");
    let result = typeck::type_check(&program);

    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
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
    let out_path = out_dir.join(format!(
        "{}-{}-{}",
        out_basename,
        std::process::id(),
        ruxen_unique_id()
    ));
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
    let rx = root.join("tests/release-e2e/cases/139_implicit_debug_format.rx");
    let (stdout, stderr, code) = compile_and_run(rx, "ruxen_derive_debug_test_bin");

    assert_eq!(
        stdout.trim(),
        "Point { x: 1, y: 2 }",
        "stdout mismatch (exit={:?}, stderr={:?})",
        code,
        stderr
    );
}

/// Characterization (Phase 6 Task 1): a struct whose field is a
/// `derive Debug` enum must render that field via `{Enum}_to_debug`,
/// not the `<...>` placeholder. The old inline format ladder in
/// `synthesize_struct_to_debug` omitted the enum arm that
/// `format_field_for_debug` carries; routing the struct path through
/// the helper fixes the `<...>` rendering for nested enum fields.
#[test]
fn struct_with_nested_derive_debug_enum_field_renders_via_enum_helper() {
    let root = workspace_root();
    let rx = root.join("tests/release-e2e/cases/213_implicit_debug_struct_enum_field.rx");
    let source = std::fs::read_to_string(&rx)
        .unwrap_or_else(|e| panic!("read {} failed: {}", rx.display(), e));

    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");
    let result = typeck::type_check(&program);

    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "type errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");

    let out_dir = std::env::temp_dir();
    let out_path = out_dir.join("ruxen_derive_debug_struct_enum_field_bin");
    let out_str = out_path.to_string_lossy().to_string();
    codegen::compile(&mir, &out_str).expect("codegen failed");

    let output = Command::new(&out_str)
        .output()
        .expect("failed to run compiled binary");
    let _ = std::fs::remove_file(&out_str);
    let _ = std::fs::remove_file(format!("{}.o", out_str));

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert_eq!(
        stdout.trim(),
        "Tag { id: 7, color: Green }",
        "nested enum field should render via Color_to_debug, not <...> \
         (exit={:?}, stderr={:?})",
        output.status.code(),
        stderr
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
