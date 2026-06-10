//! Q17 — a generic FREE FUNCTION bound by a mixin must monomorphize per
//! concrete implementor, not devirtualize to the single implementor. Before
//! the fix, calling `def paint_all[T: Paintable](s: &var T, …)` with two
//! different implementors emitted the bound-placeholder callee
//! `T: Paintable_fill_rect`, which link-failed. The single-implementor case was
//! masked because mixin dispatch devirtualized to the sole impl — which is
//! exactly why quiver was capped at one `PaintSurface` implementor.
//!
//! These pins read the SAME fixtures the release-e2e harness runs
//! (`tests/release-e2e/cases/655..658`) so the cargo pin and the e2e case can
//! never drift apart, and they COMPILE + RUN + assert exact stdout (three prior
//! episodes on this branch had pins pass while real codegen was broken). The
//! negative pin compiles an INLINE generic-method-over-mixin shape (NOT yet
//! supported) and asserts a clear lowering error — no bound-placeholder symbol.

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

/// Compile + run; returns (stdout, stderr, success).
fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}-{}.bin", basename, std::process::id()));

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
    assert!(
        errors.is_empty(),
        "typecheck errors for {basename}: {errors:?}"
    );

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .unwrap_or_else(|e| panic!("MIR lowering for {basename}: {e}"));
    codegen::compile(&mir, bin_path.to_str().unwrap())
        .unwrap_or_else(|e| panic!("codegen for {basename}: {e}"));

    let output = Command::new(&bin_path)
        .output()
        .unwrap_or_else(|e| panic!("run {basename}: {e}"));
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// Lower only (no codegen); return the lowering error message if any.
fn lower_error(source: &str) -> Option<String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let mut lowerer = Lowerer::new(&result.symbols);
    lowerer.lower_program(&result.program).err()
}

fn case(name: &str) -> (String, String) {
    let root = workspace_root();
    let src = std::fs::read_to_string(root.join("tests/release-e2e/cases").join(name))
        .unwrap_or_else(|e| panic!("read case {name}: {e}"));
    let out_name = name.replace(".rx", ".out");
    let expected = std::fs::read_to_string(root.join("tests/release-e2e/expected").join(out_name))
        .unwrap_or_else(|e| panic!("read expected for {name}: {e}"));
    (src, expected)
}

/// TWO implementors with DIFFERENT bodies: `dep=20` (4*5) and `mine=9` (4+5).
/// Distinct values prove real per-implementor monomorphization.
#[test]
fn two_implementors_monomorphize_distinctly() {
    let (src, expected) = case("655_generic_fn_mixin_two_implementors.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q17_two_impl");
    assert!(ok, "link/run failed; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// Generic-calling-generic: `render_twice[T] { render(d) + render(d) }` must
/// transitively monomorphize `render` per concrete leaf (`logo=40`, `icon=84`).
#[test]
fn generic_calling_generic_transitively_monomorphizes() {
    let (src, expected) = case("656_generic_fn_calling_generic_fn.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q17_nested");
    assert!(ok, "link/run failed; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// A mixin method with a DEFAULT body dispatched through the generic must
/// resolve `self.<other>` per implementor (`en=1005`, `fr=1010`).
#[test]
fn mixin_default_body_monomorphizes_per_implementor() {
    let (src, expected) = case("657_generic_fn_mixin_default_body.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q17_default_body");
    assert!(ok, "link/run failed; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// Single implementor (quiver's current shape) must still link + run (`area=42`).
#[test]
fn single_implementor_still_works() {
    let (src, expected) = case("658_generic_fn_mixin_single_implementor.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q17_single_impl");
    assert!(ok, "link/run failed; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// NEGATIVE: a generic METHOD over a mixin with ≥2 implementors (a generic
/// `def` INSIDE a class) is NOT yet monomorphized — the staged remainder. It
/// must surface a CLEAR lowering error, never a bound-placeholder symbol that
/// link-fails opaquely.
#[test]
fn generic_method_over_mixin_errors_cleanly() {
    let source = "\
mixin Sized
  def width -> Int
end

class Frame
  scale: Int
  def init(@scale: Int) end
  def measure[T: Sized](item: &var T) -> Int
    item.width * self.scale
  end
end

class Wide
  include Sized
  w: Int
  def init(@w: Int) end
  def width -> Int
    self.w
  end
end

class Tall
  include Sized
  v: Int
  def init(@v: Int) end
  def width -> Int
    self.v + 1
  end
end

def main
  var f = Frame.new(3)
  var a = Wide.new(10)
  var b = Tall.new(20)
  puts \"#{f.measure(&var a)} #{f.measure(&var b)}\"
end
";
    let err = lower_error(source).expect("generic method over mixin must error at lowering");
    assert!(
        err.contains("cannot monomorphize generic method"),
        "expected a clear generic-method-not-supported error, got: {err}"
    );
    // The error must NOT be a raw placeholder symbol leaking to the user.
    assert!(
        !err.contains(": Sized_width"),
        "diagnostic must not surface the raw bound-placeholder symbol: {err}"
    );
}
