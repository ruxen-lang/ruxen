//! Phase 2 #06.C2: `@[derive(Debug)]` on an enum synthesizes a
//! `{EnumName}_to_debug` method that interpolation dispatches to,
//! mirroring the struct path from #06.C.
//!
//! Output shape (matches Rust's `Debug`):
//!   * Unit variant   → `"Variant"`
//!   * Tuple variant  → `"Variant(a, b, ...)"`
//!   * Struct variant → `"Variant { name: a, ... }"`

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
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

fn compile_and_run(rx_path: PathBuf, out_basename: &str, expect_fn: &str) -> Run {
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
    assert!(
        errors.is_empty(),
        "type errors in {:?}: {:?}",
        rx_path,
        errors
    );

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering failed");

    let has_synthesized_fn = mir.functions.iter().any(|f| f.name == expect_fn);

    let out_dir = std::env::temp_dir();
    let out_path = out_dir.join(format!("{}-{}-{}", out_basename, std::process::id(), ruxen_unique_id()));
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
    let rx = root.join("tests/release-e2e/cases/200_implicit_debug_enum_unit.rx");
    let run = compile_and_run(rx, "ruxen_derive_debug_enum_unit_bin", "Color_to_debug");

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
    let rx = root.join("tests/release-e2e/cases/212_implicit_debug_enum_explicit_q.rx");
    let run = compile_and_run(
        rx,
        "ruxen_derive_debug_enum_explicit_q_bin",
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
    let rx = root.join("tests/release-e2e/cases/210_implicit_debug_enum_tuple.rx");
    let run = compile_and_run(rx, "ruxen_derive_debug_enum_tuple_bin", "Shape_to_debug");

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

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
