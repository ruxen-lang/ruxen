//! `workspace/symbol` — cross-file symbol search tests.
//!
//! Loads two `.rx` files, builds two analysis results, and runs
//! queries against the flat (Url, &AnalysisResult) slice.

use lsp_types::{SymbolKind, Url};
use ruxen_ide::analysis::{analyze, AnalysisResult};
use ruxen_ide::workspace_symbols::workspace_symbols;

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/workspace_symbols")
        .join(format!("{}.rx", stem));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn fake_url(stem: &str) -> Url {
    Url::parse(&format!("file:///fixtures/{}.rx", stem)).unwrap()
}

/// Build the two-file workspace used by every test in this file.
fn two_file_workspace() -> (Url, Url, AnalysisResult, AnalysisResult) {
    let src_a = load("file_a");
    let src_b = load("file_b");
    let res_a = analyze(&src_a);
    let res_b = analyze(&src_b);
    (fake_url("file_a"), fake_url("file_b"), res_a, res_b)
}

#[test]
fn substring_match_finds_class_across_files() {
    let (ua, ub, ra, rb) = two_file_workspace();
    let docs = vec![(ua.clone(), &ra), (ub.clone(), &rb)];
    let hits = workspace_symbols(&docs, "user");
    let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
    assert!(names.contains(&"UserSession"), "got {:?}", names);
    assert!(names.contains(&"lookup_user"), "got {:?}", names);
}

#[test]
fn query_is_case_insensitive() {
    let (ua, ub, ra, rb) = two_file_workspace();
    let docs = vec![(ua.clone(), &ra), (ub.clone(), &rb)];
    let hits = workspace_symbols(&docs, "USERsession");
    let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
    assert!(
        names.contains(&"UserSession"),
        "case-insensitive query should match: {:?}",
        names
    );
}

#[test]
fn results_carry_correct_uri_per_file() {
    let (ua, ub, ra, rb) = two_file_workspace();
    let docs = vec![(ua.clone(), &ra), (ub.clone(), &rb)];

    let hits = workspace_symbols(&docs, "WidgetKind");
    let wk = hits
        .iter()
        .find(|h| h.name == "WidgetKind")
        .expect("WidgetKind should appear");
    assert_eq!(wk.location.uri, ub, "WidgetKind lives in file_b");
    assert_eq!(wk.kind, SymbolKind::ENUM);

    let hits = workspace_symbols(&docs, "UserSession");
    let us = hits
        .iter()
        .find(|h| h.name == "UserSession")
        .expect("UserSession should appear");
    assert_eq!(us.location.uri, ua, "UserSession lives in file_a");
    assert_eq!(us.kind, SymbolKind::CLASS);
}

#[test]
fn empty_query_returns_all_named_symbols() {
    let (ua, ub, ra, rb) = two_file_workspace();
    let docs = vec![(ua.clone(), &ra), (ub.clone(), &rb)];
    let hits = workspace_symbols(&docs, "");
    let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
    // Should at minimum contain everything we defined across both files.
    for expected in ["UserSession", "lookup_user", "WidgetKind", "render_widget"] {
        assert!(
            names.contains(&expected),
            "missing {} in {:?}",
            expected,
            names
        );
    }
}

#[test]
fn query_with_no_matches_returns_empty() {
    let (ua, ub, ra, rb) = two_file_workspace();
    let docs = vec![(ua.clone(), &ra), (ub.clone(), &rb)];
    let hits = workspace_symbols(&docs, "zzzzzNotAnythingReal");
    assert!(hits.is_empty(), "expected empty, got {:?}", hits);
}

#[test]
fn enum_variants_match_by_name() {
    let (ua, ub, ra, rb) = two_file_workspace();
    let docs = vec![(ua.clone(), &ra), (ub.clone(), &rb)];
    let hits = workspace_symbols(&docs, "button");
    let button = hits
        .iter()
        .find(|h| h.name == "Button")
        .expect("Button variant should appear");
    assert_eq!(button.kind, SymbolKind::ENUM_MEMBER);
    assert_eq!(
        button.container_name.as_deref(),
        Some("WidgetKind"),
        "Button should be reported under WidgetKind"
    );
}
