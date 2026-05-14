//! Phase 2 #06.C2: `@[derive(Debug)]` on an enum synthesizes a
//! `{EnumName}_to_debug` method that interpolation dispatches to,
//! mirroring the struct path from #06.C.
//!
//! Output shape (matches Rust's `Debug`):
//!   * Unit variant   → `"Variant"`
//!   * Tuple variant  → `"Variant(a, b, ...)"`
//!   * Struct variant → `"Variant { name: a, ... }"`

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::path::PathBuf;
use std::process::Command;

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

struct Run {
    stdout: String,
    stderr: String,
    code: Option<i32>,
    has_synthesized_fn: bool,
}

fn compile_and_run(rvn_path: PathBuf, out_basename: &str, expect_fn: &str) -> Run {
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
    assert!(
        errors.is_empty(),
        "type errors in {:?}: {:?}",
        rvn_path,
        errors
    );

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");

    let has_synthesized_fn = mir.functions.iter().any(|f| f.name == expect_fn);

    let out_dir = std::env::temp_dir();
    let out_path = out_dir.join(out_basename);
    let out_str = out_path.to_string_lossy().to_string();
    codegen::compile(&mir, &out_str).expect("codegen failed");

    let output = Command::new(&out_str)
        .output()
        .expect("failed to run compiled binary");

    let _ = std::fs::remove_file(&out_str);
    let _ = std::fs::remove_file(format!("{}.o", out_str));

    Run {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        code: output.status.code(),
        has_synthesized_fn,
    }
}

#[test]
fn enum_with_derive_debug_unit_variant_prints_name() {
    let root = workspace_root();
    let rvn = root.join("tests/release-e2e/cases/200_derive_debug_enum_unit.rvn");
    let run = compile_and_run(rvn, "riven_derive_debug_enum_unit_bin", "Color_to_debug");

    assert!(
        run.has_synthesized_fn,
        "MIR should contain synthesized Color_to_debug"
    );
    assert_eq!(
        run.stdout.trim(),
        "Green",
        "stdout mismatch (exit={:?}, stderr={:?})",
        run.code,
        run.stderr
    );
}

/// `"#{e:?}"` should also route through `{EnumName}_to_debug`. Phase
/// 2 #06.C typecheck already accepts `:?` on derive-Debug types; this
/// guards that the C2 enum path is reached when the spec is explicit.
#[test]
fn enum_with_derive_debug_explicit_debug_spec_dispatches() {
    let root = workspace_root();
    let rvn = root.join("tests/release-e2e/cases/212_derive_debug_enum_explicit_q.rvn");
    let run = compile_and_run(
        rvn,
        "riven_derive_debug_enum_explicit_q_bin",
        "Color_to_debug",
    );

    assert!(
        run.has_synthesized_fn,
        "MIR should contain synthesized Color_to_debug"
    );
    assert_eq!(
        run.stdout.trim(),
        "Blue",
        "stdout mismatch (exit={:?}, stderr={:?})",
        run.code,
        run.stderr
    );
}

/// Variants declared as `Variant(name: Type, ...)` are classified by
/// the parser as `Struct` variants (named fields). The Debug output
/// mirrors Rust's named-field rendering: `"Variant { name: value }"`.
#[test]
fn enum_with_derive_debug_named_field_variant_prints_braces() {
    let root = workspace_root();
    let rvn = root.join("tests/release-e2e/cases/210_derive_debug_enum_tuple.rvn");
    let run = compile_and_run(rvn, "riven_derive_debug_enum_tuple_bin", "Shape_to_debug");

    assert!(
        run.has_synthesized_fn,
        "MIR should contain synthesized Shape_to_debug"
    );
    assert_eq!(
        run.stdout.trim(),
        "Circle { radius: 1.5 }\nRect { w: 3, h: 4 }",
        "stdout mismatch (exit={:?}, stderr={:?})",
        run.code,
        run.stderr
    );
}
