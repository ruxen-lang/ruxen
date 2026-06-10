//! Ruby `alias new_name old_name` pins (ADR: docs/decisions/alias-keyword.md).
//!
//! Covers the assertions the release-e2e stdout fixtures cannot: the alias
//! diagnostics (E1120 unknown target, E1122 collision/self-alias, E1123 staged
//! operator alias) and the fmt byte-stability of the alias item. The runtime
//! synonym behaviour (a/b/c/e/f/j) is pinned by the release-e2e cases
//! (913–918) which compile + run + assert stdout; a couple are mirrored here
//! in-process for a fast, hermetic signal.

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Type-check `source`; return the error-level diagnostics (code + message).
fn typecheck_errors(source: &str) -> Vec<(String, String)> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .map(|d| (d.code.clone().unwrap_or_default(), d.message.clone()))
        .collect()
}

/// Full pipeline compile + run (incl. borrow_check) → (stdout, stderr, exit).
fn compile_and_run(source: &str, basename: &str) -> (String, String, Option<i32>) {
    use ruxen_core::codegen;
    use ruxen_core::mir::lower::Lowerer;

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
        "typecheck errors for {basename}: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    // Borrow check must also pass (the surface-coverage lesson from Q29/649).
    let borrow_errors = ruxen_core::borrow_check::borrow_check(&result.program, &result.symbols);
    assert!(
        borrow_errors.is_empty(),
        "borrow errors for {basename}: {:?}",
        borrow_errors
            .iter()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
    );
    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .unwrap_or_else(|e| panic!("MIR lowering for {basename}: {e}"));
    codegen::compile(&mir, bin_path.to_str().unwrap())
        .unwrap_or_else(|e| panic!("codegen for {basename}: {e}"));
    let output = Command::new(&bin_path).output().expect("run");
    let _ = std::fs::remove_file(&bin_path);
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code(),
    )
}

/// Pin (g): aliasing an unknown method target → E1120.
#[test]
fn unknown_method_alias_target_is_e1120() {
    let source = r##"
class C
  def a -> Int
    1
  end
  alias b nonexistent
end
def main
  let c = C.new
  puts "#{c.a}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1120"),
        "expected E1120 for unknown alias target, got {errors:?}"
    );
}

/// Pin (g2): aliasing an unknown free-function target → E1120.
#[test]
fn unknown_free_fn_alias_target_is_e1120() {
    let source = r##"
def f -> Int
  1
end
alias g does_not_exist
def main
  puts "#{f()}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1120"),
        "expected E1120 for unknown free-fn alias target, got {errors:?}"
    );
}

/// Pin (h): a self-alias (`alias x x`) is the degenerate collision → E1122.
#[test]
fn self_alias_is_e1122() {
    let source = r##"
class C
  def a -> Int
    1
  end
  alias a a
end
def main
  let c = C.new
  puts "#{c.a}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1122"),
        "expected E1122 for self-alias, got {errors:?}"
    );
}

/// Pin (h2): an alias whose NEW name collides with an existing method → E1122.
#[test]
fn collision_with_existing_method_is_e1122() {
    let source = r##"
class C
  def a -> Int
    1
  end
  def b -> Int
    2
  end
  alias a b
end
def main
  let c = C.new
  puts "#{c.a}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1122"),
        "expected E1122 for alias colliding with existing method, got {errors:?}"
    );
}

/// Pin (d): an operator-spelled alias is staged → E1123 (ADR D6).
#[test]
fn operator_alias_is_staged_e1123() {
    let source = r##"
class V
  def push(x: Int) -> Int
    x
  end
  alias << push
end
def main
  let v = V.new
  puts "#{v.push(1)}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, _)| code == "E1123"),
        "expected E1123 for operator alias, got {errors:?}"
    );
}

/// Pin (i): `ruxen fmt` round-trips the alias item byte-stably (ADR D10) — the
/// literal `alias new old` line survives and `format(format(x)) == format(x)`.
#[test]
fn fmt_round_trips_alias_byte_stably() {
    let src = "class Bag\n  count: Int\n  def size -> Int\n    self.count\n  end\n  alias length size\nend\n";
    let out = ruxen_core::formatter::format(src).output;
    assert!(
        out.contains("alias length size"),
        "alias item not preserved verbatim; fmt output:\n{out}"
    );
    let out2 = ruxen_core::formatter::format(&out).output;
    assert_eq!(out, out2, "fmt not idempotent on alias item");

    // Free-fn alias at top level round-trips too.
    let free = "def greet -> Int\n  1\nend\n\nalias hail greet\n";
    let fout = ruxen_core::formatter::format(free).output;
    assert!(
        fout.contains("alias hail greet"),
        "free-fn alias not preserved; fmt output:\n{fout}"
    );
    assert_eq!(
        fout,
        ruxen_core::formatter::format(&fout).output,
        "fmt not idempotent on free-fn alias"
    );
}

/// Pin (a-mirror): a class method alias — both names print identical results
/// through the FULL pipeline (incl. borrow_check), in-process.
#[test]
fn class_method_alias_both_names_identical() {
    let source = r##"
class Bag
  count: Int
  def init
    self.count = 3
  end
  def size -> Int
    self.count
  end
  alias length size
end
def main
  let b = Bag.new
  puts "#{b.size}"
  puts "#{b.length}"
end
"##;
    let (stdout, stderr, code) = compile_and_run(source, "alias_class_method");
    assert_eq!(code, Some(0), "stderr={stderr:?}");
    assert_eq!(stdout, "3\n3\n", "stdout was {stdout:?}");
}
