//! `ruxen publish` end-to-end tests.
//!
//! Strategy: each test sets up
//!   * a bare repo in a tempdir (the "remote"),
//!   * a working repo in another tempdir whose `origin` points at the
//!     bare one,
//!   * a Ruxen.toml + tiny src/ tree inside the working repo,
//! and then calls `publish::publish(...)` directly. We never `cd` —
//! `publish` reads `std::env::current_dir()` only via
//! `find_project_root`, so each test sets cwd to its working repo
//! around the call. Because cargo runs tests on multiple threads,
//! changing cwd would corrupt sibling tests; we serialize publish
//! tests on a process-wide mutex.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use ruxen_cli::publish;

// One global mutex per integration-test binary. publish() reads cwd,
// so two parallel tests would race. Tests inside this file are the
// only callers, so serializing them here is sufficient.
static PUBLISH_LOCK: Mutex<()> = Mutex::new(());

fn run_git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("git {:?} failed to spawn: {}", args, e));
    assert!(status.success(), "git {:?} failed", args);
}

/// Initialise a bare repo at `path` and return it.
fn make_bare_remote(path: &Path) {
    fs::create_dir_all(path).unwrap();
    run_git(path, &["init", "--bare", "--quiet"]);
}

/// Build a fresh working repo with a Ruxen.toml + src/main.rx and
/// `origin` pointing at `bare_remote_path`.
fn make_working_repo(working: &Path, pkg_name: &str, version: &str, bare_remote_path: &Path) {
    fs::create_dir_all(working.join("src")).unwrap();
    fs::write(
        working.join("Ruxen.toml"),
        format!(
            "[package]\nname = \"{}\"\nversion = \"{}\"\n",
            pkg_name, version
        ),
    )
    .unwrap();
    fs::write(working.join("src/main.rx"), "def main\nend\n").unwrap();

    run_git(working, &["init", "--quiet"]);
    // Required for `git commit` in CI sandboxes that don't have a
    // default identity. Set per-repo so we don't touch global config.
    run_git(working, &["config", "user.email", "test@example.com"]);
    run_git(working, &["config", "user.name", "Test"]);
    run_git(
        working,
        &[
            "remote",
            "add",
            "origin",
            &bare_remote_path.to_string_lossy(),
        ],
    );
    run_git(working, &["add", "."]);
    run_git(working, &["commit", "-q", "-m", "init"]);
}

/// Save+set CWD inside the lock, returning a guard that restores it.
struct CwdGuard {
    prev: PathBuf,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl CwdGuard {
    fn enter(target: &Path) -> Self {
        let lock = PUBLISH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(target).unwrap();
        Self { prev, _lock: lock }
    }
}
impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.prev);
    }
}

// ─── Tests ─────────────────────────────────────────────────────────

#[test]
fn publish_dry_run_emits_tag_name_and_tarball() {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("remote.git");
    let working = tmp.path().join("work");
    make_bare_remote(&bare);
    make_working_repo(&working, "dry-pkg", "0.1.0", &bare);

    let _g = CwdGuard::enter(&working);
    publish::publish(true, None).expect("dry-run should succeed");

    // The tarball must exist after a dry-run (the spec says dry-run
    // emits the tarball + tag name; only the push is skipped).
    let tarball = working.join("dry-pkg-0.1.0.tar.gz");
    assert!(
        tarball.exists(),
        "expected tarball at {}",
        tarball.display()
    );

    // No tag should have been pushed.
    let output = Command::new("git")
        .current_dir(&bare)
        .args(["tag", "-l"])
        .output()
        .unwrap();
    assert!(
        output.stdout.is_empty(),
        "no tag should land on the remote in dry-run"
    );
}

#[test]
fn publish_dirty_worktree_refuses() {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("remote.git");
    let working = tmp.path().join("work");
    make_bare_remote(&bare);
    make_working_repo(&working, "dirty-pkg", "0.1.0", &bare);

    // Modify a tracked file without committing.
    fs::write(working.join("src/main.rx"), "def main\n  # change\nend\n").unwrap();

    let _g = CwdGuard::enter(&working);
    let err = publish::publish(false, None).expect_err("must refuse dirty worktree");
    assert!(
        err.contains("uncommitted") || err.contains("working tree"),
        "expected dirty-worktree diagnostic, got: {}",
        err
    );
}

#[test]
fn publish_to_local_bare_repo_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("remote.git");
    let working = tmp.path().join("work");
    make_bare_remote(&bare);
    make_working_repo(&working, "round-pkg", "0.1.0", &bare);

    {
        let _g = CwdGuard::enter(&working);
        publish::publish(false, None).expect("publish should land at the bare remote");
    }

    // Assert the tag landed in the bare repo.
    let output = Command::new("git")
        .current_dir(&bare)
        .args(["tag", "-l"])
        .output()
        .unwrap();
    let tags = String::from_utf8_lossy(&output.stdout);
    assert!(
        tags.lines().any(|t| t == "vround-pkg-0.1.0"),
        "expected `vround-pkg-0.1.0` in remote tags, got: {:?}",
        tags
    );
}

#[test]
fn publish_existing_tag_emits_e1602() {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("remote.git");
    let working = tmp.path().join("work");
    make_bare_remote(&bare);
    make_working_repo(&working, "dup-pkg", "0.1.0", &bare);

    // Pre-create the tag on the bare remote by pushing it from a
    // throwaway clone — `git tag` inside a bare repo would need
    // --update-ref plumbing; the clone+push path mirrors what a real
    // first publish would have done.
    let preclone = tmp.path().join("preclone");
    run_git(tmp.path(), &["clone", "--quiet", &bare.to_string_lossy(), &preclone.to_string_lossy()]);
    run_git(&preclone, &["config", "user.email", "test@example.com"]);
    run_git(&preclone, &["config", "user.name", "Test"]);
    // The clone of an empty bare repo has no HEAD; create one commit.
    fs::write(preclone.join("seed.txt"), "seed").unwrap();
    run_git(&preclone, &["add", "."]);
    run_git(&preclone, &["commit", "-q", "-m", "seed"]);
    run_git(&preclone, &["tag", "vdup-pkg-0.1.0"]);
    run_git(&preclone, &["push", "origin", "HEAD:refs/heads/main"]);
    run_git(&preclone, &["push", "origin", "vdup-pkg-0.1.0"]);

    let _g = CwdGuard::enter(&working);
    let err = publish::publish(false, None).expect_err("must refuse duplicate tag");
    assert!(
        err.contains("E1602"),
        "expected E1602 diagnostic, got: {}",
        err
    );
    assert!(
        err.contains("vdup-pkg-0.1.0"),
        "diagnostic must name the conflicting tag, got: {}",
        err
    );
}
