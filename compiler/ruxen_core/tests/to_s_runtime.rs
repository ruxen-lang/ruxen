//! End-to-end pins for the universal `to_s` method (REQ4).
//!
//! Part A: scalar primitives (`Int`, `Float`, `Bool`, `Char`, `String`)
//! stringify via `to_s`, reusing the existing `ruxen_*_to_string` runtime
//! helpers. Part B (user-defined class/struct/enum `to_s` routed through
//! the Display dispatch) is pinned separately once its lowering lands.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}-{}-{}.bin", basename, std::process::id(), ruxen_unique_id()));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

#[test]
fn int_to_s_stringifies() {
    let source = "def main\n  let n: Int = 42\n  puts n.to_s()\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "to_s_int");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(stdout, "42\n", "got: {:?}", stdout);
}

#[test]
fn bool_to_s_stringifies() {
    let source = "def main\n  let b: Bool = true\n  puts b.to_s()\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "to_s_bool");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(stdout, "true\n", "got: {:?}", stdout);
}

#[test]
fn float_to_s_stringifies() {
    let source = "def main\n  let f: Float = 1.5\n  puts f.to_s()\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "to_s_float");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(stdout, "1.5\n", "got: {:?}", stdout);
}

// ── Part B: user-defined types ──────────────────────────────────────

/// A struct's `to_s()` returns the same string as `"#{s}"` (the derived
/// Debug representation), since `to_s` routes through the identical
/// display dispatch.
#[test]
fn struct_to_s_matches_interpolation() {
    let source = "struct P\n  x: Int\n  y: Int\nend\n\ndef main\n  let p = P.new(1, 2)\n  puts p.to_s()\n  puts \"#{p}\"\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "to_s_struct");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(
        stdout, "P { x: 1, y: 2 }\nP { x: 1, y: 2 }\n",
        "got: {:?}",
        stdout
    );
}

/// An enum's `to_s()` matches its `"#{e}"` form too.
#[test]
fn enum_to_s_matches_interpolation() {
    let source = "enum Shape\n  Circle(radius: Float)\nend\n\ndef main\n  let c = Shape.Circle(radius: 1.5)\n  puts c.to_s()\n  puts \"#{c}\"\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "to_s_enum");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(
        stdout, "Circle { radius: 1.5 }\nCircle { radius: 1.5 }\n",
        "got: {:?}",
        stdout
    );
}

/// Pins the exact stdout of the `810_to_s_universal` e2e fixture.
#[test]
fn to_s_universal_fixture() {
    let source = concat!(
        "struct Point\n  x: Int\n  y: Int\nend\n\n",
        "enum Shape\n  Circle(radius: Float)\nend\n\n",
        "class Widget\n  id: Int\n  def init(@id: Int)\n  end\n",
        "  def to_s -> String\n    \"widget##{self.id}\"\n  end\nend\n\n",
        "def main\n",
        "  let n: Int = 42\n  puts n.to_s()\n",
        "  let f: Float = 1.5\n  puts f.to_s()\n",
        "  let b: Bool = true\n  puts b.to_s()\n",
        "  let p = Point.new(3, 4)\n  puts p.to_s()\n",
        "  let c = Shape.Circle(radius: 2.5)\n  puts c.to_s()\n",
        "  let w = Widget.new(7)\n  puts w.to_s()\n",
        "end\n",
    );
    let (stdout, stderr, ok) = compile_and_run(source, "to_s_universal_fixture");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(
        stdout, "42\n1.5\ntrue\nPoint { x: 3, y: 4 }\nCircle { radius: 2.5 }\nwidget#7\n",
        "got: {:?}",
        stdout
    );
}

/// A user-defined `to_s` method wins over the synthesized default.
#[test]
fn user_defined_to_s_wins() {
    let source = "class Widget\n  id: Int\n  def init(@id: Int)\n  end\n  def to_s -> String\n    \"custom-widget\"\n  end\nend\n\ndef main\n  let w = Widget.new(1)\n  puts w.to_s()\nend\n";
    let (stdout, stderr, ok) = compile_and_run(source, "to_s_user_override");
    assert!(ok, "stderr: {}", stderr);
    assert_eq!(stdout, "custom-widget\n", "got: {:?}", stdout);
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
