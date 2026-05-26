//! Workspace end-to-end tests for `ruxen_cli`.
//!
//! Each test sets up a workspace under a `tempfile::TempDir`, calls
//! the relevant public entry points, and verifies the observable
//! contract. We never `cd` (the build entry points read
//! `std::env::current_dir()`, so we'd serialize all tests on a
//! global lock); instead every API takes an explicit `project_dir`
//! / `workspace_root`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ruxen_cli::manifest::{
    expand_workspace_members, find_workspace_root, Manifest,
};
use ruxen_cli::resolve_deps;

// ─── Fixture helpers ───────────────────────────────────────────────

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write");
}

/// Build a workspace with two members `pkg-a` and `pkg-b` (no
/// dependencies between them by default).
fn build_two_member_workspace(root: &Path) {
    write(
        &root.join("Ruxen.toml"),
        "[workspace]\nmembers = [\"pkg-a\", \"pkg-b\"]\n",
    );
    write(
        &root.join("pkg-a/Ruxen.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"0.1.0\"\n",
    );
    write(&root.join("pkg-a/src/main.rx"), "def main\nend\n");
    write(
        &root.join("pkg-b/Ruxen.toml"),
        "[package]\nname = \"pkg-b\"\nversion = \"0.1.0\"\n\n[build]\ntype = \"library\"\n",
    );
    write(&root.join("pkg-b/src/lib.rx"), "pub def hi\nend\n");
}

// ─── Tests ─────────────────────────────────────────────────────────

#[test]
fn workspace_root_detection_from_member_subdir() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_two_member_workspace(root);
    // Create a deep nested dir inside a member.
    let deep = root.join("pkg-a/src/foo/bar");
    fs::create_dir_all(&deep).unwrap();

    let found =
        find_workspace_root(&deep).expect("workspace root detected from deep member subdir");
    // Compare canonical paths because TempDir paths on macOS resolve
    // through /private/var/... when canonicalized — and any production
    // caller of `find_workspace_root` will likely have already
    // canonicalized through earlier filesystem ops.
    assert_eq!(found.canonicalize().unwrap(), root.canonicalize().unwrap());
}

#[test]
fn workspace_glob_expansion() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("Ruxen.toml"),
        "[workspace]\nmembers = [\"examples/*\"]\n",
    );
    for name in ["ex-one", "ex-two", "ex-three"] {
        write(
            &root.join(format!("examples/{}/Ruxen.toml", name)),
            &format!(
                "[package]\nname = \"{}\"\nversion = \"0.1.0\"\n",
                name
            ),
        );
        write(
            &root.join(format!("examples/{}/src/main.rx", name)),
            "def main\nend\n",
        );
    }
    // A non-package dir in the same parent — must be skipped (no
    // Ruxen.toml inside).
    fs::create_dir_all(root.join("examples/scratch")).unwrap();

    let members = expand_workspace_members(root, &["examples/*".to_string()])
        .expect("glob expansion succeeds");
    let names: Vec<&str> = members.iter().map(|(_, n)| n.as_str()).collect();
    // BTreeSet ordering inside the expander → alphabetical.
    assert_eq!(names, vec!["ex-one", "ex-three", "ex-two"]);
}

#[test]
fn workspace_member_intra_dep_resolves_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_two_member_workspace(root);
    // Have pkg-a depend on pkg-b by bare version — the intra-workspace
    // resolver should redirect to the sibling without needing path =
    // "../pkg-b".
    write(
        &root.join("pkg-a/Ruxen.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"0.1.0\"\n\n[dependencies]\npkg-b = \"0.1.0\"\n",
    );

    let pkg_a_dir = root.join("pkg-a");
    let manifest = Manifest::load(&pkg_a_dir).unwrap();

    let members = expand_workspace_members(root, &["pkg-a".to_string(), "pkg-b".to_string()])
        .expect("expand members");
    let mut workspace_map: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (dir, name) in members {
        if dir != pkg_a_dir {
            workspace_map.insert(name, dir);
        }
    }

    let result = resolve_deps::resolve_with_workspace(&pkg_a_dir, &manifest, None, &workspace_map)
        .expect("intra-workspace resolution succeeds");
    assert_eq!(result.deps.len(), 1);
    assert_eq!(result.deps[0].name, "pkg-b");
    assert!(
        result.deps[0].is_path,
        "intra-workspace dep is treated as a path-dep"
    );
    assert_eq!(
        result.deps[0].source_dir.canonicalize().unwrap(),
        root.join("pkg-b").canonicalize().unwrap()
    );
}

#[test]
fn workspace_shared_target_dir() {
    // Verifies the contract: `find_target_root(<member_dir>)` returns
    // the workspace root when one exists, so `target/` lands at the
    // workspace level instead of inside the member.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    build_two_member_workspace(root);

    let pkg_a_dir = root.join("pkg-a");
    let target_root = ruxen_cli::build::find_target_root(&pkg_a_dir);
    assert_eq!(
        target_root.canonicalize().unwrap(),
        root.canonicalize().unwrap(),
        "shared target lives at the workspace root, not inside the member"
    );

    // For a standalone (non-workspace) project, the target root is
    // the project itself.
    let standalone = tempfile::tempdir().unwrap();
    write(
        &standalone.path().join("Ruxen.toml"),
        "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n",
    );
    write(&standalone.path().join("src/main.rx"), "def main\nend\n");
    let target_root = ruxen_cli::build::find_target_root(standalone.path());
    assert_eq!(
        target_root.canonicalize().unwrap(),
        standalone.path().canonicalize().unwrap()
    );
}

#[test]
fn workspace_member_not_found_emits_e1600() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("Ruxen.toml"),
        "[workspace]\nmembers = [\"does-not-exist\"]\n",
    );

    let err = expand_workspace_members(root, &["does-not-exist".to_string()])
        .expect_err("missing member must error");
    assert!(
        err.contains("E1600"),
        "diagnostic must cite E1600, got: {}",
        err
    );
    assert!(
        err.contains("does-not-exist"),
        "diagnostic must name the missing member, got: {}",
        err
    );
}

#[test]
fn workspace_circular_path_dep_emits_e1601() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // pkg-a → pkg-b → pkg-a, both via intra-workspace name resolution.
    write(
        &root.join("Ruxen.toml"),
        "[workspace]\nmembers = [\"pkg-a\", \"pkg-b\"]\n",
    );
    write(
        &root.join("pkg-a/Ruxen.toml"),
        "[package]\nname = \"pkg-a\"\nversion = \"0.1.0\"\n\n[dependencies]\npkg-b = \"0.1.0\"\n",
    );
    write(&root.join("pkg-a/src/main.rx"), "def main\nend\n");
    write(
        &root.join("pkg-b/Ruxen.toml"),
        "[package]\nname = \"pkg-b\"\nversion = \"0.1.0\"\n\n[build]\ntype = \"library\"\n\n[dependencies]\npkg-a = \"0.1.0\"\n",
    );
    write(&root.join("pkg-b/src/lib.rx"), "pub def hi\nend\n");

    let pkg_a_dir = root.join("pkg-a");
    let manifest = Manifest::load(&pkg_a_dir).unwrap();

    let members = expand_workspace_members(root, &["pkg-a".to_string(), "pkg-b".to_string()])
        .expect("expand members");
    let mut workspace_map: BTreeMap<String, PathBuf> = BTreeMap::new();
    for (dir, name) in members {
        workspace_map.insert(name, dir);
    }
    // Don't strip self — we WANT the cycle detector to see the
    // round-trip back to pkg-a as an intra-workspace cycle.

    let err = resolve_deps::resolve_with_workspace(&pkg_a_dir, &manifest, None, &workspace_map)
        .expect_err("cycle must be reported");
    assert!(
        err.contains("E1601"),
        "expected E1601 in cycle diagnostic, got: {}",
        err
    );
    assert!(
        err.contains("Circular"),
        "expected 'Circular' wording, got: {}",
        err
    );
}
