//! B4 pin tests — no `def __drop` remains in stdlib `.rx` files.
//!
//! `mir/lower/collect.rs::collect_user_drop_classes` matches the
//! literal method name `drop`. Any stdlib `.rx` class declaring its
//! Drop hook as `def __drop as "..."` would be silently skipped — the
//! C destructor never fires and the class leaks its heap on scope
//! exit. The historical bite (commit `26e6daf` multithreading round)
//! shipped nine `def __drop` lib decls across `library/std/sync/src/
//! lib.rx`; B4 of `docs/specs/system/zero_rust_stdlib_classes.spec.md`
//! sweeps them to `def drop`.
//!
//! This pin scans every `library/std/*/src/lib.rx` for `def __drop`
//! occurrences and fails if any remain.

use std::fs;
use std::path::{Path, PathBuf};

fn stdlib_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("library")
        .join("std")
}

fn walk_lib_rxs(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(it) => it,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Look for `<pkg>/src/lib.rx`.
            let candidate = path.join("src").join("lib.rx");
            if candidate.is_file() {
                out.push(candidate);
            }
        }
    }
}

#[test]
fn no_double_underscore_drop_remains_in_stdlib() {
    let root = stdlib_root();
    assert!(
        root.is_dir(),
        "stdlib root missing at {} — did the workspace layout move?",
        root.display(),
    );

    let mut lib_files: Vec<PathBuf> = Vec::new();
    walk_lib_rxs(&root, &mut lib_files);
    assert!(
        !lib_files.is_empty(),
        "no library/std/*/src/lib.rx files found under {}",
        root.display(),
    );

    let mut offenders: Vec<(PathBuf, usize, String)> = Vec::new();
    for file in &lib_files {
        let contents = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for (idx, line) in contents.lines().enumerate() {
            // Skip comments — `#` and `##` are line-comments in Ruxen.
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue;
            }
            if trimmed.contains("def __drop") {
                offenders.push((file.clone(), idx + 1, line.to_string()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "stdlib `.rx` files must declare Drop hooks as `def drop as \"...\"`, never `def __drop` \
         (the MIR drop-class collector matches `drop`, not `__drop` — see B4 of \
         docs/specs/system/zero_rust_stdlib_classes.spec.md). Offenders:\n{}",
        offenders
            .iter()
            .map(|(p, line, txt)| format!("  {}:{} → {}", p.display(), line, txt.trim()))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}
