use riven_core::diagnostics::DiagnosticLevel;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::{borrow_check, codegen, typeck};
use std::io::Write;
use std::process::{Command, Stdio};

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

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
    let source = rvn("use_std_io_typechecks_cleanly");
    let diagnostics = typecheck_source(&source);
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
    let source = rvn("std_sync_concurrency_surface_typechecks_cleanly");
    let diagnostics = typecheck_source(&source);
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
    let source = rvn("std_sync_thread_sleep_and_yield_round_trip");
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
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "thread helpers ok\n"
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
}

#[test]
fn std_io_group_imports_and_methods_typecheck_cleanly() {
    let source = rvn("std_io_group_imports_and_methods_typecheck_cleanly");
    let diagnostics = typecheck_source(&source);
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
    let source = rvn("std_io_println_and_eprintln_round_trip");
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
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello stdout\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "hello stderr\n");
}

#[test]
fn std_io_write_str_result_is_unit_and_round_trips() {
    let source = rvn("std_io_write_str_result_is_unit_and_round_trips");
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
    assert_eq!(String::from_utf8_lossy(&output.stdout), "hello stdout");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "hello stderr");
}

#[test]
fn std_io_read_line_and_stdout_round_trip() {
    let source = rvn("std_io_read_line_and_stdout_round_trip");
    let exe = compile_to_exe(&source);
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
    let source = rvn("std_io_stdin_read_to_string_round_trip");
    let exe = compile_to_exe(&source);
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
    let source = rvn("main_shim_initializes_runtime_argv");
    let exe = compile_to_exe(&source);
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
    let source = rvn("use_std_env_typechecks_cleanly");
    let diagnostics = typecheck_source(&source);
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
    let source = rvn("std_env_args_round_trip");
    let exe = compile_to_exe(&source);
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
    let source = rvn("std_env_var_round_trip");
    let exe = compile_to_exe(&source);
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
    let source = rvn("use_std_fs_typechecks_cleanly");
    let diagnostics = typecheck_source(&source);
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

    let source = rvn("std_fs_round_trip").replace("{path}", &path_str);

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
    let source = rvn("use_std_process_typechecks_cleanly");
    let diagnostics = typecheck_source(&source);
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
    let source = rvn("std_process_exit_round_trip");
    let exe = compile_to_exe(&source);
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
    let source = rvn("use_std_fs_mutation_helpers_typecheck_cleanly");
    let diagnostics = typecheck_source(&source);
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

    let source = rvn("std_fs_mutation_helpers_round_trip")
        .replace("{root}", &root_str)
        .replace("{file_a}", &file_a_str)
        .replace("{file_b}", &file_b_str);

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
    let source = rvn("use_std_fs_create_dir_all_typechecks_cleanly");
    let diagnostics = typecheck_source(&source);
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

    let source = rvn("std_fs_create_dir_all_round_trip").replace("{nested}", &nested_str);

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
