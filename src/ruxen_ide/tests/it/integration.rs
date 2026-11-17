use ruxen_ide::analysis::analyze;

#[test]
fn analysis_of_sample_program() {
    let source =
        std::fs::read_to_string("../../compiler/ruxen_core/tests/fixtures/sample_program.rx")
            .expect("failed to read sample_program.rx");
    let result = analyze(&source);
    assert!(
        result.program.is_some(),
        "Sample program should analyze successfully"
    );

    // Should produce semantic tokens
    let tokens = ruxen_ide::semantic_tokens::semantic_tokens(&result);
    assert!(
        !tokens.is_empty(),
        "Expected semantic tokens from sample program"
    );

    // Should produce diagnostics (collect them)
    let uri = lsp_types::Url::parse("file:///test.rx").unwrap();
    let _diagnostics = ruxen_ide::diagnostics::collect_diagnostics(&result, &uri);
}

#[test]
fn e2e_semantic_tokens_and_diagnostics_on_type_error() {
    // A program with a type error should still produce semantic tokens
    // (for syntax-highlighting resilience).
    let source = "def main\n  let x: Int = \"oops\"\nend\n";
    let result = analyze(source);
    let uri = lsp_types::Url::parse("file:///bad.rx").unwrap();
    let diagnostics = ruxen_ide::diagnostics::collect_diagnostics(&result, &uri);
    assert!(
        !diagnostics.is_empty(),
        "Expected diagnostics for type error"
    );
    let tokens = ruxen_ide::semantic_tokens::semantic_tokens(&result);
    assert!(
        !tokens.is_empty(),
        "Expected tokens even when there is a type error"
    );
}

#[test]
fn e2e_pipeline_recovers_from_nonfatal_warnings() {
    // A valid program should pass through all phases and produce a full HIR.
    let source =
        "def add(a: Int, b: Int) -> Int\n  a + b\nend\n\ndef main\n  let r = add(1, 2)\nend\n";
    let result = analyze(source);
    assert!(result.program.is_some());
    assert!(result.symbols.is_some());
    assert!(result.type_context.is_some());
    // Verify add is in symbols
    let symbols = result.symbols.as_ref().unwrap();
    assert!(symbols.iter().any(|d| d.name == "add"));
}

#[test]
fn e2e_diagnostics_preserve_positions_across_pipeline() {
    let source = "def main\n  let x: Int = \"bad\"\nend\n";
    let result = analyze(source);
    let uri = lsp_types::Url::parse("file:///t.rx").unwrap();
    let diagnostics = ruxen_ide::diagnostics::collect_diagnostics(&result, &uri);
    assert!(!diagnostics.is_empty());
    // First diagnostic should be on line 1
    let line = diagnostics[0].range.start.line;
    assert_eq!(line, 1, "Expected diagnostic on line 1, got {}", line);
}

#[test]
fn e2e_diagnostic_collection_is_stable() {
    let source = "def main\n  let x: Int = \"bad\"\nend\n";
    let result = analyze(source);
    let uri = lsp_types::Url::parse("file:///t.rx").unwrap();
    let diagnostics1 = ruxen_ide::diagnostics::collect_diagnostics(&result, &uri);
    let diagnostics2 = ruxen_ide::diagnostics::collect_diagnostics(&result, &uri);
    assert_eq!(diagnostics1.len(), diagnostics2.len());
    for (a, b) in diagnostics1.iter().zip(diagnostics2.iter()) {
        assert_eq!(a.message, b.message);
        assert_eq!(a.range, b.range);
        assert_eq!(a.severity, b.severity);
    }
}
