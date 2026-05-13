//! Pin tests for `docs/specs/stdlib/vec.spec.md` gaps:
//! `Vec.first / last / contains / clone / reverse` — wired in the
//! runtime + codegen but not directly pinned.  These tests compile a
//! tiny Riven program for each, run it, and assert on the stdout so
//! the runtime contract is enforced.

use riven_core::codegen;
use riven_core::lexer::Lexer;
use riven_core::mir::lower::Lowerer;
use riven_core::parser::Parser;
use riven_core::typeck;
use std::process::Command;

fn workspace_root() -> std::path::PathBuf {
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn compile_and_run(source: &str, basename: &str) -> (String, String, bool) {
    let root = workspace_root();
    let tmp_dir = root.join("tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    let bin_path = tmp_dir.join(format!("{}.bin", basename));

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == riven_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {:?}", errors);

    let mut lowerer = Lowerer::new(&result.symbols);
    let mir = lowerer
        .lower_program(&result.program)
        .expect("MIR lowering");
    codegen::compile(&mir, bin_path.to_str().unwrap()).expect("codegen");

    let output = Command::new(&bin_path).output().expect("run binary");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// `Vec.first()` returns `Option::Some(first)` on a non-empty vec,
/// `Option::None` on an empty vec.
#[test]
fn vec_first_returns_first_element() {
    let source = r##"
def main
  let mut v: Vec[Int] = Vec.new
  v.push(10)
  v.push(20)
  v.push(30)
  match v.first
    Some(n) -> puts "first=#{n}"
    None    -> puts "empty"
  end

  let empty: Vec[Int] = Vec.new
  match empty.first
    Some(_) -> puts "should_not"
    None    -> puts "empty_ok"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_vec_first");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("first=10"), "first branch: {}", stdout);
    assert!(stdout.contains("empty_ok"), "empty branch: {}", stdout);
}

/// `Vec.last()` returns `Option::Some(last)` / `Option::None`.
#[test]
fn vec_last_returns_last_element() {
    let source = r##"
def main
  let mut v: Vec[Int] = Vec.new
  v.push(10)
  v.push(20)
  v.push(30)
  match v.last
    Some(n) -> puts "last=#{n}"
    None    -> puts "empty"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_vec_last");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("last=30"), "got: {}", stdout);
}

/// `Vec.contains(&x)` returns Bool.
#[test]
fn vec_contains_finds_element() {
    let source = r##"
def main
  let mut v: Vec[Int] = Vec.new
  v.push(1)
  v.push(2)
  v.push(3)
  if v.contains(&2)
    puts "has_2"
  else
    puts "missing_2"
  end
  if v.contains(&99)
    puts "has_99"
  else
    puts "missing_99"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_vec_contains");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("has_2"), "present: {}", stdout);
    assert!(stdout.contains("missing_99"), "absent: {}", stdout);
}

/// `Vec.clone()` returns a deep copy — modifying the original does
/// not affect the clone.
#[test]
fn vec_clone_returns_independent_copy() {
    let source = r##"
def main
  let mut original: Vec[Int] = Vec.new
  original.push(1)
  original.push(2)
  original.push(3)
  let copy = original.clone
  original.push(99)
  puts "orig_len=#{original.len}"
  puts "copy_len=#{copy.len}"
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_vec_clone");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("orig_len=4"), "orig grew: {}", stdout);
    assert!(stdout.contains("copy_len=3"), "copy unchanged: {}", stdout);
}

/// `Vec.reverse()` reverses in place.
#[test]
fn vec_reverse_inverts_order() {
    let source = r##"
def main
  let mut v: Vec[Int] = Vec.new
  v.push(1)
  v.push(2)
  v.push(3)
  v.reverse
  for n in v
    puts "n=#{n}"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_vec_reverse");
    assert!(ok, "stderr: {}", stderr);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some("n=3"), "first after reverse");
    assert_eq!(lines.get(2).copied(), Some("n=1"), "last after reverse");
}
