//! Q32 acceptance: a flat-merged FFI dependency's C runtime (and its
//! `[system_libs]`) must reach the link line of executable-producing builds
//! (`ruxen test`, `ruxen build` binary) — not just the dependency's `.rx`
//! bodies (Q16).
//!
//! Before Q32, Q16 flat-merged a path-dependency's `src/**.rx` into the
//! consumer's TEST executable — including method bodies that call the dep's
//! `lib "C"` FFI symbols — but the dep's `runtime/**.c` objects and its
//! `[system_libs]` were never compiled / linked into that executable. Result:
//! `Undefined symbols: _ruxen_*` at link (the `ruxen_canvas_*` shape quiver hit).
//!
//! FIX (`ruxenc::test_runner` + `compile.rs` `--link-arg=` + `ruxen_cli::build`):
//! when flat-merging a dependency into an executable build, also compile each
//! dep's `runtime/**.c` and forward each dep's `[system_libs]` as `-l<lib>`,
//! mirroring exactly what `compile_project` does for a directly-declared dep.
//!
//! These tests stage an isolated install of the FRESHLY-BUILT
//! `CARGO_BIN_EXE_ruxen` and exercise REAL compile + link + RUN paths:
//!   1. an FFI dep (a `lib "C"` fn backed by a 5-line `runtime/shim.c`) used
//!      only via the consumer's `tests/**.rx` — `ruxen test` must LINK and the
//!      test must PASS (the returned value is asserted, so a broken link or a
//!      wrong-symbol resolution fails loudly);
//!   2. a binary that ALSO declares the same FFI dep directly — must build +
//!      run with NO duplicate-symbol error (the canvas/examples shape);
//!   3. a non-FFI dep (no `runtime/`, no `[system_libs]`) — `ruxen test` must
//!      still link and pass (no spurious flag / empty-runtime regression).
//!
//! Mirrors `dep_visibility.rs`'s staged-install + two-package-workspace shape.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn stdlib_root_src() -> PathBuf {
    workspace_root().join("library/std")
}

fn ruxen_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ruxen"))
}

/// Shared, pre-staged install layout for all tests in this binary (one stage,
/// see `dep_visibility.rs` / `installed_pkg_manager.rs` for the ETXTBUSY race).
fn shared_install() -> &'static Path {
    static INSTALL: OnceLock<(TempDir, PathBuf)> = OnceLock::new();
    &INSTALL
        .get_or_init(|| {
            let temp = tempfile::tempdir().expect("mktemp shared install");
            let bin_dir = temp.path().join("bin");
            let lib_dir = temp.path().join("lib");
            fs::create_dir_all(&bin_dir).unwrap();
            fs::create_dir_all(&lib_dir).unwrap();

            let staged = bin_dir.join("ruxen");
            fs::copy(ruxen_exe(), &staged).expect("copy ruxen");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&staged).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&staged, perms).unwrap();
            }

            let lib_std_dir = lib_dir.join("std");
            copy_runtime_tree(&stdlib_root_src(), &lib_std_dir);
            (temp, staged)
        })
        .1
}

fn copy_runtime_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_runtime_tree(&path, &dest);
        } else {
            fs::copy(&path, &dest).expect("copy runtime file");
        }
    }
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn run_ruxen(ruxen: &Path, project: &Path, args: &[&str]) -> std::process::Output {
    Command::new(ruxen)
        .args(args)
        .current_dir(project)
        .output()
        .expect("spawn ruxen")
}

/// Stage an FFI dependency: a `lib "C"`-style `runtime/shim.c` fn plus a
/// Ruxen wrapper, declaring an (empty) `[system_libs]` table like canvas does.
fn stage_ffi_dep(root: &Path) {
    write(
        &root.join("ffidep/Ruxen.toml"),
        "[package]\nname = \"ffidep\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"library\"\n\n\
         [system_libs]\nlibs = []\n",
    );
    write(
        &root.join("ffidep/src/lib.rx"),
        "lib \"runtime/shim.c\"\n  \
           def shim_answer as \"ruxen_shim_answer\"(x: Int) -> Int\nend\n\n\
         def answer_plus(x: Int) -> Int { shim_answer(x) + 1 }\n",
    );
    write(
        &root.join("ffidep/runtime/shim.c"),
        "#include <stdint.h>\n\
         int64_t ruxen_shim_answer(int64_t x) { return x * 10; }\n",
    );
}

/// A consumer LIBRARY that calls the FFI dep only through a `tests/**.rx` file.
fn stage_consumer(root: &Path) -> PathBuf {
    write(
        &root.join("consumer/Ruxen.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"library\"\n\n\
         [dependencies]\nffidep = { path = \"../ffidep\" }\n",
    );
    write(
        &root.join("consumer/src/lib.rx"),
        "def doubled(x: Int) -> Int { answer_plus(x) }\n",
    );
    write(
        &root.join("consumer/tests/ffi_test.rx"),
        "Tester.describe(\"ffi\") do |t: &var Tester|\n  \
           t.it(\"links + calls through the FFI dep runtime\") do\n    \
             t.expect(doubled(4)).to_eq(41)\n  \
           end\nend\n",
    );
    root.join("consumer")
}

/// `ruxen test` must compile the dep's `runtime/shim.c` into the test binary,
/// LINK successfully, and RUN — the assertion (4*10+1 == 41) confirms the right
/// C symbol resolved, not just that the link succeeded.
#[test]
fn test_links_flat_merged_ffi_dep_runtime() {
    let temp = tempfile::tempdir().unwrap();
    let ruxen = shared_install();
    stage_ffi_dep(temp.path());
    let consumer = stage_consumer(temp.path());

    let out = run_ruxen(ruxen, &consumer, &["test"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`ruxen test` should link + run a test calling through the FFI dep.\n\
         stdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        stdout.contains("1 passed") && !stdout.contains("1 failed"),
        "expected a single passing test; got stdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        !stderr.contains("Undefined symbols") && !stdout.contains("Undefined symbols"),
        "FFI symbol must resolve at link; got stderr:\n{stderr}",
    );
}

/// The canvas/examples shape: a BINARY that ALSO declares the FFI dep directly.
/// Must build + run with no duplicate-symbol error and the right value.
#[test]
fn binary_declaring_ffi_dep_directly_builds_and_runs() {
    let temp = tempfile::tempdir().unwrap();
    let ruxen = shared_install();
    stage_ffi_dep(temp.path());
    write(
        &temp.path().join("bincon/Ruxen.toml"),
        "[package]\nname = \"bincon\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"binary\"\n\n\
         [dependencies]\nffidep = { path = \"../ffidep\" }\n",
    );
    write(
        &temp.path().join("bincon/src/main.rx"),
        "def main\n  puts \"result=#{answer_plus(7)}\"\nend\n",
    );
    let bincon = temp.path().join("bincon");

    let build = run_ruxen(ruxen, &bincon, &["build"]);
    let bstdout = String::from_utf8_lossy(&build.stdout);
    let bstderr = String::from_utf8_lossy(&build.stderr);
    assert!(
        build.status.success(),
        "binary build with a directly-declared FFI dep must not duplicate-symbol.\n\
         stdout:\n{bstdout}\nstderr:\n{bstderr}",
    );

    let bin = bincon.join("target/debug/bincon");
    let run = Command::new(&bin).output().expect("run bincon");
    let rstdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success() && rstdout.contains("result=71"),
        "binary should print 7*10+1=71; got stdout:\n{rstdout}",
    );
}

/// A NON-FFI dep (no `runtime/`, no `[system_libs]`) must still link + pass
/// under `ruxen test` — guards against a spurious empty-flag / empty-runtime
/// regression from the Q32 dep-iteration loops.
#[test]
fn non_ffi_dep_test_still_links() {
    let temp = tempfile::tempdir().unwrap();
    let ruxen = shared_install();
    write(
        &temp.path().join("puredep/Ruxen.toml"),
        "[package]\nname = \"puredep\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"library\"\n",
    );
    write(
        &temp.path().join("puredep/src/lib.rx"),
        "def pure_double(x: Int) -> Int { x * 2 }\n",
    );
    write(
        &temp.path().join("pureconsumer/Ruxen.toml"),
        "[package]\nname = \"pureconsumer\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"library\"\n\n\
         [dependencies]\npuredep = { path = \"../puredep\" }\n",
    );
    write(
        &temp.path().join("pureconsumer/src/lib.rx"),
        "def quad(x: Int) -> Int { pure_double(pure_double(x)) }\n",
    );
    write(
        &temp.path().join("pureconsumer/tests/pure_test.rx"),
        "Tester.describe(\"pure\") do |t: &var Tester|\n  \
           t.it(\"uses a non-FFI dep symbol\") do\n    \
             t.expect(quad(3)).to_eq(12)\n  \
           end\nend\n",
    );
    let consumer = temp.path().join("pureconsumer");

    let out = run_ruxen(ruxen, &consumer, &["test"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && stdout.contains("1 passed") && !stdout.contains("1 failed"),
        "non-FFI dep `ruxen test` should still pass.\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
}
