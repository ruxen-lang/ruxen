//! Q17 — a generic FREE FUNCTION bound by a mixin must monomorphize per
//! concrete implementor, not devirtualize to the single implementor. Before
//! the fix, calling `def paint_all[T: Paintable](s: &var T, …)` with two
//! different implementors emitted the bound-placeholder callee
//! `T: Paintable_fill_rect`, which link-failed. The single-implementor case was
//! masked because mixin dispatch devirtualized to the sole impl — which is
//! exactly why quiver was capped at one `PaintSurface` implementor.
//!
//! These pins read the SAME fixtures the release-e2e harness runs
//! (`tests/release-e2e/cases/474` and `655..659`) so the cargo pin and e2e cases can
//! never drift apart, and they COMPILE + RUN + assert exact stdout (three prior
//! episodes on this branch had pins pass while real codegen was broken). The
//! Q17b's generic-method analogue reads case 474 and likewise compiles, runs,
//! and asserts exact stdout for two distinct concrete implementors.

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

fn lower_error(source: &str) -> Option<String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let mut lowerer = Lowerer::new(&result.symbols);
    lowerer.lower_program(&result.program).err()
}

fn lower_to_mir(source: &str) -> ruxen_core::mir::nodes::MirProgram {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let mut lowerer = Lowerer::new(&result.symbols);
    lowerer.lower_program(&result.program).expect("MIR lowering")
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

/// Q35: the single implementor may be a STRUCT. Its in-body `include`
/// must satisfy typeck's nominal bound and reach Q17 monomorphization.
#[test]
fn struct_implementor_satisfies_bound_and_runs() {
    let (src, expected) = case("659_generic_fn_mixin_struct_implementor.rx");
    let (stdout, stderr, ok) = compile_and_run(&src, "q35_struct_impl");
    assert!(ok, "link/run failed; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// Q17b: a generic METHOD inside a class specializes once for each concrete
/// mixin implementor (`Wide` and `Tall`) and dispatches both bodies correctly.
#[test]
fn generic_method_over_mixin_monomorphizes_per_implementor() {
    let (src, expected) = case("474_generic_method_mixin_mono.rx");
    let mir = lower_to_mir(&src);
    let names: Vec<&str> = mir.functions.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"Frame_measure_scaled__mono__Wide"));
    assert!(names.contains(&"Frame_measure_scaled__mono__Tall"));
    assert!(
        !names.contains(&"Frame_measure_scaled"),
        "placeholder-prone opaque method body must be suppressed"
    );
    let (stdout, stderr, ok) = compile_and_run(&src, "q17b_method");
    assert!(ok, "link/run failed; stderr: {stderr}");
    assert_eq!(stdout, expected, "stdout was {stdout:?}");
}

/// The same Q17b path applies to fixed struct and enum receivers; only the
/// method's mixin-bounded parameter varies.
#[test]
fn generic_method_supports_struct_and_enum_receivers() {
    let source = "\
mixin Measured
  def measure -> Int
end

struct Wide
  include Measured
  w: Int
  def measure -> Int
    self.w
  end
end

struct Scaler
  scale: Int
  def apply[T: Measured](item: &var T) -> Int
    item.measure * self.scale
  end
end

enum Multiplier
  Triple
  def apply[T: Measured](item: &var T) -> Int
    item.measure * 3
  end
end

def main
  var wide = Wide.new(7)
  let scaler = Scaler.new(4)
  puts \"struct=#{scaler.apply(&var wide)}\"
  puts \"enum=#{Multiplier.Triple.apply(&var wide)}\"
end
";
    let (stdout, stderr, ok) = compile_and_run(source, "q17b_receivers");
    assert!(ok, "link/run failed; stderr: {stderr}");
    assert_eq!(stdout, "struct=28\nenum=21\n");
}

/// The staged cross-method fixpoint is a hard error, never a call to an
/// un-emitted opaque method or a bound-placeholder symbol.
#[test]
fn generic_calling_generic_across_methods_errors_without_placeholder() {
    let source = "\
mixin Measured
  def measure -> Int
end

class Frame
  def inner[T: Measured](item: &var T) -> Int
    item.measure
  end

  def outer[U: Measured](item: &var U) -> Int
    self.inner(item)
  end
end

class Wide
  include Measured
  def measure -> Int
    7
  end
end

class Tall
  include Measured
  def measure -> Int
    9
  end
end

def main
  var frame = Frame.new
  var wide = Wide.new
  puts frame.outer(&var wide)
end
";
    let err = lower_error(source).expect("cross-method generic chain must fail clearly");
    assert!(
        err.contains("generic-calling-generic across methods"),
        "unexpected residual diagnostic: {err}"
    );
    assert!(
        !err.contains(": Measured_measure"),
        "bound placeholder leaked into residual diagnostic: {err}"
    );
}
