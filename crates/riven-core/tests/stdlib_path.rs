//! Integration test for Phase 3 `std::path` module.
//!
//! Verifies that `path_join`, `path_parent`, `path_file_name`,
//! `path_extension`, and `path_is_absolute` resolve through the
//! resolver, lower to the right runtime calls, and produce correct
//! values at runtime. Linux-style separators only — Windows backslash
//! is a non-goal for v1.

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

#[test]
fn path_module_basic_operations() {
    let source = r#"
use std.path.{path_join, path_parent, path_file_name, path_extension, path_is_absolute}

def main
  let p = path_join(&"/usr/local", &"bin/riven.rvn")
  puts p
  puts path_parent(&p)
  puts path_file_name(&p)
  puts path_extension(&p)
  if path_is_absolute(&p)
    puts "abs"
  else
    puts "rel"
  end
end
"#;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_path_basic");
    assert!(
        ok,
        "binary exited non-zero. stdout=[{}] stderr=[{}]",
        stdout, stderr
    );
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some("/usr/local/bin/riven.rvn"));
    assert_eq!(lines.get(1).copied(), Some("/usr/local/bin"));
    assert_eq!(lines.get(2).copied(), Some("riven.rvn"));
    assert_eq!(lines.get(3).copied(), Some("rvn"));
    assert_eq!(lines.get(4).copied(), Some("abs"));
}

#[test]
fn path_join_handles_absolute_second() {
    let source = r#"
use std.path.path_join

def main
  # When `b` is absolute, it overrides `a`.
  puts path_join(&"/etc", &"/usr/bin")
end
"#;
    let (stdout, _stderr, ok) = compile_and_run(source, "stdlib_path_abs_override");
    assert!(ok, "stdout=[{}]", stdout);
    assert!(
        stdout.lines().next() == Some("/usr/bin"),
        "absolute second should override; got: [{}]",
        stdout
    );
}

#[test]
fn path_extension_empty_when_missing() {
    let source = r#"
use std.path.path_extension

def main
  let e1 = path_extension(&"/foo/bar")
  let e2 = path_extension(&"/foo/.hidden")
  if e1.is_empty() && e2.is_empty()
    puts "ok"
  else
    puts "fail e1=[#{e1}] e2=[#{e2}]"
  end
end
"#;
    let (stdout, _stderr, ok) = compile_and_run(source, "stdlib_path_no_ext");
    assert!(ok, "stdout=[{}]", stdout);
    assert!(stdout.contains("ok"), "got: [{}]", stdout);
}

#[test]
fn path_is_absolute_detects_root() {
    let source = r#"
use std.path.path_is_absolute

def main
  if path_is_absolute(&"/etc") && !path_is_absolute(&"foo/bar")
    puts "ok"
  else
    puts "fail"
  end
end
"#;
    let (stdout, _stderr, ok) = compile_and_run(source, "stdlib_path_is_abs");
    assert!(ok, "stdout=[{}]", stdout);
    assert!(stdout.contains("ok"), "got: [{}]", stdout);
}
