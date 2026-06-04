//! Integration tests for Phase 2 stdlib (#06) `std::fs` additions:
//! `is_file`, `is_dir`, and `read_dir`.
//!
//! Each test creates a fresh temporary directory tree, exercises the
//! Ruxen-side fn against it from a compiled program, and asserts on
//! the program's stdout. Avoids any dependency on system paths whose
//! existence varies across CI runners.

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
    let bin_path = tmp_dir.join(format!(
        "{}-{}-{}.bin",
        basename,
        std::process::id(),
        ruxen_unique_id()
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

/// Stage a unique temp directory for the test. Returns the path; the
/// directory is created fresh and the caller may populate it before
/// running the Ruxen binary.
fn unique_tmp_dir(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "ruxen_stdlib_fs_{}_{}_{}",
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

    let source = rx("fs_is_file_distinguishes_regular_files")
        .replace("__FILE__", &file.display().to_string())
        .replace("__DIR__", &dir.display().to_string())
        .replace(
            "__MISSING__",
            &dir.join("does_not_exist.txt").display().to_string(),
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

    let source = rx("fs_is_dir_distinguishes_directories")
        .replace("__DIR__", &dir.display().to_string())
        .replace("__FILE__", &file.display().to_string())
        .replace("__MISSING__", &dir.join("nope").display().to_string());
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

    // Helper-fn workaround: Ruxen match arms are single-expression,
    // and Result.unwrap is typeck-only (no codegen dispatch). A
    // dedicated reducer keeps each arm a single expression yet lets
    // the rest of `main` operate on the unwrapped Array.
    let source = rx("fs_read_dir_lists_all_entries").replace("__DIR__", &dir.display().to_string());
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
    let source = rx("fs_read_dir_missing_path_returns_err");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_read_dir_missing");
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("err_ok"),
        "expected err branch, got: {}",
        stdout
    );
}

// ─── fs spec B1 / B2 / B3 / B8 direct pins (gap fill 2026-05) ──────────

/// `fs::write(path, contents)` then `fs::read_to_string(path)` round-trip.
/// Pins spec B1 + B2.
#[test]
fn fs_write_then_read_to_string_round_trips() {
    let dir = unique_tmp_dir("write_read");
    let file = dir.join("a.txt");
    let source = rx("fs_write_then_read_to_string_round_trips")
        .replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_write_read");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("wrote"), "write branch: {}", stdout);
    assert!(stdout.contains("read=hello fs"), "read branch: {}", stdout);
}

/// `fs::read_to_string(missing)` returns `Result::Err(_)`.  Pins B1 negative.
#[test]
fn fs_read_to_string_missing_returns_err() {
    let source = rx("fs_read_to_string_missing_returns_err");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_read_missing");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("err_ok"), "got: {}", stdout);
}

/// `fs::exists(path)` returns `true` for an existing file, `false`
/// for a missing path.  Pins spec B3.
#[test]
fn fs_exists_distinguishes_existing_and_missing() {
    let dir = unique_tmp_dir("exists");
    let file = dir.join("a.txt");
    std::fs::write(&file, b"x").expect("write");

    let source = rx("fs_exists_distinguishes_existing_and_missing")
        .replace("__FILE__", &file.display().to_string())
        .replace("__MISSING__", &dir.join("nope").display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_exists");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("yes"), "existing: {}", stdout);
    assert!(stdout.contains("missing_no"), "missing: {}", stdout);
}

/// `fs::create_dir(path)` creates the directory, then `fs::is_dir`
/// confirms it.  Pins one half of spec B8.
#[test]
fn fs_create_dir_then_is_dir() {
    let dir = unique_tmp_dir("create_dir");
    // Use a nested name that doesn't yet exist.
    let sub = dir.join("nested");

    let source = rx("fs_create_dir_then_is_dir").replace("__SUB__", &sub.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_create_dir");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("created"), "create: {}", stdout);
    assert!(stdout.contains("is_dir_yes"), "is_dir: {}", stdout);
}

/// `fs::metadata(file)` returns Ok with a positive `len` and the kind
/// predicates correctly distinguishing a regular file. Pins spec
/// tier1_01_stdlib §3.6 `metadata`.
#[test]
fn fs_metadata_file_returns_len_and_kind() {
    let dir = unique_tmp_dir("metadata_file");
    let file = dir.join("payload.txt");
    std::fs::write(&file, b"hello metadata!").expect("write");

    let source = rx("fs_metadata_file_returns_len_and_kind")
        .replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_metadata_file");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("len_positive"), "len: {}", stdout);
    assert!(stdout.contains("is_file_yes"), "is_file: {}", stdout);
    assert!(stdout.contains("is_dir_no"), "is_dir: {}", stdout);
    assert!(stdout.contains("is_symlink_no"), "is_symlink: {}", stdout);
}

/// `fs::metadata(dir)` reports `is_dir = true` and `is_file = false`.
/// Pins the dir branch of `metadata`.
#[test]
fn fs_metadata_dir_reports_is_dir() {
    let dir = unique_tmp_dir("metadata_dir");
    let source =
        rx("fs_metadata_dir_reports_is_dir").replace("__DIR__", &dir.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_metadata_dir");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("dir_yes"), "is_dir: {}", stdout);
    assert!(stdout.contains("file_no"), "is_file: {}", stdout);
}

/// `fs::metadata(missing)` returns `Result::Err(_)` rather than panicking
/// or returning Ok with zero fields. Pins the negative path.
#[test]
fn fs_metadata_missing_returns_err() {
    let source = rx("fs_metadata_missing_returns_err");
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_metadata_missing");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("err_ok"), "got: {}", stdout);
}

/// `fs::write` then `fs::remove_file` then `fs::exists` round-trip.
/// Pins another half of spec B8.
#[test]
fn fs_remove_file_then_exists_false() {
    let dir = unique_tmp_dir("remove_file");
    let file = dir.join("byebye.txt");

    let source =
        rx("fs_remove_file_then_exists_false").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_remove_file");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("removed"), "remove: {}", stdout);
    assert!(stdout.contains("gone"), "after-remove exists: {}", stdout);
}

// ─── Phase 2 stdlib (#06.5 T3): fs completeness ────────────────────────
//
// Eight free functions: copy / rename / create_dir_all / remove_dir_all /
// canonicalize / write_atomic / read_link / symlink. `rename` and
// `create_dir_all` runtime entries were added in T2 (commit d44387f);
// T3 wires them into the resolver / codegen and adds positive +
// negative coverage here.

/// `fs::copy(src, dst)` reads the source and writes a fresh dest file,
/// returning bytes-copied on success. Round-trips against read_to_string.
#[test]
fn fs_copy_round_trip() {
    let dir = unique_tmp_dir("copy");
    let src = dir.join("src.txt");
    let dst = dir.join("dst.txt");

    let source = rx("fs_copy_round_trip")
        .replace("__SRC__", &src.display().to_string())
        .replace("__DST__", &dst.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_copy_round_trip");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    // "hello copy" = 10 bytes.
    assert!(stdout.contains("copied=10"), "copy bytes: {}", stdout);
    assert!(stdout.contains("read=hello copy"), "read-back: {}", stdout);
}

/// `fs::copy` against a missing source surfaces `IoError.NotFound`.
#[test]
fn fs_copy_missing_source_returns_not_found() {
    let dir = unique_tmp_dir("copy_missing");
    let missing = dir.join("nope.txt");
    let dst = dir.join("dst.txt");

    let source = rx("fs_copy_missing_source_returns_not_found")
        .replace("__MISSING__", &missing.display().to_string())
        .replace("__DST__", &dst.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_copy_missing");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(
        stdout.contains("matched_not_found"),
        "expected NotFound variant; got: {}",
        stdout
    );
}

/// `fs::rename` moves the file: source disappears, target has the
/// original contents.
#[test]
fn fs_rename_round_trip() {
    let dir = unique_tmp_dir("rename");
    let src = dir.join("from.txt");
    let dst = dir.join("to.txt");

    let source = rx("fs_rename_round_trip")
        .replace("__SRC__", &src.display().to_string())
        .replace("__DST__", &dst.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_rename_round_trip");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("renamed"), "rename: {}", stdout);
    assert!(stdout.contains("src_gone"), "source removed: {}", stdout);
    assert!(
        stdout.contains("dst=renamed content"),
        "target contents: {}",
        stdout
    );
}

/// `fs::create_dir_all` builds every missing component of a nested path.
#[test]
fn fs_create_dir_all_nested() {
    let dir = unique_tmp_dir("cdr_all");
    let a = dir.join("a");
    let b = a.join("b");
    let c = b.join("c");

    let source = rx("fs_create_dir_all_nested")
        .replace("__NESTED__", &c.display().to_string())
        .replace("__A__", &a.display().to_string())
        .replace("__B__", &b.display().to_string())
        .replace("__C__", &c.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_create_dir_all_nested");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("created"), "create: {}", stdout);
    assert!(stdout.contains("a_yes"), "a: {}", stdout);
    assert!(stdout.contains("b_yes"), "b: {}", stdout);
    assert!(stdout.contains("c_yes"), "c: {}", stdout);
}

/// `fs::create_dir_all` is idempotent — calling twice on the same path
/// is Ok both times (EEXIST is swallowed by design).
#[test]
fn fs_create_dir_all_idempotent() {
    let dir = unique_tmp_dir("cdr_idem");
    let path = dir.join("x").join("y");

    let source =
        rx("fs_create_dir_all_idempotent").replace("__PATH__", &path.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_create_dir_all_idempotent");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("first_ok"), "first: {}", stdout);
    assert!(stdout.contains("second_ok"), "second: {}", stdout);
}

/// `fs::remove_dir_all` deletes a tree containing files, subdirs, and a
/// symlink. The symlink must be unlinked (not followed), then the parent
/// tree is emptied, then the root rmdir'd.
#[test]
fn fs_remove_dir_all_tree() {
    let dir = unique_tmp_dir("rmrf");
    let root = dir.join("tree");
    let sub = root.join("sub");
    std::fs::create_dir_all(&sub).expect("create_dir_all");
    std::fs::write(root.join("a.txt"), b"hello").expect("write a");
    std::fs::write(sub.join("b.txt"), b"world").expect("write b");
    // External target for the symlink; we don't want remove_dir_all to
    // follow into it.
    let outside = dir.join("outside.txt");
    std::fs::write(&outside, b"OUTSIDE - must survive").expect("write outside");
    std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");

    let source = rx("fs_remove_dir_all_tree").replace("__ROOT__", &root.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_remove_dir_all_tree");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("removed"), "remove: {}", stdout);
    assert!(stdout.contains("gone"), "after-remove: {}", stdout);
    // The symlink target must still exist — we didn't follow it.
    assert!(
        outside.exists(),
        "symlink target was followed into: {}",
        outside.display()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `fs::canonicalize` resolves symlinks and returns the target's
/// absolute path.
#[test]
fn fs_canonicalize_resolves_symlink_to_target() {
    let dir = unique_tmp_dir("canon");
    // canonicalize the temp dir up front so target/link path comparisons
    // do not get tripped up by /var → /private/var on macOS.
    let canon_dir = std::fs::canonicalize(&dir).expect("canon dir");
    let target = canon_dir.join("real.txt");
    std::fs::write(&target, b"x").expect("write");
    let link = canon_dir.join("alias.txt");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");

    let source = rx("fs_canonicalize_symlink").replace("__LINK__", &link.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_canonicalize_symlink");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    let expected = format!("resolved={}", target.display());
    assert!(
        stdout.contains(&expected),
        "expected `{}`; got: {}",
        expected,
        stdout
    );
}

/// `fs::write_atomic` writes the contents, replaces an existing file,
/// and leaves no `.tmp.<pid>` siblings behind on success.
#[test]
fn fs_write_atomic_replaces_existing_file() {
    let dir = unique_tmp_dir("write_atomic");
    let file = dir.join("config.toml");

    let source = rx("fs_write_atomic_replaces").replace("__FILE__", &file.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_write_atomic_replaces");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("first_ok"), "first: {}", stdout);
    assert!(
        stdout.contains("first=first contents"),
        "first read: {}",
        stdout
    );
    assert!(stdout.contains("second_ok"), "second: {}", stdout);
    assert!(stdout.contains("second=second"), "second read: {}", stdout);

    // No leaked .tmp.* siblings in the same directory.
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    for name in &entries {
        assert!(
            !name.contains(".tmp."),
            "leaked tmp sibling `{}` in {:?}",
            name,
            entries
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// `fs::write_atomic` failure (unwritable parent directory): error is
/// surfaced AND no `.tmp.<pid>` file is left behind.
#[test]
fn fs_write_atomic_failure_leaves_no_tmp_file() {
    let dir = unique_tmp_dir("write_atomic_fail");
    // Path inside a directory that does not exist. open(O_CREAT) fails
    // with ENOENT for the missing parent — we want the implementation
    // to surface Err without creating a stray tmp.
    let bad_parent = dir.join("does").join("not").join("exist");
    let bad_path = bad_parent.join("config.toml");

    let source = rx("fs_write_atomic_unwritable_parent")
        .replace("__BAD_PATH__", &bad_path.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_write_atomic_fail");
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("err_ok"), "expected Err branch: {}", stdout);
    // The bad parent should never have been materialized.
    assert!(
        !bad_parent.exists(),
        "implementation accidentally created parent dir: {}",
        bad_parent.display()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `fs::read_link` round-trips with `fs::symlink`: create a link, then
/// read it back and assert the target string matches.
#[test]
fn fs_read_link_returns_target_path() {
    let dir = unique_tmp_dir("read_link");
    let target = dir.join("target.txt");
    std::fs::write(&target, b"x").expect("write target");
    let link = dir.join("link.txt");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");

    let source = rx("fs_read_link_round_trip").replace("__LINK__", &link.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_read_link");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    let expected = format!("target={}", target.display());
    assert!(
        stdout.contains(&expected),
        "expected `{}`; got: {}",
        expected,
        stdout
    );
}

/// `fs::symlink(target, link)` creates a symlink; `metadata` confirms
/// `is_symlink=true` (lstat semantics); `read_link` returns the original
/// target string verbatim (no canonicalization).
#[test]
fn fs_symlink_then_metadata_reports_symlink() {
    let dir = unique_tmp_dir("symlink");
    let target = dir.join("real.txt");
    std::fs::write(&target, b"hello symlink").expect("write");
    let link = dir.join("alias.txt");

    let source = rx("fs_symlink_then_metadata_and_read_link")
        .replace("__TARGET__", &target.display().to_string())
        .replace("__LINK__", &link.display().to_string());
    let (stdout, stderr, ok) = compile_and_run(&source, "stdlib_fs_symlink_then_metadata");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ok, "stderr: {}", stderr);
    assert!(stdout.contains("linked"), "symlink: {}", stdout);
    assert!(
        stdout.contains("is_symlink=true"),
        "metadata is_symlink: {}",
        stdout
    );
    let expected = format!("target={}", target.display());
    assert!(stdout.contains(&expected), "read_link: {}", stdout);
}

fn ruxen_unique_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
