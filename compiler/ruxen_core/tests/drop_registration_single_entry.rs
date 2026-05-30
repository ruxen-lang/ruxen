//! B4 pin test for
//! `docs/specs/system/compiler_consolidation.spec.md` — user-Drop
//! registration has ONE entry point.
//!
//! Background: historically there was a temptation to fork the
//! `def drop` discovery walk into a parallel path keyed on hardcoded
//! class names (the trio-leak symptom — see commit b891712 and memory
//! note `project-ruxen-drop-name-mismatch`). Phase D-6 of #06.95
//! collapsed that into a single generic walker:
//! `Lowerer::collect_user_drop_classes` in
//! `compiler/ruxen_core/src/mir/lower/collect.rs`. The downstream
//! drop-elaboration pass in `mir/lower/drops.rs` consults exactly the
//! `user_drop_classes` set this function populates.
//!
//! This pin asserts the registration walker is defined in exactly one
//! place. If a future refactor accidentally introduces a sibling
//! collector (e.g. a hardcoded `seed_drop_classes(...)` helper), this
//! test fails.

use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn src_dir() -> PathBuf {
    workspace_root().join("compiler/ruxen_core/src")
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
fn collect_user_drop_classes_is_defined_exactly_once() {
    let mut files = Vec::new();
    walk_rs_files(&src_dir(), &mut files);
    assert!(!files.is_empty(), "no .rs files found under compiler src");

    let mut definition_sites = Vec::new();
    for path in &files {
        let content = std::fs::read_to_string(path).expect("read");
        for (lineno, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("///") || trimmed.starts_with("//") {
                continue;
            }
            // Match the `fn collect_user_drop_classes(` signature; do
            // not count call sites (those have `self.collect_…(…)` or
            // `Lowerer::collect_…`).
            if line.contains("fn collect_user_drop_classes(") {
                definition_sites.push(format!(
                    "{}:{}",
                    path.strip_prefix(workspace_root()).unwrap().display(),
                    lineno + 1
                ));
            }
        }
    }
    assert_eq!(
        definition_sites.len(),
        1,
        "expected exactly one `fn collect_user_drop_classes` definition. \
         Found: {:?}. If you split the registration walk, fold the new \
         source back into `class_has_drop_method` in \
         compiler/ruxen_core/src/mir/lower/collect.rs",
        definition_sites
    );
}

#[test]
fn no_parallel_drop_registration_helpers() {
    // Catch the trio-leak symptom — a helper that seeds drop classes
    // from a hardcoded list bypassing the single entry. A function
    // named anything like `seed_drop_classes`, `hardcoded_drop_…`,
    // `register_builtin_drop_…` is a regression.
    let mut files = Vec::new();
    walk_rs_files(&src_dir(), &mut files);
    let forbidden_names = [
        "seed_drop_classes",
        "hardcoded_drop_class",
        "register_builtin_drop",
        "init_drop_classes_hardcoded",
    ];
    let mut offenders = Vec::new();
    for path in &files {
        let content = std::fs::read_to_string(path).expect("read");
        for (lineno, line) in content.lines().enumerate() {
            for name in &forbidden_names {
                if line.contains(name) {
                    offenders.push(format!(
                        "{}:{}: {}",
                        path.strip_prefix(workspace_root()).unwrap().display(),
                        lineno + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Drop registration must flow through `collect_user_drop_classes` only:\n{}",
        offenders.join("\n")
    );
}
