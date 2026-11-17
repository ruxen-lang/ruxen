//! Every workspace crate builds its tests with `autotests = false`, so cargo
//! no longer finds test files by itself. This check fails when a
//! `tests/it/*.rs` file has no `mod` line in its `main.rs`, or a
//! `tests/*.rs` file has no `[[test]]` entry, since either would never be
//! compiled or run.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rs_stems(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut stems: Vec<String> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "rs"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    stems.sort();
    stems
}

#[test]
fn every_test_file_is_declared() {
    let root = workspace_root();
    let mut problems = Vec::new();
    let mut checked = 0;
    for parent in ["compiler", "src"] {
        let Ok(crates) = fs::read_dir(root.join(parent)) else {
            continue;
        };
        for krate in crates.filter_map(|e| e.ok().map(|e| e.path())) {
            let Ok(manifest) = fs::read_to_string(krate.join("Cargo.toml")) else {
                continue;
            };
            if !manifest.contains("autotests = false") {
                continue;
            }
            checked += 1;
            let tests = krate.join("tests");
            for stem in rs_stems(&tests) {
                if !manifest.contains(&format!("path = \"tests/{stem}.rs\"")) {
                    problems.push(format!(
                        "{}: no [[test]] entry",
                        tests.join(format!("{stem}.rs")).display()
                    ));
                }
            }
            let it = tests.join("it");
            let main = fs::read_to_string(it.join("main.rs")).unwrap_or_default();
            for stem in rs_stems(&it).into_iter().filter(|s| s != "main") {
                if !main.lines().any(|l| l.trim() == format!("mod {stem};")) {
                    problems.push(format!(
                        "{}: no `mod {stem};` in main.rs",
                        it.join(format!("{stem}.rs")).display()
                    ));
                }
            }
        }
    }
    assert!(
        checked > 0,
        "found no crate with `autotests = false` under {}",
        root.display()
    );
    assert!(
        problems.is_empty(),
        "undeclared test files:\n{}",
        problems.join("\n")
    );
}
