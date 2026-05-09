use riven_core::diagnostics::DiagnosticLevel;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::{borrow_check, codegen, typeck};
use std::io::Write;
use std::process::{Command, Stdio};

fn typecheck_source(source: &str) -> Vec<riven_core::diagnostics::Diagnostic> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    typeck::type_check(&program).diagnostics
}

fn compile_to_exe(source: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let output_path = format!(
        "{}/riven_std_io_{}_{}.exe",
        std::env::temp_dir().display(),
        std::process::id(),
        id
    );

    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse failed");
    let type_result = typeck::type_check(&program);
    let errors: Vec<_> = type_result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "type errors: {:?}", errors);

    let borrow_errors = borrow_check::borrow_check(&type_result.program, &type_result.symbols);
    assert!(
        borrow_errors.is_empty(),
        "borrow errors: {:?}",
        borrow_errors
    );

    let mut lowerer = riven_core::mir::lower::Lowerer::new(&type_result.symbols);
    let mir_program = lowerer
        .lower_program(&type_result.program)
        .expect("MIR lowering failed");
    codegen::compile(&mir_program, &output_path).expect("codegen failed");

    output_path
}

#[test]
fn use_std_io_typechecks_cleanly() {
    let source = r#"
use std.io

def main
  puts "ok"
end
"#;

    let diagnostics = typecheck_source(source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();

    assert!(
        errors.is_empty(),
        "expected no typecheck errors, got: {:?}",
        errors
    );
}

#[test]
fn std_sync_concurrency_surface_typechecks_cleanly() {
    let source = r#"
use std.sync.{Thread, JoinHandle, ThreadId, Mutex, MutexGuard, Arc, PoisonError, ThreadPanic}

def main
  let mutex: Mutex[Int] = Mutex.new(41)
  let guard_result: Result[MutexGuard[Int], PoisonError] = mutex.lock()
  let guard: MutexGuard[Int] = mutex.lock!()
  let try_guard: Option[MutexGuard[Int]] = mutex.try_lock()
  let inner: Result[Int, PoisonError] = mutex.into_inner()

  let shared: Arc[Int] = Arc.new(5)
  let shared2: Arc[Int] = shared.clone()
  let strong: USize = shared.strong_count()
  let weak: USize = shared.weak_count()
  let shared_ref: &Int = shared.deref()
  let guard_ref: &Int = guard.deref()
  let guard_mut_ref: &mut Int = guard.deref_mut()

  let handle: JoinHandle[Int] = Thread.spawn({ || 42 })
  let joined: Result[Int, ThreadPanic] = handle.join()

  let handle2 = Thread.spawn({ || 7 })
  let joined2: Int = handle2.join!()
  let spawned_thread_id: ThreadId = Thread.spawn({ || 1 }).thread_id()
  let current: Thread = Thread.current()
  let current_id: ThreadId = current.id()
  let current_name: Option[String] = current.name()
  Thread.sleep(0)
  Thread.yield_now()

  let _ = joined
end
"#;

    let diagnostics = typecheck_source(source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();

    assert!(
        errors.is_empty(),
        "expected no typecheck errors, got: {:?}",
        errors
    );
}

#[test]
fn std_sync_thread_sleep_and_yield_round_trip() {
    let source = r#"
use std.sync.Thread

def main
  Thread.sleep(0)
  Thread.yield_now()
  puts "thread helpers ok"
end
"#;

    let exe = compile_to_exe(source);
    let output = Command::new(&exe)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&exe);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "thread helpers ok\n"
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

#[test]
fn std_io_group_imports_and_methods_typecheck_cleanly() {
    let source = r#"
use std.io.{read_line, stdin, stdout, stderr, println, eprintln, Stdin, Stdout, Stderr, IoError}

def echo(reader: Stdin, writer: Stdout, err: Stderr) -> Result[String, IoError]
  writer.flush()
  err.flush()
  reader.read_line()
end

def main
  let reader = stdin()
  let writer = stdout()
  let err = stderr()
  println("prelude line")
  eprintln("err line")
  let line = echo(reader, writer, err).expect!("echo")
  writer.write_str(line).expect!("write")
  writer.flush().expect!("flush")
  let line2 = read_line().expect!("read_line")
  err.write_str(line2).expect!("stderr")
end
"#;

    let diagnostics = typecheck_source(source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();

    assert!(
        errors.is_empty(),
        "expected no typecheck errors, got: {:?}",
        errors
    );
}

#[test]
fn std_io_println_and_eprintln_round_trip() {
    let source = r#"
use std.io.{println, eprintln}

def main
  println("hello stdout")
  eprintln("hello stderr")
end
"#;

    let exe = compile_to_exe(source);
    let output = Command::new(&exe)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&exe);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello stdout\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "hello stderr\n");
}

#[test]
fn std_io_write_str_result_is_unit_and_round_trips() {
    let source = r#"
use std.io.{stdout, stderr, Stdout, Stderr, IoError}

def write_stdout(out: Stdout) -> Result[(), IoError]
  out.write_str("hello stdout")
end

def write_stderr(err: Stderr) -> Result[(), IoError]
  err.write_str("hello stderr")
end

def main
  let out = stdout()
  let err = stderr()
  write_stdout(out).expect!("stdout write")
  stdout().flush().expect!("stdout flush")
  write_stderr(err).expect!("stderr write")
  stderr().flush().expect!("stderr flush")
end
"#;

    let exe = compile_to_exe(source);
    let output = Command::new(&exe)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&exe);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello stdout");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "hello stderr");
}

#[test]
fn std_io_read_line_and_stdout_round_trip() {
    let source = r#"
use std.io.{read_line, stdout}

def main
  let line = read_line().expect!("stdin line")
  let out = stdout()
  out.write_str(line).expect!("stdout write")
  out.flush().expect!("stdout flush")
end
"#;

    let exe = compile_to_exe(source);
    let mut child = Command::new(&exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");

    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(b"hello from stdin\n")
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    let _ = std::fs::remove_file(&exe);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "hello from stdin\n"
    );
}

#[test]
fn std_io_stdin_read_to_string_round_trip() {
    let source = r#"
use std.io.{stdin, stdout}

def main
  let reader = stdin()
  let out = stdout()
  let text = reader.read_to_string().expect!("stdin read_to_string")
  out.write_str(text).expect!("stdout write")
  out.flush().expect!("stdout flush")
end
"#;

    let exe = compile_to_exe(source);
    let mut child = Command::new(&exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");

    child
        .stdin
        .as_mut()
        .expect("stdin pipe")
        .write_all(b"hello\nfrom\nread_to_string")
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait");
    let _ = std::fs::remove_file(&exe);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "hello\nfrom\nread_to_string"
    );
}

#[test]
fn main_shim_initializes_runtime_argv() {
    let source = r##"
extern "C"
  def riven_env_args_count() -> Int64
end

def main
  puts "#{riven_env_args_count()}"
end
"##;

    let exe = compile_to_exe(source);
    let output = Command::new(&exe)
        .arg("alpha")
        .arg("beta")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&exe);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "3\n");
}

#[test]
fn use_std_env_typechecks_cleanly() {
    let source = r##"
use std.env.{args, var}

def main
  let argv = args()
  puts "#{argv.len()}"
  puts var("HOME").expect!("home")
end
"##;

    let diagnostics = typecheck_source(source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();

    assert!(
        errors.is_empty(),
        "expected no typecheck errors, got: {:?}",
        errors
    );
}

#[test]
fn std_env_args_round_trip() {
    let source = r##"
use std.env.args

def main
  let argv = args()
  puts "#{argv.len()}"
end
"##;

    let exe = compile_to_exe(source);
    let output = Command::new(&exe)
        .arg("alpha")
        .arg("beta")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&exe);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "3\n");
}

#[test]
fn std_env_var_round_trip() {
    let source = r#"
use std.env.var

def main
  puts var("RIVEN_STD_ENV_TEST").expect!("env")
end
"#;

    let exe = compile_to_exe(source);
    let output = Command::new(&exe)
        .env("RIVEN_STD_ENV_TEST", "env value")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&exe);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "env value\n");
}

#[test]
fn use_std_fs_typechecks_cleanly() {
    let source = r#"
use std.fs.{read_to_string, write, exists}

def main
  if exists("fixture.txt")
    puts read_to_string("fixture.txt").expect!("read")
  end
  write("fixture.txt", "hello file").expect!("write")
end
"#;

    let diagnostics = typecheck_source(source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();

    assert!(
        errors.is_empty(),
        "expected no typecheck errors, got: {:?}",
        errors
    );
}

#[test]
fn std_fs_round_trip() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("riven_std_fs_{}_{}.txt", std::process::id(), id));
    let path_str = path.to_string_lossy().replace('\\', "\\\\");
    let _ = std::fs::remove_file(&path);

    let source = format!(
        r##"
use std.fs.{{read_to_string, write, exists}}

def main
  write("{path}", "hello file").expect!("write")
  if exists("{path}")
    puts read_to_string("{path}").expect!("read")
  else
    puts "missing"
  end
end
"##,
        path = path_str
    );

    let exe = compile_to_exe(&source);
    let output = Command::new(&exe)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&exe);
    let _ = std::fs::remove_file(&path);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello file\n");
}

#[test]
fn use_std_process_typechecks_cleanly() {
    let source = r#"
use std.process.exit

def main
  exit(0)
end
"#;

    let diagnostics = typecheck_source(source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();

    assert!(
        errors.is_empty(),
        "expected no typecheck errors, got: {:?}",
        errors
    );
}

#[test]
fn std_process_exit_round_trip() {
    let source = r#"
use std.process.exit

def main
  exit(23)
end
"#;

    let exe = compile_to_exe(source);
    let output = Command::new(&exe)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&exe);

    assert_eq!(output.status.code(), Some(23));
    assert!(output.stdout.is_empty(), "expected no stdout");
    assert!(output.stderr.is_empty(), "expected no stderr");
}

#[test]
fn use_std_fs_mutation_helpers_typecheck_cleanly() {
    let source = r#"
use std.fs.{create_dir, rename, remove_file}

def main
  create_dir("tmp-dir").expect!("mkdir")
  rename("tmp-dir/a.txt", "tmp-dir/b.txt").expect!("rename")
  remove_file("tmp-dir/b.txt").expect!("remove")
end
"#;

    let diagnostics = typecheck_source(source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();

    assert!(
        errors.is_empty(),
        "expected no typecheck errors, got: {:?}",
        errors
    );
}

#[test]
fn std_fs_mutation_helpers_round_trip() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "riven_std_fs_mutation_{}_{}",
        std::process::id(),
        id
    ));
    let file_a = root.join("a.txt");
    let file_b = root.join("b.txt");
    let root_str = root.to_string_lossy().replace('\\', "\\\\");
    let file_a_str = file_a.to_string_lossy().replace('\\', "\\\\");
    let file_b_str = file_b.to_string_lossy().replace('\\', "\\\\");

    let _ = std::fs::remove_file(&file_a);
    let _ = std::fs::remove_file(&file_b);
    let _ = std::fs::remove_dir(&root);

    let source = format!(
        r##"
use std.fs.{{create_dir, write, rename, exists, remove_file}}

def main
  create_dir("{root}").expect!("create_dir")
  write("{file_a}", "temp").expect!("write")
  rename("{file_a}", "{file_b}").expect!("rename")
  remove_file("{file_b}").expect!("remove_file")
  if exists("{file_b}")
    puts "still-there"
  else
    puts "gone"
  end
end
"##,
        root = root_str,
        file_a = file_a_str,
        file_b = file_b_str
    );

    let exe = compile_to_exe(&source);
    let output = Command::new(&exe)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&exe);
    let _ = std::fs::remove_file(&file_a);
    let _ = std::fs::remove_file(&file_b);
    let _ = std::fs::remove_dir(&root);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "gone\n");
}

#[test]
fn use_std_fs_create_dir_all_typechecks_cleanly() {
    let source = r#"
use std.fs.create_dir_all

def main
  create_dir_all("tmp/a/b/c").expect!("mkdirs")
end
"#;

    let diagnostics = typecheck_source(source);
    let errors: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();

    assert!(
        errors.is_empty(),
        "expected no typecheck errors, got: {:?}",
        errors
    );
}

#[test]
fn std_fs_create_dir_all_round_trip() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("riven_std_fs_all_{}_{}", std::process::id(), id));
    let nested = root.join("a").join("b").join("c");
    let root_str = root.to_string_lossy().replace('\\', "\\\\");
    let nested_str = nested.to_string_lossy().replace('\\', "\\\\");

    let _ = std::fs::remove_dir_all(&root);

    let source = format!(
        r##"
use std.fs.{{create_dir_all, exists}}

def main
  create_dir_all("{nested}").expect!("create_dir_all")
  if exists("{nested}")
    puts "created"
  else
    puts "missing"
  end
end
"##,
        nested = nested_str
    );

    let exe = compile_to_exe(&source);
    let output = Command::new(&exe)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run");
    let _ = std::fs::remove_file(&exe);

    assert!(
        output.status.success(),
        "expected success, status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "created\n");
    assert!(nested.exists(), "expected nested directory to exist");

    let _ = std::fs::remove_dir_all(&root);
    let _ = root_str; // keep the root path materialized for cleanup/debug symmetry
}
