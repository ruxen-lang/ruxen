//! Ruby-style block builder DSL over `yield self`.
//!
//! Regression cover for the three fixes that make a nested builder DSL
//! (`widget do |w| w.text "x" end`) compile in v1:
//!
//!   1. `yield self` infers the trailing block's parameter type as the
//!      enclosing class (resolve: synthesize the `__block` parameter whose
//!      `self` positions are typed as the class, not a fresh var — methods
//!      went through a cloned signature that lost the link).
//!   2. A method that `yield`s anywhere but its tail expression keeps the
//!      synthetic `__block` return type resolvable (defaulted to Unit; v1
//!      never consumes a block's value).
//!   3. Paren-less method calls with a string argument (`s.line "a"`) parse
//!      as a call, Ruby command-call style.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!(
        "{}-{}-{}.bin",
        basename,
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

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
        "typecheck errors for {}: {:?}",
        basename,
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .unwrap_or_else(|e| panic!("MIR lowering for {}: {}", basename, e));
    codegen::compile(&mir, bin_path.to_str().unwrap())
        .unwrap_or_else(|e| panic!("codegen for {}: {}", basename, e));

    let output = Command::new(&bin_path)
        .output()
        .unwrap_or_else(|e| panic!("run {}: {}", basename, e));
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// The full nested builder: `yield self` with inferred block params, a method
/// that yields mid-body, and paren-less string-argument calls — the exact
/// shape quiver's widget DSL relies on.
#[test]
fn yield_self_nested_builder_dsl() {
    let source = rx("yield_self_block_dsl");
    let (stdout, stderr, ok) = compile_and_run(&source, "yield_self_block_dsl");
    assert!(ok, "non-zero exit; stderr: {}", stderr);
    // section "[" -> a -> [nested: "[" -> b -> "]"] -> c -> "]"
    assert_eq!(stdout, "[;a;[;b;];c;];", "stdout was {:?}", stdout);
}

/// Narrow pin: a single-level `yield self` resolves the inferred block
/// parameter so a method call on it dispatches (was `?T::method`).
#[test]
fn yield_self_infers_block_param() {
    let source = r##"
class Counter
  n: Int
  def init
    self.n = 41
  end
  def with_self -> nil
    yield self
  end
  def bump -> nil
    self.n = self.n + 1
    print("#{self.n}")
  end
end
def main
  var c = Counter.new
  c.with_self do |it|
    it.bump
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "yield_self_infers_block_param");
    assert!(ok, "non-zero exit; stderr: {}", stderr);
    assert_eq!(stdout, "42", "stdout was {:?}", stdout);
}

/// Paren-less method call with a bare string argument parses as a call.
#[test]
fn paren_less_method_call_with_string_arg() {
    let source = r##"
class Sink
  acc: String
  def init
    self.acc = ""
  end
  def put(s: &String) -> nil
    self.acc = "#{self.acc}#{s}"
  end
  def show -> nil
    print("#{self.acc}")
  end
end
def main
  var s = Sink.new
  s.put "x"
  s.put "y"
  s.show
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "paren_less_method_call_with_string_arg");
    assert!(ok, "non-zero exit; stderr: {}", stderr);
    assert_eq!(stdout, "xy", "stdout was {:?}", stdout);
}
