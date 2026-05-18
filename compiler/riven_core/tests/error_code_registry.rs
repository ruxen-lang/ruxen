//! Compile-time-style coverage check for the error-code registry
//! (T5.04 phase 1). Greps the source tree for `error_with_code(...)`
//! call sites and asserts that every emitted code is registered in
//! `riven_core::diagnostics::codes::REGISTRY`.
//!
//! When this test fails:
//!  1. Either add the new code to `crates/riven-core/src/diagnostics/codes.rs`
//!     with a one-line title, or
//!  2. Reuse an existing code if the situation is the same.

use riven_core::diagnostics::codes::is_registered;
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir.parent().unwrap().parent().unwrap().to_path_buf()
}

/// Walk `crates/` and collect every `"EXXXX"` literal that appears
/// within five lines after a line containing `error_with_code(`.
fn collect_emitted_codes() -> Vec<(String, PathBuf, usize)> {
    let mut hits = Vec::new();
    let crates_dir = workspace_root().join("crates");
    visit_dir(&crates_dir, &mut hits);
    hits
}

fn visit_dir(dir: &PathBuf, hits: &mut Vec<(String, PathBuf, usize)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Skip target/, hidden dirs, and the registry / this test
        // itself (so the code literals there don't get scanned).
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if path.is_dir() {
            visit_dir(&path, hits);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            // Skip the registry source and this test file.
            let path_str = path.to_string_lossy();
            if path_str.contains("/diagnostics/codes.rs")
                || path_str.contains("/error_code_registry.rs")
                || path_str.contains("/diagnostics/mod.rs")
            {
                continue;
            }
            scan_file(&path, hits);
        }
    }
}

fn scan_file(path: &PathBuf, hits: &mut Vec<(String, PathBuf, usize)>) {
    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let lines: Vec<&str> = src.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("error_with_code") {
            continue;
        }
        // Look ahead up to 14 lines for the code literal. The original
        // 5-line window silently missed `implicit_includes/mod.rs` call sites
        // where the format!() argument spans several lines and the
        // `"EXXXX"` literal lands 6+ lines after `error_with_code(`.
        // Twelve lines is enough for every emitter in-tree today and
        // leaves headroom for one wrapped diagnostic argument.
        let end = (i + 15).min(lines.len());
        for look in &lines[i..end] {
            for (start, _) in look.match_indices("\"E") {
                let after = &look[start + 1..];
                if let Some(close) = after.find('"') {
                    let candidate = &after[..close];
                    if candidate.len() >= 4 && candidate.chars().skip(1).all(|c| c.is_ascii_digit())
                    {
                        hits.push((candidate.to_string(), path.clone(), i + 1));
                    }
                }
            }
        }
    }
}

#[test]
fn every_emitted_error_code_is_registered() {
    let mut unregistered = Vec::new();
    for (code, path, line) in collect_emitted_codes() {
        if !is_registered(&code) {
            unregistered.push(format!(
                "{} at {}:{} is not in REGISTRY",
                code,
                path.display(),
                line
            ));
        }
    }
    assert!(
        unregistered.is_empty(),
        "unregistered error codes:\n  {}",
        unregistered.join("\n  ")
    );
}

/// Every code in `codes::REGISTRY` must have a matching
/// `docs/errors/<code>.md` page.  Catches the drift that lets a
/// reserved code be registered with no human-facing explanation —
/// the failure surfaces with the missing filename so the fix is to
/// either author the doc or strike the registry entry.
#[test]
fn every_registered_error_code_has_a_docs_page() {
    use riven_core::diagnostics::codes::REGISTRY;
    let errors_dir = workspace_root().join("docs/errors");
    let mut missing = Vec::new();
    for entry in REGISTRY {
        let doc_path = errors_dir.join(format!("{}.md", entry.code));
        if !doc_path.exists() {
            missing.push(format!(
                "{} — expected docs page at {}",
                entry.code,
                doc_path.display()
            ));
        }
    }
    assert!(
        missing.is_empty(),
        "registered error codes without a docs/errors/*.md page:\n  {}",
        missing.join("\n  ")
    );
}

/// Inverse direction: every `docs/errors/E*.md` file must correspond
/// to a `REGISTRY` entry.  A stale doc page (e.g. left over after a
/// code is renumbered) is a footgun for users following stale
/// links from earlier compiler output; fail the build so the
/// orphan gets reconciled either by re-registering or by deleting.
#[test]
fn every_docs_error_page_has_a_registry_entry() {
    let errors_dir = workspace_root().join("docs/errors");
    let entries = match std::fs::read_dir(&errors_dir) {
        Ok(e) => e,
        Err(err) => panic!("read docs/errors: {}", err),
    };
    let mut orphans = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with('E') || !name.ends_with(".md") {
            continue;
        }
        let code = name.trim_end_matches(".md");
        if !is_registered(code) {
            orphans.push(format!(
                "{} — docs page at {} has no REGISTRY entry",
                code,
                entry.path().display()
            ));
        }
    }
    assert!(
        orphans.is_empty(),
        "orphan docs/errors/*.md pages:\n  {}",
        orphans.join("\n  ")
    );
}
