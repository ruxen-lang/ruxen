//! Q17 acceptance: a dependency's generic function over a mixin bound must
//! monomorphize for a type defined in the CONSUMING package — and for TWO
//! such consumer types in one program (proving real per-instantiation
//! monomorphization, not the old devirtualize-to-the-single-implementor
//! fast path that capped quiver's framework at one paint backend).
//!
//! Before Q17, a dependency's `def paint_all[T: Paintable](s: &var T, …)`
//! called with a consumer-defined `Paintable` implementor emitted the bound
//! placeholder callee `T: Paintable_fill_rect`, which link-fails. The single
//! implementor case was masked only because mixin dispatch devirtualized to
//! the sole impl. This forced quiver to keep exactly one `PaintSurface`
//! implementor (`RecordingSurface`) inside its own package.
//!
//! These tests stage an isolated install of the FRESHLY-BUILT
//! `CARGO_BIN_EXE_ruxen` (target/, never the installed `~/.ruxen` one),
//! scaffold a two-package layout — a `dep-paint` library exposing a
//! `mixin Paintable`, its own implementor `RecordingSurface`, and the generic
//! free function `paint_all`; and a `consumer` package that defines its OWN
//! second implementor and calls the dependency's generic against BOTH — then
//! COMPILE + RUN + assert exact stdout. This is a real codegen + link path,
//! so a duplicate-symbol / placeholder-symbol / devirtualize-to-one
//! regression fails the link or the stdout assertion, not just a type check.
//!
//! Mirrors `dep_visibility.rs`'s staged-install pattern (which see for the
//! OnceLock / ETXTBUSY rationale).

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

/// Shared, pre-staged install layout for all tests in this binary.
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

/// The dependency library: a `Paintable` mixin, the dep's OWN implementor
/// `RecordingSurface`, and the generic free functions `paint_all` (direct)
/// and `paint_twice` (generic-calling-generic — exercises the transitive
/// instantiation worklist).
const DEP_LIB_RX: &str = "\
mixin Paintable\n\
\x20 def fill_rect(w: Int, h: Int) -> Int\n\
end\n\
\n\
def paint_all[T: Paintable](s: &var T, w: Int, h: Int) -> Int\n\
\x20 s.fill_rect(w, h)\n\
end\n\
\n\
def paint_twice[T: Paintable](s: &var T, w: Int, h: Int) -> Int\n\
\x20 paint_all(s, w, h) + paint_all(s, w, h)\n\
end\n\
\n\
class RecordingSurface\n\
\x20 include Paintable\n\
\x20 area: Int\n\
\x20 def init; self.area = 0; end\n\
\x20 def var fill_rect(w: Int, h: Int) -> Int\n\
\x20\x20\x20 self.area = w * h\n\
\x20\x20\x20 self.area\n\
\x20 end\n\
end\n";

fn write_dep(root: &Path) {
    write(
        &root.join("dep-paint/Ruxen.toml"),
        "[package]\nname = \"dep-paint\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"library\"\n",
    );
    write(&root.join("dep-paint/src/lib.rx"), DEP_LIB_RX);
}

fn run_ruxen(ruxen: &Path, project: &Path, args: &[&str]) -> std::process::Output {
    Command::new(ruxen)
        .args(args)
        .current_dir(project)
        .output()
        .expect("spawn ruxen")
}

/// Consumer BINARY defines its own second implementor `TallySurface` and runs
/// the dependency's generic against BOTH the dep's `RecordingSurface` and the
/// consumer's `TallySurface`. The two implementors compute DIFFERENT values
/// (`w*h` vs `w+h`) so the asserted stdout proves the generic was specialized
/// per concrete type rather than devirtualized to one.
#[test]
fn consumer_binary_runs_dep_generic_over_two_implementors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ruxen = shared_install();
    write_dep(root);

    write(
        &root.join("app/Ruxen.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"binary\"\n\n\
         [dependencies]\ndep-paint = { path = \"../dep-paint\" }\n",
    );
    write(
        &root.join("app/src/main.rx"),
        "class TallySurface\n\
        \x20 include Paintable\n\
        \x20 total: Int\n\
        \x20 def init; self.total = 0; end\n\
        \x20 def var fill_rect(w: Int, h: Int) -> Int\n\
        \x20\x20\x20 self.total = w + h\n\
        \x20\x20\x20 self.total\n\
        \x20 end\n\
        end\n\
        \n\
        def main\n\
        \x20 var dep = RecordingSurface.new\n\
        \x20 var mine = TallySurface.new\n\
        \x20 let a = paint_all(&var dep, 4, 5)\n\
        \x20 let b = paint_all(&var mine, 4, 5)\n\
        \x20 puts \"dep=#{a} mine=#{b}\"\n\
        end\n",
    );

    let app = root.join("app");
    let build = run_ruxen(ruxen, &app, &["build"]);
    assert!(
        build.status.success(),
        "consumer binary `ruxen build` should monomorphize the dep generic for \
         the consumer's own implementor.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    // Run the produced binary and assert exact stdout: RecordingSurface
    // computes 4*5=20, TallySurface computes 4+5=9 — distinct values prove
    // real per-implementor monomorphization, not devirtualize-to-one.
    let bin = app.join("target/debug/app");
    let run = Command::new(&bin).output().expect("run app binary");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(
        run.status.success(),
        "app binary should run cleanly.\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stderr),
    );
    assert_eq!(
        stdout.trim(),
        "dep=20 mine=9",
        "each implementor must paint with its OWN body (20 = 4*5 for the dep's \
         RecordingSurface, 9 = 4+5 for the consumer's TallySurface)"
    );
}

/// `ruxen test` in the consumer: the same two-implementor shape exercised
/// through the test runner (which flat-merges the dep source and synthesises
/// a `def main`). Also covers the generic-calling-generic `paint_twice`.
#[test]
fn consumer_test_runs_dep_generic_over_two_implementors() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ruxen = shared_install();
    write_dep(root);

    write(
        &root.join("consumer/Ruxen.toml"),
        "[package]\nname = \"consumer\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"library\"\n\n\
         [dependencies]\ndep-paint = { path = \"../dep-paint\" }\n",
    );
    // A library symbol the test can call, plus the consumer's own implementor.
    write(
        &root.join("consumer/src/lib.rx"),
        "class TallySurface\n\
        \x20 include Paintable\n\
        \x20 total: Int\n\
        \x20 def init; self.total = 0; end\n\
        \x20 def var fill_rect(w: Int, h: Int) -> Int\n\
        \x20\x20\x20 self.total = w + h\n\
        \x20\x20\x20 self.total\n\
        \x20 end\n\
        end\n\
        \n\
        def dep_area -> Int\n\
        \x20 var s = RecordingSurface.new\n\
        \x20 paint_all(&var s, 4, 5)\n\
        end\n\
        \n\
        def consumer_area -> Int\n\
        \x20 var s = TallySurface.new\n\
        \x20 paint_all(&var s, 4, 5)\n\
        end\n\
        \n\
        def consumer_twice -> Int\n\
        \x20 var s = TallySurface.new\n\
        \x20 paint_twice(&var s, 4, 5)\n\
        end\n",
    );
    write(
        &root.join("consumer/tests/paint_test.rx"),
        "Tester.describe(\"cross-package mono\") do |t: &var Tester|\n\
        \x20 t.it(\"runs the dep generic over the dep's own implementor\") do\n\
        \x20\x20\x20 t.expect(dep_area()).to_eq(20)\n\
        \x20 end\n\
        \x20 t.it(\"monomorphizes the dep generic for a CONSUMER implementor\") do\n\
        \x20\x20\x20 t.expect(consumer_area()).to_eq(9)\n\
        \x20 end\n\
        \x20 t.it(\"handles generic-calling-generic for the consumer type\") do\n\
        \x20\x20\x20 t.expect(consumer_twice()).to_eq(18)\n\
        \x20 end\n\
        end\n",
    );

    let consumer = root.join("consumer");
    let out = run_ruxen(ruxen, &consumer, &["test"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`ruxen test` should compile + run tests that monomorphize the dep \
         generic for a consumer type.\nstdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        stdout.contains("passed"),
        "expected a passing test summary; got stdout:\n{stdout}\nstderr:\n{stderr}",
    );
    assert!(
        !stdout.contains("FAILED") && !stdout.contains("1 failed") && !stdout.contains("3 failed"),
        "no test should fail; got stdout:\n{stdout}\nstderr:\n{stderr}",
    );
}

/// The single-implementor shape (quiver's CURRENT acceptance bar) must keep
/// working: a dep generic over a mixin with exactly ONE implementor links and
/// runs. This pins that the new per-instantiation monomorphization does not
/// regress the devirtualize-equivalent single-impl path.
#[test]
fn single_implementor_still_links_and_runs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let ruxen = shared_install();
    write_dep(root);

    write(
        &root.join("app/Ruxen.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n\
         [build]\ntype = \"binary\"\n\n\
         [dependencies]\ndep-paint = { path = \"../dep-paint\" }\n",
    );
    // Only the dep's OWN implementor is used — no consumer implementor.
    write(
        &root.join("app/src/main.rx"),
        "def main\n\
        \x20 var s = RecordingSurface.new\n\
        \x20 let a = paint_all(&var s, 6, 7)\n\
        \x20 puts \"area=#{a}\"\n\
        end\n",
    );

    let app = root.join("app");
    let build = run_ruxen(ruxen, &app, &["build"]);
    assert!(
        build.status.success(),
        "single-implementor dep generic must still link.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );
    let bin = app.join("target/debug/app");
    let run = Command::new(&bin).output().expect("run app binary");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.success(), "single-impl app should run");
    assert_eq!(stdout.trim(), "area=42", "6*7 = 42 via RecordingSurface");
}
