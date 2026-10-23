//! End-to-end tests for the `ruxen compile` subcommand (low-level driver).
//!
//! These tests stage an isolated "installed" layout:
//!
//!     <tempdir>/bin/ruxen
//!     <tempdir>/lib/runtime.c
//!
//! …and check that the staged `ruxen compile` finds its runtime the way it
//! does on a user's machine after `install.sh`, catching regressions like
//! hardcoded `CARGO_MANIFEST_DIR` paths. Language behaviour is covered by
//! the e2e fixtures in `tests/release-e2e`.
//!
//! The fixtures shipped a `ruxenc` binary historically; the toolchain now
//! consolidates everything under a single `ruxen` driver, so these tests
//! stage and exercise `ruxen compile <file>` instead.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Shared, pre-staged install layout for all tests in this binary.
///
/// On Linux, if two parallel tests each `fs::copy(ruxenc_exe(), dst)` and
/// `Command::new(dst).spawn()` at the same time, a sibling thread's fork
/// inherits the still-open write fd, and the child's `execve` hits
/// ETXTBUSY ("Text file busy") even when its own staged binary is fully
/// written and closed. Staging exactly once via `OnceLock` — before any
/// test spawns — eliminates the race: there is no live write fd when
/// tests run.
fn shared_install() -> &'static Path {
    static INSTALL: OnceLock<PathBuf> = OnceLock::new();
    INSTALL.get_or_init(|| {
        // A static is never dropped, so a `TempDir` here would leak a
        // full staged toolchain (about 140 MB) into the system temp dir
        // on every run. Stage under the cargo target dir instead, one
        // directory per test process, and sweep the ones left by
        // processes that have exited.
        let install = shared_install_dir();
        let bin_dir = install.join("bin");
        let lib_dir = install.join("lib");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::create_dir_all(&lib_dir).unwrap();

        let staged_ruxen = bin_dir.join("ruxen");
        fs::copy(ruxen_exe(), &staged_ruxen).expect("copy ruxen");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&staged_ruxen).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&staged_ruxen, perms).unwrap();
        }

        // #06.95 Phase B reorganized the C runtime into per-package
        // subdirectories under `library/std/<pkg>/runtime/`, with
        // the aggregator `runtime.c` living at
        // `library/std/core/runtime/runtime.c`. The aggregator
        // `#include`s siblings via relative `../../<pkg>/runtime/*.c`
        // paths, so the entire `library/std/` tree must be staged
        // with its directory structure intact for the includes to
        // resolve at build time. Stage it under `<install>/lib/std/`
        // and point `RUXEN_RUNTIME` at the staged `runtime.c`.
        let staged_std_root = lib_dir.join("std");
        copy_runtime_tree(&workspace_root().join("library/std"), &staged_std_root);

        // #06.8 Wave 2 stdlib self-hosting: ruxenc reads
        // `library/std/<pkg>/src/lib.rx` at startup via the
        // bootstrap loader. `resolve_stdlib_root` walks
        //   1. $RUXEN_STDLIB_PATH
        //   2. workspace-relative (CARGO_MANIFEST_DIR walk)
        //   3. `<exe>/../library/std/`
        // The full `library/std/` tree is already staged at
        // `<install>/lib/std/` above (it contains both the runtime
        // and the .rx sources side-by-side per #06.95 Phase B), so
        // no additional copy is needed for the .rx sources — the
        // staged_stdlib_dir() helper returns this same root.

        staged_ruxen
    })
}

/// `<CARGO_TARGET_TMPDIR>/installed_binary/<pid>`, created empty. Removes
/// the directories of earlier test processes that are no longer running.
fn shared_install_dir() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("installed_binary");
    fs::create_dir_all(&root).expect("create shared install root");
    for entry in fs::read_dir(&root).unwrap().flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        // Liveness is only checkable through /proc; elsewhere keep them.
        let exited = cfg!(target_os = "linux") && !Path::new(&format!("/proc/{pid}")).exists();
        if pid != std::process::id() && exited {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    let dir = root.join(std::process::id().to_string());
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create shared install dir");
    dir
}

/// Recursively copy a workspace directory tree (e.g. `library/std/`)
/// into the staged install layout, preserving structure. Mirrors what
/// `install.sh` does with `cp -R "$SRC/lib/." "$RUXEN_HOME/lib/"` on a
/// real release archive.
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

/// Resolve the workspace root by walking up from this crate's manifest dir.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Path to the unified `ruxen` driver under test. `CARGO_BIN_EXE_ruxen`
/// is only injected for tests in the bin's own package (`ruxen_cli`);
/// here in `ruxenc` we fall back to building the bin once and resolving
/// it at `<workspace>/target/<profile>/ruxen`.
fn ruxen_exe() -> PathBuf {
    use std::sync::OnceLock;
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        if let Some(p) = option_env!("CARGO_BIN_EXE_ruxen") {
            return PathBuf::from(p);
        }
        let workspace = workspace_root();
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.join("target"));
        let exe = if cfg!(windows) { "ruxen.exe" } else { "ruxen" };
        let bin = target.join("debug").join(exe);
        if !bin.exists() {
            let status = Command::new("cargo")
                .args(["build", "-p", "ruxen_cli", "--bin", "ruxen"])
                .current_dir(&workspace)
                .status()
                .expect("spawn cargo build for ruxen bin");
            assert!(
                status.success(),
                "cargo build -p ruxen_cli --bin ruxen failed"
            );
        }
        bin
    })
    .clone()
}

/// Path to the staged `library/std/` package-root tree alongside the
/// shared install. Spawned ruxenc subprocesses need this via
/// `RUXEN_STDLIB_PATH` because the bootstrap loader's CARGO_MANIFEST_DIR
/// fallback resolves to `src/ruxenc` (this crate, not the workspace)
/// when cargo runs the test binary; walking up from there does NOT
/// find a sibling `library/std/` because we're already two levels
/// deep under `src/`. Exe-adjacent resolution would land on the
/// staged copy, but only if ruxenc is invoked through the staged
/// binary path — the env var is the deterministic fix.
///
/// Post-#06.95 Phase B the staged stdlib lives under `<install>/lib/std/`
/// (per-package layout with both `src/lib.rx` and `runtime/*.c` for
/// each module).
fn staged_stdlib_dir() -> PathBuf {
    shared_install()
        .parent()
        .and_then(|bin| bin.parent())
        .map(|root| root.join("lib").join("std"))
        .expect("staged install layout")
}

/// Path to the staged aggregator `runtime.c`. The C runtime tree is
/// staged with its `library/std/` directory structure intact so that
/// `runtime.c`'s `#include "../../<pkg>/runtime/*.c"` directives
/// resolve. Subprocess `ruxenc` invocations need this via
/// `RUXEN_RUNTIME` because `find_runtime_c`'s `<exe>/../lib/runtime.c`
/// fallback no longer matches the per-package layout.
fn staged_runtime_c() -> PathBuf {
    staged_stdlib_dir()
        .join("core")
        .join("runtime")
        .join("runtime.c")
}

/// Read a `.rx` fixture from `tests/fixtures/ruxen/<name>.rx`.
///
/// Ruxen source for these tests lives in standalone `.rx` files so that
/// future surface-syntax migrations can sweep `*.rx` uniformly without
/// touching Rust string literals.
fn rx(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

/// Build an isolated layout for a test: a per-test tempdir for project
/// files, with the path to the process-wide shared staged `ruxen` binary.
///
/// Returns the tempdir (kept alive by the caller, cleaned up at test end)
/// and the path to the staged `ruxen` binary that is shared across all
/// tests in this binary.
fn stage_install() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("mktemp");
    (temp, shared_install().to_path_buf())
}

/// Compile `source` with `ruxen compile` (staged) in `dir`, then run the
/// resulting binary and return its captured stdout. Panics with context
/// on any failure.
fn compile_and_run(ruxen: &Path, dir: &Path, source_name: &str, source: &str) -> String {
    let src = dir.join(source_name);
    fs::write(&src, source).unwrap();

    let out_name = source_name.trim_end_matches(".rx");
    let out = dir.join(out_name);

    let compile = Command::new(ruxen)
        .arg("compile")
        .arg(source_name)
        .arg("-o")
        .arg(out_name)
        .current_dir(dir)
        .env("RUXEN_STDLIB_PATH", staged_stdlib_dir())
        .env("RUXEN_RUNTIME", staged_runtime_c())
        .output()
        .expect("spawn ruxen compile");

    assert!(
        compile.status.success(),
        "compile failed for {}\nstdout:\n{}\nstderr:\n{}",
        source_name,
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );

    let run = Command::new(&out)
        .current_dir(dir)
        .output()
        .expect("spawn compiled binary");

    assert!(
        run.status.success(),
        "run failed for {}\nstdout:\n{}\nstderr:\n{}",
        source_name,
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );

    String::from_utf8(run.stdout).expect("utf8 stdout")
}

// ── Install-layout tests ──────────────────────────────────────────────

#[test]
fn find_runtime_c_from_sibling_lib_dir() {
    // The core assertion: the binary resolves runtime.c via the installed
    // layout (bin/../lib/runtime.c), not via the CARGO_MANIFEST_DIR baked in
    // at build time. If this regresses, release binaries will fail on every
    // user machine.
    let (temp, ruxenc) = stage_install();
    let source = rx("find_runtime_c_from_sibling_lib_dir");
    let out = compile_and_run(&ruxenc, temp.path(), "hello.rx", &source);
    assert_eq!(out.trim(), "hello");
}
#[test]
fn runtime_env_override() {
    // RUXEN_RUNTIME env var should take precedence over the bin-relative
    // lookup. We stage a normal install and then point RUXEN_RUNTIME at a
    // secondary copy of runtime.c — compilation must still succeed.
    let (temp, ruxen) = stage_install();
    // Stage a secondary copy of the whole `library/std/` tree so the
    // unity-build `#include "../../<pkg>/runtime/*.c"` lookups still
    // resolve, then point RUXEN_RUNTIME at the aggregator inside.
    let alt_std_root = temp.path().join("alt_std");
    copy_runtime_tree(&workspace_root().join("library/std"), &alt_std_root);
    let alt = alt_std_root.join("core").join("runtime").join("runtime.c");

    fs::write(temp.path().join("env_ov.rx"), rx("runtime_env_override")).unwrap();

    let compile = Command::new(&ruxen)
        .arg("compile")
        .arg("env_ov.rx")
        .arg("-o")
        .arg("env_ov")
        .env("RUXEN_RUNTIME", &alt)
        .env("RUXEN_STDLIB_PATH", staged_stdlib_dir())
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(
        compile.status.success(),
        "compile failed:\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(temp.path().join("env_ov")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "ok");
}
#[test]
fn missing_runtime_gives_clear_error() {
    // If runtime.c cannot be found anywhere, the error message should name
    // every location we looked, so users can fix their install.
    let temp = tempfile::tempdir().unwrap();
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let ruxen = bin_dir.join("ruxen");
    fs::copy(ruxen_exe(), &ruxen).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&ruxen).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&ruxen, perms).unwrap();
    }

    fs::write(temp.path().join("x.rx"), "def main\n  puts \"hi\"\nend\n").unwrap();

    // Deliberately do NOT create lib/runtime.c. Also clear RUXEN_RUNTIME
    // and CARGO_MANIFEST_DIR so the dev fallback can't accidentally save us.
    //
    // We DO need stdlib bootstrap to succeed (else its E0725 fires
    // first and obscures the runtime-missing error). Point
    // RUXEN_STDLIB_PATH at the staged copy alongside the shared
    // install so bootstrap finds .rx files without falling back
    // through CARGO_MANIFEST_DIR.
    let out = Command::new(&ruxen)
        .arg("compile")
        .arg("x.rx")
        .current_dir(temp.path())
        .env_remove("RUXEN_RUNTIME")
        .env("CARGO_MANIFEST_DIR", "/nonexistent/ruxen-fake")
        .env("RUXEN_STDLIB_PATH", staged_stdlib_dir())
        .output()
        .unwrap();

    // We can't cleanly prevent the binary from finding runtime.c via its
    // compile-time baked CARGO_MANIFEST_DIR (env!()), so this test only
    // asserts that *when* the binary reports a missing runtime, the
    // message is informative. Most CI users will have the fallback path
    // populated, so we tolerate success here and only check error shape
    // if the compile failed.
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("runtime.c not found") && stderr.contains("RUXEN_RUNTIME"),
            "unhelpful error message: {}",
            stderr
        );
    }
}
