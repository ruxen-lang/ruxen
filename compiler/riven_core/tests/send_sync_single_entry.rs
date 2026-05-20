//! B2 pin test for
//! `docs/specs/system/compiler_consolidation.spec.md` — Send/Sync
//! classification has ONE entry point.
//!
//! Pre-consolidation, `hir/types.rs` shipped both a loose
//! `is_send_with` (classes auto-derived by field walk) and a strict
//! `is_send_strict_with` (classes required `include Send`). Spec §B2
//! merged them: the strict semantics are canonical, the loose variant
//! never should have existed (spec
//! `docs/specs/ownership/send_sync_enforcement.spec.md` §B10 was
//! always the rule).
//!
//! This pin asserts the duplication is gone. If a future contributor
//! reintroduces `is_send_strict_with` or any `_strict_` variant, this
//! test fails and they have to converge on the single entry point.

use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn src_dir() -> PathBuf {
    workspace_root().join("compiler/riven_core/src")
}

fn walk_rs_files(dir: &PathBuf, files: &mut Vec<PathBuf>) {
    let read_dir = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs_files(&path, files);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

#[test]
fn no_strict_send_variant_remains_in_compiler_src() {
    let mut files = Vec::new();
    walk_rs_files(&src_dir(), &mut files);
    assert!(!files.is_empty(), "no .rs files found under compiler src");

    let mut offenders = Vec::new();
    for path in &files {
        let content = std::fs::read_to_string(path).expect("read");
        // A historical reference to `is_send_strict_with` inside a doc
        // comment explaining the consolidation is OK; an actual call
        // site or function definition is not.
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("///") || trimmed.starts_with("//") {
                continue;
            }
            if line.contains("is_send_strict_with") {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(workspace_root()).unwrap().display(),
                    lineno + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Send/Sync classification must have a single entry point. \
         Found references to the eliminated `is_send_strict_with` \
         variant outside doc comments. Use `is_send_with` (the \
         strict-mode canonical entry):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn is_send_with_is_defined_exactly_once() {
    // The body of `pub fn is_send_with` should appear at exactly one
    // location in `hir/types.rs`. If a future refactor accidentally
    // duplicates the signature, this catches it.
    let types_rs = src_dir().join("hir/types.rs");
    let content = std::fs::read_to_string(&types_rs).expect("read hir/types.rs");
    let count = content.matches("pub fn is_send_with(").count();
    assert_eq!(
        count, 1,
        "expected exactly one `pub fn is_send_with(` definition in \
         hir/types.rs; found {}",
        count
    );

    let inner_count = content.matches("fn is_send_with_inner(").count();
    assert_eq!(
        inner_count, 1,
        "expected exactly one `fn is_send_with_inner(` definition in \
         hir/types.rs; found {}",
        inner_count
    );
}

#[test]
fn is_sync_with_is_defined_exactly_once() {
    let types_rs = src_dir().join("hir/types.rs");
    let content = std::fs::read_to_string(&types_rs).expect("read hir/types.rs");
    let count = content.matches("pub fn is_sync_with(").count();
    assert_eq!(
        count, 1,
        "expected exactly one `pub fn is_sync_with(` definition in \
         hir/types.rs; found {}",
        count
    );

    let inner_count = content.matches("fn is_sync_with_inner(").count();
    assert_eq!(
        inner_count, 1,
        "expected exactly one `fn is_sync_with_inner(` definition in \
         hir/types.rs; found {}",
        inner_count
    );
}
