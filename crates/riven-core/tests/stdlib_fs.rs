//! Integration tests for Phase 2 stdlib (#06) `std::fs` additions:
//! `is_file`, `is_dir`, and `read_dir`.
//!
//! Each test creates a fresh temporary directory tree, exercises the
//! Riven-side fn against it from a compiled program, and asserts on
//! the program's stdout. Avoids any dependency on system paths whose
//! existence varies across CI runners.

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

/// Stage a unique temp directory for the test. Returns the path; the
/// directory is created fresh and the caller may populate it before
/// running the Riven binary.
fn unique_tmp_dir(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "riven_stdlib_fs_{}_{}_{}",
        name,
        std::process::id(),
        n
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create unique tmp dir");
    dir
}

/// `fs::is_file(path)` returns Bool: true on a regular file, false
/// on a directory or missing path.
#[test]
fn fs_is_file_distinguishes_regular_files() {
    let dir = unique_tmp_dir("is_file");
    let file = dir.join("a.txt");
    std::fs::write(&file, b"hello").expect("write");

    let source = format!(
        r##"
use std.fs.is_file

def main
  if is_file("{file}")
    puts "yes"
  else
    puts "no"
  end
  if is_file("{dir}")
    puts "dir_yes"
  else
    puts "dir_no"
  end
  if is_file("{missing}")
    puts "missing_yes"
  else
    puts "missing_no"
  end
end
"##,
        file = file.display(),
        dir = dir.display(),
        missing = dir.join("does_not_exist.txt").display(),
    );
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_is_file");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("yes"), "regular-file branch: {}", stdout);
    assert!(stdout.contains("dir_no"), "directory branch: {}", stdout);
    assert!(stdout.contains("missing_no"), "missing branch: {}", stdout);
}

/// `fs::is_dir(path)` is the dual of `is_file`: true on directories,
/// false on regular files or missing paths.
#[test]
fn fs_is_dir_distinguishes_directories() {
    let dir = unique_tmp_dir("is_dir");
    let file = dir.join("a.txt");
    std::fs::write(&file, b"hello").expect("write");

    let source = format!(
        r##"
use std.fs.is_dir

def main
  if is_dir("{dir}")
    puts "dir_yes"
  else
    puts "dir_no"
  end
  if is_dir("{file}")
    puts "file_yes"
  else
    puts "file_no"
  end
  if is_dir("{missing}")
    puts "missing_yes"
  else
    puts "missing_no"
  end
end
"##,
        dir = dir.display(),
        file = file.display(),
        missing = dir.join("nope").display(),
    );
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_is_dir");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("dir_yes"), "directory: {}", stdout);
    assert!(stdout.contains("file_no"), "regular file: {}", stdout);
    assert!(stdout.contains("missing_no"), "missing: {}", stdout);
}

/// `fs::read_dir(path)` returns `Result[Vec[String], IoError]`. The
/// happy path lists every entry name (without "." or ".."). We
/// populate three known files and assert each name shows up; we do
/// not assert ordering because `readdir(3)` does not guarantee any.
#[test]
fn fs_read_dir_lists_all_entries() {
    let dir = unique_tmp_dir("read_dir");
    for name in ["alpha.txt", "beta.txt", "gamma.txt"] {
        std::fs::write(dir.join(name), b"x").expect("write");
    }

    // Helper-fn workaround: Riven match arms are single-expression,
    // and Result.unwrap is typeck-only (no codegen dispatch). A
    // dedicated reducer keeps each arm a single expression yet lets
    // the rest of `main` operate on the unwrapped Vec.
    let source = format!(
        r##"
use std.fs.read_dir

def or_empty(r: Result[Vec[String], IoError]) -> Vec[String]
  match r
    Ok(v)  -> v
    Err(_) -> Vec.new
  end
end

def main
  let entries = or_empty(read_dir("{dir}"))
  puts "len=#{{entries.len}}"
  for name in entries
    puts "name=#{{name}}"
  end
end
"##,
        dir = dir.display()
    );
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_read_dir");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("len=3"),
        "expected three entries (no . / ..); got: {}",
        stdout
    );
    for name in ["alpha.txt", "beta.txt", "gamma.txt"] {
        assert!(
            stdout.contains(&format!("name={}", name)),
            "missing entry `{}` in stdout: {}",
            name,
            stdout
        );
    }
}

/// `fs::read_dir` on a missing path returns `Err`, not Ok with an
/// empty Vec.
#[test]
fn fs_read_dir_missing_path_returns_err() {
    let source = r##"
use std.fs.read_dir

def main
  match read_dir("/no/such/dir/we/hope")
    Ok(_)  -> puts "unexpectedly_ok"
    Err(_) -> puts "err_ok"
  end
end
"##;
    let (stdout, stderr, ok) = compile_and_run(source, "stdlib_fs_read_dir_missing");
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("err_ok"),
        "expected err branch, got: {}",
        stdout
    );
}
