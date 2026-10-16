//! Checks that the C runtime files `library/std/json` uses compile with
//! `-Wall -Wextra -Werror`. The package's behaviour tests live in
//! `library/std/json/tests/json_test.rx`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}", std::process::id(), n)
}

fn json_runtime_sources(root: &Path) -> Vec<PathBuf> {
    [
        "library/std/core/runtime/alloc.c",
        "library/std/string/runtime/string.c",
        "library/std/array/runtime/vec.c",
        "library/std/hash/runtime/hash.c",
        "library/std/json/runtime/json.c",
    ]
    .into_iter()
    .map(|rel| root.join(rel))
    .collect()
}

fn compile_objects(extra_flags: &[&str]) -> Vec<PathBuf> {
    let root = workspace_root();
    let mut objects = Vec::new();
    for src in json_runtime_sources(&root) {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("json");
        let obj = std::env::temp_dir().join(format!("ruxen_json_{stem}_{}.o", unique_suffix()));
        let mut cmd = Command::new("cc");
        cmd.arg("-c").arg(&src).arg("-o").arg(&obj);
        for flag in extra_flags {
            cmd.arg(flag);
        }
        let status = cmd.status().expect("invoke cc");
        assert!(
            status.success(),
            "failed to compile {} with flags {:?}",
            src.display(),
            extra_flags
        );
        objects.push(obj);
    }
    objects
}

#[test]
fn json_runtime_compiles_with_strict_warnings() {
    let objects = compile_objects(&["-O2", "-Wall", "-Wextra", "-Werror"]);
    for obj in &objects {
        let _ = std::fs::remove_file(obj);
    }
}

