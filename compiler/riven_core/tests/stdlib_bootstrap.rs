//! Pin tests for #06.8 Wave 1 Task 0b: the stdlib bootstrap loader.
//!
//! Scope of this commit: parse-only gate. `BOOTSTRAP_FILES` is empty,
//! so the production [`run_bootstrap`] path is a no-op. The tests
//! exercise [`run_bootstrap_with_files`] which takes an explicit
//! file list and an optional sysroot override — that is the surface
//! Wave 2 will hook through to load `iter.rvn` and `net.rvn` first.

use riven_core::diagnostics::Diagnostic;
use riven_core::resolve::bootstrap::{run_bootstrap, run_bootstrap_with_files, BOOTSTRAP_FILES};

fn write_fixture(dir: &std::path::Path, rel: &str, contents: &str) {
    let full = dir.join(rel);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("create fixture parent dir");
    }
    std::fs::write(&full, contents).expect("write fixture file");
}

/// Self-cleaning tempdir under `target/tmp/`. Rolled by hand to avoid
/// adding `tempfile` as a `riven_core` dev-dependency just for these
/// bootstrap tests. Each test gets a uniquely-named directory; the
/// `Drop` impl best-effort removes the tree.
struct TempDir {
    path: std::path::PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let mut base = std::env::temp_dir();
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // Append a per-call counter so two tests running in parallel
        // (the rust test harness threads them by default) never
        // collide on the same path.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        base.push(format!("riven-stdlib-bootstrap-{pid}-{nanos}-{n}"));
        std::fs::create_dir_all(&base).expect("create tempdir");
        TempDir { path: base }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn bootstrap_files_list_includes_smoke_and_wave_2_migrations() {
    // Wave 1.5 (#06.8 Phase 3) shipped a proof-of-life
    // `_bootstrap_smoke.rvn`. Wave 2 starts the actual stdlib
    // migration; `rand.rvn` is the first module to live in `.rvn`
    // form. The smoke file MUST stay first (the bootstrap_smoke_*
    // E2E tests depend on its `BootstrapSmokeClass.add_one` /
    // `bootstrap_smoke_add_one` FFI aliases) and the migrated
    // modules append after it. Pinning the contents here surfaces
    // any drive-by additive change.
    assert!(
        BOOTSTRAP_FILES.first() == Some(&"_bootstrap_smoke.rvn"),
        "BOOTSTRAP_FILES must start with the Wave-1.5 proof-of-life \
         file (E2E tests depend on it); got {:?}",
        BOOTSTRAP_FILES
    );
    assert!(
        BOOTSTRAP_FILES.contains(&"rand.rvn"),
        "BOOTSTRAP_FILES must include the Wave-2 migrated rand.rvn; \
         got {:?}",
        BOOTSTRAP_FILES
    );
}

#[test]
fn run_bootstrap_parses_all_listed_files_clean() {
    // Production `run_bootstrap` should parse every file in
    // BOOTSTRAP_FILES without emitting any diagnostics. If this
    // fails, a `library/std/src/*.rvn` file is missing or doesn't
    // parse — both fatal for the driver.
    let mut diags = Vec::<Diagnostic>::new();
    let programs = run_bootstrap(&mut diags);
    assert!(
        diags.is_empty(),
        "production run_bootstrap should emit zero diagnostics; got: {:?}",
        diags
    );
    assert_eq!(
        programs.len(),
        BOOTSTRAP_FILES.len(),
        "expected one parsed program per BOOTSTRAP_FILES entry; got \
         {} programs for {} files",
        programs.len(),
        BOOTSTRAP_FILES.len()
    );
    for (rel, program) in BOOTSTRAP_FILES.iter().zip(programs.iter()) {
        assert!(
            !program.items.is_empty(),
            "parsed stdlib file `{}` should contain at least one top-level item",
            rel
        );
    }
}

#[test]
fn bootstrap_failure_in_stdlib_file_has_file_line() {
    // Write a deliberately broken .rvn into a tempdir and verify the
    // emitted diagnostic carries E0725 with both the filename and a
    // non-zero line number. The exact line a parser settles on for a
    // given error is implementation detail; we only assert that the
    // cite is *informative* (file mentioned + line > 0).
    let tmp = TempDir::new();
    write_fixture(
        tmp.path(),
        "broken.rvn",
        "class Foo\n  def open(self) ->\n    # missing return type and body\nend\n",
    );

    let mut diags = Vec::<Diagnostic>::new();
    let _ = run_bootstrap_with_files(&["broken.rvn"], Some(tmp.path()), &mut diags);

    assert!(
        !diags.is_empty(),
        "broken stdlib fixture should produce at least one diagnostic"
    );
    let first = &diags[0];
    assert_eq!(
        first.code.as_deref(),
        Some("E0725"),
        "diagnostic code should be E0725; got {:?}",
        first.code
    );
    assert!(
        first.message.contains("library/std/src/broken.rvn"),
        "diagnostic should cite the stdlib file path; got: {}",
        first.message
    );
    assert!(
        first.span.line > 0 || first.message.contains(":"),
        "diagnostic should carry either a non-zero line on the span \
         or a `<file>:<line>` cite in the message; got line={} msg={}",
        first.span.line,
        first.message
    );
}

#[test]
fn bootstrap_missing_file_reports_e0725() {
    // A reference to a file the sysroot does not contain should fail
    // with E0725 too — same code, different cause, same docs page.
    let tmp = TempDir::new();

    let mut diags = Vec::<Diagnostic>::new();
    let _ = run_bootstrap_with_files(&["nope.rvn"], Some(tmp.path()), &mut diags);

    assert!(
        diags.iter().any(|d| d.code.as_deref() == Some("E0725")),
        "missing stdlib file should emit E0725; got: {:?}",
        diags
    );
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("library/std/src/nope.rvn")),
        "diagnostic should cite the missing file path; got: {:?}",
        diags.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

#[test]
fn bootstrap_clean_stdlib_file_returns_program() {
    // The happy path: a syntactically valid stdlib file parses
    // successfully and the loader returns a Program. Wave 2+ will
    // do real things with this program (merge into prelude scope);
    // Wave 1 just verifies the loader is wired through.
    let tmp = TempDir::new();
    write_fixture(
        tmp.path(),
        "ok.rvn",
        "# A no-op stdlib module — just enough to round-trip the parser.\ndef placeholder() -> Int\n  0\nend\n",
    );

    let mut diags = Vec::<Diagnostic>::new();
    let programs = run_bootstrap_with_files(&["ok.rvn"], Some(tmp.path()), &mut diags);
    assert!(
        diags.is_empty(),
        "clean stdlib file should not produce diagnostics; got: {:?}",
        diags
    );
    assert_eq!(programs.len(), 1, "expected one parsed Program");
    assert!(
        !programs[0].items.is_empty(),
        "parsed Program should contain the placeholder def"
    );
}
