//! Integration tests for `riven explain ECODE` long-form markdown
//! explanations (T5.04 phase 3).
//!
//! Phase 2 only printed the title from the central registry; phase 3
//! extends `riven explain` to print embedded markdown content (Why /
//! Example / Fix sections) for every registered code.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn riven_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_riven"))
}

fn errors_dir() -> PathBuf {
    workspace_root().join("docs/errors")
}

/// Read every registered code by parsing the source of
/// `diagnostics/codes.rs`. We use a textual scrape to avoid pulling
/// `riven-core` into the dev-dependency closure for tests that don't
/// otherwise need it (the CLI already depends on riven-core, but
/// integration tests run as a separate binary).
fn registered_codes() -> Vec<String> {
    let path = workspace_root().join("compiler/riven_core/src/diagnostics/codes.rs");
    let source = std::fs::read_to_string(&path).expect("read codes.rs");
    let mut codes = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("code: \"") {
            if let Some(end) = rest.find('"') {
                codes.push(rest[..end].to_string());
            }
        }
    }
    assert!(!codes.is_empty(), "scraped zero codes from registry");
    codes
}

fn run_explain(code: &str) -> (bool, String, String) {
    let output = Command::new(riven_exe())
        .args(["explain", code])
        .output()
        .expect("invoke riven explain");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn explain_e0001_prints_long_form_sections() {
    let (ok, stdout, stderr) = run_explain("E0001");
    assert!(ok, "riven explain E0001 failed; stderr: {stderr}");
    assert!(
        stdout.contains("E0001"),
        "expected code in stdout, got: {stdout}"
    );
    assert!(
        stdout.contains("## Why"),
        "missing '## Why' section in stdout: {stdout}"
    );
    assert!(
        stdout.contains("## Example"),
        "missing '## Example' section in stdout: {stdout}"
    );
    assert!(
        stdout.contains("## Fix"),
        "missing '## Fix' section in stdout: {stdout}"
    );
}

#[test]
fn explain_e1011_prints_long_form_sections() {
    let (ok, stdout, _stderr) = run_explain("E1011");
    assert!(ok);
    assert!(stdout.contains("## Why"));
    assert!(stdout.contains("## Example"));
    assert!(stdout.contains("## Fix"));
}

#[test]
fn every_registered_code_has_a_markdown_file() {
    let dir = errors_dir();
    let mut missing = Vec::new();
    for code in registered_codes() {
        let path: &Path = &dir.join(format!("{code}.md"));
        if !path.exists() {
            missing.push(code);
        }
    }
    assert!(
        missing.is_empty(),
        "missing docs/errors/<code>.md for: {missing:?}"
    );
}

#[test]
fn every_markdown_file_contains_required_sections() {
    let dir = errors_dir();
    for code in registered_codes() {
        let path = dir.join(format!("{code}.md"));
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
        // Two doc-template generations coexist:
        // - Original (E0001-E0712, E10*): `## Why` / `## Example` / `## Fix`
        // - #06.5+ (E0714, E0722-E0725): `## Summary` /
        //   `## Common causes` / `## How to fix`
        // Either set of three section headers satisfies the contract.
        let original = ["## Why", "## Example", "## Fix"]
            .iter()
            .all(|h| body.contains(h));
        let modern = ["## Summary", "## Common causes", "## How to fix"]
            .iter()
            .all(|h| body.contains(h));
        assert!(
            original || modern,
            "{} missing required section triad — needs either \
             '## Why' / '## Example' / '## Fix' (original template) \
             or '## Summary' / '## Common causes' / '## How to fix' \
             (#06.5+ template)",
            path.display(),
        );
        // Title line is `# <code>: <title>` (original template) or
        // `# <code> — <title>` (#06.5+ template).
        let first_line = body.lines().next().unwrap_or("");
        let title_ok = first_line.starts_with(&format!("# {code}:"))
            || first_line.starts_with(&format!("# {code} —"))
            || first_line.starts_with(&format!("# {code} -"));
        assert!(
            title_ok,
            "{} first line should be '# {}: ...' or '# {} — ...', got: {:?}",
            path.display(),
            code,
            code,
            first_line
        );
    }
}

#[test]
fn explain_unknown_code_returns_nonzero() {
    let (ok, _stdout, _stderr) = run_explain("E9999");
    assert!(!ok, "expected nonzero exit for unknown code");
}
