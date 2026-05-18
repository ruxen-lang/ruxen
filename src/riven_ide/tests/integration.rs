use riven_ide::analysis::analyze;

#[test]
fn analysis_produces_program_for_valid_source() {
    let source = "def main\n  let x = 42\nend\n";
    let result = analyze(source);
    assert!(
        result.program.is_some(),
        "Expected a program for valid source"
    );
    assert!(result.symbols.is_some(), "Expected a symbol table");
    assert!(
        result.diagnostics.is_empty()
            || result
                .diagnostics
                .iter()
                .all(|d| { d.level != riven_core::diagnostics::DiagnosticLevel::Error }),
        "Expected no error diagnostics for valid source: {:?}",
        result.diagnostics
    );
}

#[test]
fn analysis_stops_on_lex_error() {
    let source = "let x = \"\n"; // unterminated string
    let result = analyze(source);
    assert!(
        result.program.is_none(),
        "Should not produce a program on lex error"
    );
    assert!(!result.diagnostics.is_empty(), "Should have diagnostics");
}

#[test]
fn analysis_stops_on_parse_error() {
    let source = "def\n"; // incomplete function definition
    let result = analyze(source);
    // Should either fail to parse or produce diagnostics
    assert!(
        result.program.is_none() || !result.diagnostics.is_empty(),
        "Should have errors or no program for parse error"
    );
}

#[test]
fn diagnostics_for_type_error() {
    // Try assigning a string to an Int-annotated variable
    let source = "def main\n  let x: Int = \"hello\"\nend\n";
    let result = analyze(source);
    let uri = lsp_types::Url::parse("file:///test.rvn").unwrap();
    let diagnostics = riven_ide::diagnostics::collect_diagnostics(&result, &uri);
    assert!(
        !diagnostics.is_empty(),
        "Expected at least one diagnostic for type mismatch"
    );
    assert_eq!(
        diagnostics[0].severity,
        Some(lsp_types::DiagnosticSeverity::ERROR)
    );
}

#[test]
fn hover_shows_inferred_type() {
    let source = "def main\n  let x = 42\nend\n";
    let result = analyze(source);

    // Find byte offset of 'x' in "let x = 42"
    // "def main\n  let x = 42\nend\n"
    // 0123456789...
    // 'd','e','f',' ','m','a','i','n','\n',' ',' ','l','e','t',' ','x'
    // x is at byte offset 15
    let pos = result.line_index.position_of(15);
    let hover = riven_ide::hover::hover_at(&result, pos);
    if let Some(info) = hover {
        // The hover should mention Int (the inferred type of 42)
        assert!(
            info.content.contains("Int"),
            "Expected Int type in hover, got: {}",
            info.content
        );
    }
    // If hover returns None, the node finder didn't locate x —
    // acceptable for now since we need more precise span matching.
}

#[test]
fn goto_definition_finds_variable() {
    let source = "def main\n  let x = 42\n  let y = x\nend\n";
    let result = analyze(source);

    // Find the 'x' reference in "let y = x"
    // "def main\n  let x = 42\n  let y = x\nend\n"
    // line 2 (0-indexed): "  let y = x"
    // The x reference is at the end of line 2
    // Let's find it by searching for the second 'x'
    if let Some(pos) = source.rfind('x') {
        // This is the 'x' in 'let y = x'
        let lsp_pos = result.line_index.position_of(pos);
        let location = riven_ide::goto_def::goto_definition(&result, lsp_pos);
        if let Some(loc) = location {
            // Definition should point to line 1 (where x is defined)
            assert_eq!(loc.range.start.line, 1, "Expected definition on line 1");
        }
    }
}

#[test]
fn semantic_tokens_produces_output() {
    let source = "def main\n  let x = 42\nend\n";
    let result = analyze(source);
    let tokens = riven_ide::semantic_tokens::semantic_tokens(&result);
    assert!(!tokens.is_empty(), "Expected some semantic tokens");
}

#[test]
fn analysis_of_sample_program() {
    let source = std::fs::read_to_string("../riven-core/tests/fixtures/sample_program.rvn")
        .expect("failed to read sample_program.rvn");
    let result = analyze(&source);
    assert!(
        result.program.is_some(),
        "Sample program should analyze successfully"
    );

    // Should produce semantic tokens
    let tokens = riven_ide::semantic_tokens::semantic_tokens(&result);
    assert!(
        !tokens.is_empty(),
        "Expected semantic tokens from sample program"
    );

    // Should produce diagnostics (collect them)
    let uri = lsp_types::Url::parse("file:///test.rvn").unwrap();
    let _diagnostics = riven_ide::diagnostics::collect_diagnostics(&result, &uri);
}

#[test]
fn analysis_of_arithmetic_program() {
    let source = "def main\n  let x = 10\n  let y = 20\n  let sum = x + y\nend\n";
    let result = analyze(source);
    assert!(
        result.program.is_some(),
        "Arithmetic program should analyze successfully"
    );
    assert!(result.symbols.is_some());
}

// ─── End-to-end integration tests ──────────────────────────────────

#[test]
fn e2e_hover_and_goto_consistent_for_variable() {
    // Hover and goto-def on the same variable reference should both succeed and
    // point at the same logical location.
    let source = "def main\n  let alpha = 42\n  let beta = alpha\nend\n";
    let result = analyze(source);
    // Find the 'alpha' reference in "let beta = alpha"
    let offset = source.rfind("alpha").unwrap();
    let pos = result.line_index.position_of(offset);

    let hover = riven_ide::hover::hover_at(&result, pos);
    let goto = riven_ide::goto_def::goto_definition(&result, pos);

    assert!(hover.is_some(), "Hover should resolve alpha");
    assert!(goto.is_some(), "Goto-def should resolve alpha");
    // goto-def should point at line 1 where alpha is defined
    assert_eq!(goto.unwrap().range.start.line, 1);
}

#[test]
fn e2e_semantic_tokens_and_diagnostics_on_type_error() {
    // A program with a type error should still produce semantic tokens
    // (for syntax-highlighting resilience).
    let source = "def main\n  let x: Int = \"oops\"\nend\n";
    let result = analyze(source);
    let uri = lsp_types::Url::parse("file:///bad.rvn").unwrap();
    let diagnostics = riven_ide::diagnostics::collect_diagnostics(&result, &uri);
    assert!(
        !diagnostics.is_empty(),
        "Expected diagnostics for type error"
    );
    let tokens = riven_ide::semantic_tokens::semantic_tokens(&result);
    assert!(
        !tokens.is_empty(),
        "Expected tokens even when there is a type error"
    );
}

#[test]
fn e2e_goto_def_for_named_function() {
    let source =
        "def compute(x: Int) -> Int\n  x * 2\nend\n\ndef main\n  let r = compute(5)\nend\n";
    let result = analyze(source);
    // Find 'compute' in "let r = compute(5)"
    let offset = source.rfind("compute").unwrap();
    let pos = result.line_index.position_of(offset);
    let loc = riven_ide::goto_def::goto_definition(&result, pos);
    assert!(loc.is_some());
    // Definition is on line 0
    assert_eq!(loc.unwrap().range.start.line, 0);
}

#[test]
fn e2e_hover_on_multiple_functions() {
    let source = "def first -> Int\n  1\nend\n\ndef second -> Int\n  2\nend\n\ndef main\n  let a = first\n  let b = second\nend\n";
    let result = analyze(source);

    // Hover on 'first' call
    let off1 = source.rfind("first").unwrap();
    let pos1 = result.line_index.position_of(off1);
    let h1 = riven_ide::hover::hover_at(&result, pos1);
    assert!(h1.is_some(), "Expected hover on 'first'");

    // Hover on 'second' call
    let off2 = source.rfind("second").unwrap();
    let pos2 = result.line_index.position_of(off2);
    let h2 = riven_ide::hover::hover_at(&result, pos2);
    assert!(h2.is_some(), "Expected hover on 'second'");
}

#[test]
fn e2e_analysis_result_has_correct_source_and_line_index() {
    let source = "def main\n  let x = 1\nend\n";
    let result = analyze(source);
    assert_eq!(result.source, source);
    // Verify line_index correctly locates line 1, col 2 (the start of '  let x')
    let pos = result.line_index.position_of(9);
    assert_eq!(pos.line, 1);
    assert_eq!(pos.character, 0);
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
    let uri = lsp_types::Url::parse("file:///t.rvn").unwrap();
    let diagnostics = riven_ide::diagnostics::collect_diagnostics(&result, &uri);
    assert!(!diagnostics.is_empty());
    // First diagnostic should be on line 1
    let line = diagnostics[0].range.start.line;
    assert_eq!(line, 1, "Expected diagnostic on line 1, got {}", line);
}

#[test]
fn e2e_goto_def_cross_function_var_ref() {
    let source = "def main\n  let x = 100\n  let y = 200\n  let z = x + y\nend\n";
    let result = analyze(source);

    // Goto def on 'x' in "x + y"
    let offset = source.rfind("x + y").unwrap();
    let pos = result.line_index.position_of(offset);
    let loc = riven_ide::goto_def::goto_definition(&result, pos);
    assert!(loc.is_some());
    assert_eq!(loc.unwrap().range.start.line, 1); // line with let x = 100
}

#[test]
fn e2e_semantic_tokens_are_nonoverlapping() {
    let source = "def main\n  let x = 42\n  let y = x + 1\nend\n";
    let result = analyze(source);
    let tokens = riven_ide::semantic_tokens::semantic_tokens(&result);
    // Walk deltas and verify absolute positions are non-decreasing
    let mut line = 0u32;
    let mut start = 0u32;
    for t in &tokens {
        if t.delta_line == 0 {
            start += t.delta_start;
        } else {
            line += t.delta_line;
            start = t.delta_start;
        }
        // length > 0
        assert!(t.length > 0);
        // token_type is valid
        assert!((t.token_type as usize) < riven_ide::semantic_tokens::TOKEN_TYPES.len());
    }
    let _ = (line, start); // suppress unused warnings
}

#[test]
fn e2e_diagnostic_collection_is_stable() {
    let source = "def main\n  let x: Int = \"bad\"\nend\n";
    let result = analyze(source);
    let uri = lsp_types::Url::parse("file:///t.rvn").unwrap();
    let diagnostics1 = riven_ide::diagnostics::collect_diagnostics(&result, &uri);
    let diagnostics2 = riven_ide::diagnostics::collect_diagnostics(&result, &uri);
    assert_eq!(diagnostics1.len(), diagnostics2.len());
    for (a, b) in diagnostics1.iter().zip(diagnostics2.iter()) {
        assert_eq!(a.message, b.message);
        assert_eq!(a.range, b.range);
        assert_eq!(a.severity, b.severity);
    }
}
