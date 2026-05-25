//! P0.5 verification: compile every release-e2e fixture with the workspace
//! `ruxenc` binary and report compile-fail / output-mismatch fixtures.
//!
//! This is intentionally minimal — the canonical e2e harness lives in
//! tests/release-e2e/run.sh and is the source of truth in CI. This test
//! gives us a way to drive the same fixture list from inside `cargo test`,
//! which is the only thing the constrained P0.5 worktree sandbox can run.

use std::path::PathBuf;
use std::process::Command;

#[test]
#[ignore] // run explicitly: `cargo test -p ruxenc --test p05_e2e_check -- --ignored --nocapture`
fn release_e2e_fixtures_compile_and_match() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest.parent().unwrap().parent().unwrap();
    let ruxenc = workspace.join("target").join("release").join("ruxenc");
    assert!(
        ruxenc.exists(),
        "ruxenc release binary not built at {}",
        ruxenc.display()
    );
    let cases = workspace.join("tests").join("release-e2e").join("cases");
    let expected_dir = workspace.join("tests").join("release-e2e").join("expected");
    let tmp = tempfile::tempdir().unwrap();

    let mut entries: Vec<_> = std::fs::read_dir(&cases)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "rx").unwrap_or(false))
        .collect();
    entries.sort_by_key(|e| e.path());

    let mut pass = 0usize;
    let mut compile_fails: Vec<String> = Vec::new();
    let mut output_fails: Vec<String> = Vec::new();

    for entry in entries {
        let src = entry.path();
        let base = src.file_stem().unwrap().to_string_lossy().to_string();
        let bin = tmp.path().join(format!("{base}.bin"));

        let compile = Command::new(&ruxenc)
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("ruxenc spawn");

        if !compile.status.success() {
            compile_fails.push(format!(
                "{base}: {}",
                String::from_utf8_lossy(&compile.stderr)
                    .lines()
                    .next()
                    .unwrap_or("")
            ));
            continue;
        }

        let expected_path = expected_dir.join(format!("{base}.out"));
        if !expected_path.exists() {
            pass += 1;
            continue;
        }

        let run = Command::new(&bin).output().expect("run spawn");
        let actual = String::from_utf8_lossy(&run.stdout).to_string();
        let expected = std::fs::read_to_string(&expected_path).unwrap();
        if actual.trim_end() == expected.trim_end() {
            pass += 1;
        } else {
            output_fails.push(base);
        }
    }

    eprintln!("PASS={pass}");
    eprintln!("COMPILE_FAILS ({}):", compile_fails.len());
    for f in &compile_fails {
        eprintln!("  {f}");
    }
    eprintln!("OUTPUT_FAILS ({}):", output_fails.len());
    for f in &output_fails {
        eprintln!("  {f}");
    }
    // Don't assert — this is a probe. Caller reads the output.
}
