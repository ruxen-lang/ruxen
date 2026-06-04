//! Integration tests for Phase 2 stdlib (#06.5 T2) `std::io::File`.
//!
//! Each test stages a real temp file on the host, compiles a small
//! Ruxen program that exercises one File surface, runs the binary,
//! and asserts on stdout. Path templating mirrors `stdlib_fs.rs` —
//! the `.rx` fixtures contain `__FILE__` / `__DIR__` placeholders
//! that the test driver substitutes before compilation.

use ruxen_core::codegen;
use ruxen_core::lexer::Lexer;
use ruxen_core::mir::lower::Lowerer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;
use std::process::Command;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

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

/// Stage a unique temp directory per test so concurrent test runs
/// don't trample each other. Mirrors `stdlib_fs::unique_tmp_dir`.
fn unique_tmp_dir(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "ruxen_stdlib_file_{}_{}_{}",
        name,
        std::process::id(),
        n
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create unique tmp dir");
    dir
}

/// `File.open` on an existing file returns Ok, and `read_to_string`
/// returns the file's contents. The simplest happy path through the
/// new class.
#[test]
fn file_open_read_to_string_round_trips() {
    let dir = unique_tmp_dir("open_read");
    let file = dir.join("greeting.txt");
    std::fs::write(&file, b"hello file class").expect("write");

    let source =
        rx("file_open_read_to_string_round_trips").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_file_open_read");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("contents=hello file class"),
        "expected file contents in stdout; got: {}",
        stdout
    );
}

/// `File.open` on a missing path returns Err(IoError.NotFound(_)).
/// This is the T1 errno-classifier wiring exercised end-to-end —
/// the open(2) returns ENOENT, the runtime maps it to NotFound, and
/// `.kind()` round-trips to IoErrorKind.NotFound at user-side.
#[test]
fn file_open_missing_returns_not_found() {
    let dir = unique_tmp_dir("open_missing");
    let missing = dir.join("does_not_exist.txt");

    let source = rx("file_open_missing_returns_not_found")
        .replace("__FILE__", &missing.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_file_open_missing");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("kind=NotFound"),
        "expected NotFound kind, got: {}",
        stdout
    );
}

/// `File.create` truncates an existing file. Verifies the
/// `O_WRONLY | O_CREAT | O_TRUNC` flag set by re-opening for read
/// and asserting the byte count is the new write length, not the
/// concatenation of old + new.
#[test]
fn file_create_truncates_existing() {
    let dir = unique_tmp_dir("create_trunc");
    let file = dir.join("payload.txt");
    std::fs::write(&file, b"long original contents that should be truncated")
        .expect("stage original");

    let source =
        rx("file_create_truncates_existing").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_file_create_trunc");
    let after = std::fs::read_to_string(&file).expect("read final");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("wrote_ok"),
        "expected wrote_ok in stdout: {}",
        stdout
    );
    assert_eq!(
        after, "short",
        "create should truncate; file contents: {:?}",
        after
    );
}

/// `File.append` preserves the original bytes and appends afterwards.
/// `O_WRONLY | O_CREAT | O_APPEND` — every write goes to the end
/// regardless of the current file position.
#[test]
fn file_append_preserves_existing_bytes() {
    let dir = unique_tmp_dir("append");
    let file = dir.join("log.txt");
    std::fs::write(&file, b"foo").expect("stage original");

    let source =
        rx("file_append_preserves_existing_bytes").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_file_append");
    let after = std::fs::read_to_string(&file).expect("read final");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("wrote_ok"),
        "expected wrote_ok in stdout: {}",
        stdout
    );
    assert_eq!(after, "foobar", "append should preserve original bytes");
}

/// `OpenOptions.new().read(true).write(true).create(true)` →
/// File.open_options opens for read+write, creating the file if
/// missing. We then write through the same handle and verify
/// content on disk.
#[test]
fn file_open_options_rw_create() {
    let dir = unique_tmp_dir("oo_rw_create");
    let file = dir.join("rw.txt");

    let source = rx("file_open_options_rw_create").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_file_oo_rw_create");
    let after = std::fs::read_to_string(&file).ok();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("wrote_ok"),
        "expected wrote_ok; got: {}",
        stdout
    );
    assert_eq!(
        after.as_deref(),
        Some("from OpenOptions"),
        "file contents should match write"
    );
}

/// `OpenOptions.new()` with no read/write/append set must surface
/// `IoError.InvalidInput` per E0711. The user-side error message is
/// not asserted (the runtime payload string is canonical and may
/// drift) — we only assert the variant kind.
#[test]
fn file_open_options_no_access_mode_returns_invalid_input() {
    let dir = unique_tmp_dir("oo_no_mode");
    let file = dir.join("ignored.txt");

    let source = rx("file_open_options_no_access_mode_returns_invalid_input")
        .replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_file_oo_no_mode");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("kind=InvalidInput"),
        "expected InvalidInput kind, got: {}",
        stdout
    );
}

/// `File.seek(SeekFrom.Start(3))` after writing "0123456789" lets a
/// subsequent `read_to_string` return "3456789".
#[test]
fn file_seek_start_skips_prefix() {
    let dir = unique_tmp_dir("seek_start");
    let file = dir.join("digits.txt");
    std::fs::write(&file, b"0123456789").expect("stage digits");

    let source =
        rx("file_seek_start_skips_prefix").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_file_seek_start");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("tail=3456789"),
        "expected tail=3456789, got: {}",
        stdout
    );
}

/// `File.seek(SeekFrom.End(0))` returns the file size as the new
/// position. Cross-checks the value against the underlying byte
/// count.
#[test]
fn file_seek_end_returns_size() {
    let dir = unique_tmp_dir("seek_end");
    let file = dir.join("payload.bin");
    let bytes = b"0123456789ABCDEF"; // 16 bytes
    std::fs::write(&file, bytes).expect("stage payload");

    let source = rx("file_seek_end_returns_size").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_file_seek_end");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("pos=16"),
        "expected pos=16, got: {}",
        stdout
    );
}

/// `File.metadata()` returns the same Metadata shape as
/// `fs.metadata`. The `.len` accessor must match the file size on
/// disk; `.is_file` is true for a regular file.
#[test]
fn file_metadata_matches_fs_metadata() {
    let dir = unique_tmp_dir("metadata");
    let file = dir.join("size_me.txt");
    std::fs::write(&file, b"exactly twelve").expect("stage 14 bytes");
    // "exactly twelve" is 14 bytes — name is a holdover from an
    // earlier draft; the assertion below pins the actual size.

    let source =
        rx("file_metadata_matches_fs_metadata").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_file_metadata");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    let expected_len = "exactly twelve".len(); // 14
    assert!(
        stdout.contains(&format!("len={}", expected_len)),
        "expected len={}, got: {}",
        expected_len,
        stdout
    );
    assert!(
        stdout.contains("is_file=true"),
        "expected is_file=true, got: {}",
        stdout
    );
}

/// Drop semantics: opening 1024 files inside a block scope (each
/// dropped on scope exit) must not exhaust the per-process fd limit.
/// Without `File_drop` registered in `user_drop_classes` this test
/// would either hit EMFILE on the higher-iteration runs or leak the
/// fds until process exit (still a contract violation — the surface
/// promises drop-runs-close).
///
/// 1024 is below the typical macOS soft limit (256 → 10240 with
/// `ulimit -n 1024` baseline) — we're not stress-testing the limit,
/// we're proving drop releases the fd in normal use.
#[test]
fn file_drop_closes_fd() {
    let dir = unique_tmp_dir("drop_closes");
    let file = dir.join("touch.txt");
    std::fs::write(&file, b"x").expect("stage");

    let source = rx("file_drop_closes_fd").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_file_drop_closes");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("opened=1024"),
        "expected 1024 opens; got: {}",
        stdout
    );
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
