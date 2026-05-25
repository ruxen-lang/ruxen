use lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location,
    NumberOrString, Url,
};
use ruxen_core::borrow_check::errors::BorrowError;
use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};

use crate::analysis::AnalysisResult;
use crate::line_index::LineIndex;

pub fn to_lsp_diagnostic(diag: &Diagnostic, line_index: &LineIndex) -> LspDiagnostic {
    LspDiagnostic {
        range: line_index.span_to_range(&diag.span),
        severity: Some(match diag.level {
            DiagnosticLevel::Error => DiagnosticSeverity::ERROR,
            DiagnosticLevel::Warning => DiagnosticSeverity::WARNING,
            DiagnosticLevel::Help => DiagnosticSeverity::HINT,
        }),
        code: diag
            .code
            .as_ref()
            .map(|c| NumberOrString::String(c.clone())),
        source: Some("ruxenc".to_string()),
        message: diag.message.clone(),
        related_information: None,
        ..Default::default()
    }
}

pub fn borrow_error_to_lsp(err: &BorrowError, line_index: &LineIndex, uri: &Url) -> LspDiagnostic {
    let related: Vec<DiagnosticRelatedInformation> = err
        .secondary
        .iter()
        .map(|label| DiagnosticRelatedInformation {
            location: Location {
                uri: uri.clone(),
                range: line_index.span_to_range(&label.span),
            },
            message: label.label.clone(),
        })
        .collect();

    LspDiagnostic {
        range: line_index.span_to_range(&err.primary.span),
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(err.code.code_str().to_string())),
        source: Some("ruxenc".to_string()),
        message: format!("{}: {}", err.code.title(), err.primary.label),
        related_information: if related.is_empty() {
            None
        } else {
            Some(related)
        },
        ..Default::default()
    }
}

/// Convert all diagnostics from an AnalysisResult into LSP diagnostics.
pub fn collect_diagnostics(result: &AnalysisResult, uri: &Url) -> Vec<LspDiagnostic> {
    let mut lsp_diagnostics: Vec<LspDiagnostic> = result
        .diagnostics
        .iter()
        .map(|d| to_lsp_diagnostic(d, &result.line_index))
        .collect();

    for err in &result.borrow_errors {
        lsp_diagnostics.push(borrow_error_to_lsp(err, &result.line_index, uri));
    }

    lsp_diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;
    use ruxen_core::borrow_check::errors::{ErrorCode, SpanLabel};
    use ruxen_core::lexer::token::Span;

    fn make_line_index(src: &str) -> LineIndex {
        LineIndex::new(src)
    }

    #[test]
    fn error_diagnostic_maps_to_error_severity() {
        let src = "hello world";
        let idx = make_line_index(src);
        let diag = Diagnostic::error("oops", Span::new(0, 5, 0, 0));
        let lsp = to_lsp_diagnostic(&diag, &idx);
        assert_eq!(lsp.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(lsp.message, "oops");
    }

    #[test]
    fn warning_diagnostic_maps_to_warning_severity() {
        let src = "hello world";
        let idx = make_line_index(src);
        let diag = Diagnostic::warning("heads up", Span::new(0, 5, 0, 0));
        let lsp = to_lsp_diagnostic(&diag, &idx);
        assert_eq!(lsp.severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn help_diagnostic_maps_to_hint_severity() {
        let src = "hello";
        let idx = make_line_index(src);
        let diag = Diagnostic {
            level: DiagnosticLevel::Help,
            message: "try this".to_string(),
            span: Span::new(0, 5, 0, 0),
            code: None,
        };
        let lsp = to_lsp_diagnostic(&diag, &idx);
        assert_eq!(lsp.severity, Some(DiagnosticSeverity::HINT));
    }

    #[test]
    fn diagnostic_source_is_ruxenc() {
        let idx = make_line_index("x");
        let diag = Diagnostic::error("err", Span::new(0, 1, 0, 0));
        let lsp = to_lsp_diagnostic(&diag, &idx);
        assert_eq!(lsp.source.as_deref(), Some("ruxenc"));
    }

    #[test]
    fn diagnostic_code_is_preserved() {
        let idx = make_line_index("x");
        let diag = Diagnostic::error_with_code("err", Span::new(0, 1, 0, 0), "E42");
        let lsp = to_lsp_diagnostic(&diag, &idx);
        match lsp.code {
            Some(NumberOrString::String(s)) => assert_eq!(s, "E42"),
            other => panic!("Expected String code E42, got {:?}", other),
        }
    }

    #[test]
    fn diagnostic_without_code_preserves_none() {
        let idx = make_line_index("x");
        let diag = Diagnostic::error("err", Span::new(0, 1, 0, 0));
        let lsp = to_lsp_diagnostic(&diag, &idx);
        assert!(lsp.code.is_none());
    }

    #[test]
    fn diagnostic_range_maps_from_span() {
        let src = "abc\ndef";
        let idx = make_line_index(src);
        // Second line, chars 1..3 ("ef")
        let diag = Diagnostic::error("x", Span::new(5, 7, 1, 1));
        let lsp = to_lsp_diagnostic(&diag, &idx);
        assert_eq!(lsp.range.start.line, 1);
        assert_eq!(lsp.range.start.character, 1);
        assert_eq!(lsp.range.end.line, 1);
        assert_eq!(lsp.range.end.character, 3);
    }

    #[test]
    fn diagnostic_multiline_span() {
        // Span across two lines: from byte 2 (line 0) to byte 5 (line 1)
        let src = "abc\ndef";
        let idx = make_line_index(src);
        let diag = Diagnostic::error("span", Span::new(2, 5, 0, 2));
        let lsp = to_lsp_diagnostic(&diag, &idx);
        assert_eq!(lsp.range.start.line, 0);
        assert_eq!(lsp.range.end.line, 1);
    }

    #[test]
    fn borrow_error_maps_to_error_severity() {
        let src = "abc\ndef";
        let idx = make_line_index(src);
        let uri = Url::parse("file:///x.rx").unwrap();
        let err = BorrowError {
            code: ErrorCode::E1001,
            primary: SpanLabel {
                span: Span::new(0, 3, 0, 0),
                label: "primary label".to_string(),
            },
            secondary: Vec::new(),
            help: Vec::new(),
        };
        let lsp = borrow_error_to_lsp(&err, &idx, &uri);
        assert_eq!(lsp.severity, Some(DiagnosticSeverity::ERROR));
        // Code should carry the E-code string
        match lsp.code {
            Some(NumberOrString::String(s)) => assert_eq!(s, "E1001"),
            other => panic!("expected E1001 code, got {:?}", other),
        }
    }

    #[test]
    fn borrow_error_has_ruxenc_source() {
        let idx = make_line_index("a");
        let uri = Url::parse("file:///x.rx").unwrap();
        let err = BorrowError {
            code: ErrorCode::E1006,
            primary: SpanLabel {
                span: Span::new(0, 1, 0, 0),
                label: "x".into(),
            },
            secondary: Vec::new(),
            help: Vec::new(),
        };
        let lsp = borrow_error_to_lsp(&err, &idx, &uri);
        assert_eq!(lsp.source.as_deref(), Some("ruxenc"));
    }

    #[test]
    fn borrow_error_message_includes_title() {
        let idx = make_line_index("a");
        let uri = Url::parse("file:///x.rx").unwrap();
        let err = BorrowError {
            code: ErrorCode::E1001,
            primary: SpanLabel {
                span: Span::new(0, 1, 0, 0),
                label: "used here".into(),
            },
            secondary: Vec::new(),
            help: Vec::new(),
        };
        let lsp = borrow_error_to_lsp(&err, &idx, &uri);
        // Title is "value used after move"; label is "used here"
        assert!(
            lsp.message.contains("value used after move"),
            "Message should include title: {}",
            lsp.message
        );
        assert!(
            lsp.message.contains("used here"),
            "Message should include label: {}",
            lsp.message
        );
    }

    #[test]
    fn borrow_error_with_secondary_has_related_info() {
        let idx = make_line_index("ab");
        let uri = Url::parse("file:///x.rx").unwrap();
        let err = BorrowError {
            code: ErrorCode::E1001,
            primary: SpanLabel {
                span: Span::new(0, 1, 0, 0),
                label: "primary".into(),
            },
            secondary: vec![SpanLabel {
                span: Span::new(1, 2, 0, 1),
                label: "secondary".into(),
            }],
            help: Vec::new(),
        };
        let lsp = borrow_error_to_lsp(&err, &idx, &uri);
        assert!(
            lsp.related_information.is_some(),
            "Expected related information for secondary spans"
        );
        let related = lsp.related_information.unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].message, "secondary");
        assert_eq!(related[0].location.uri, uri);
    }

    #[test]
    fn borrow_error_with_no_secondary_has_no_related_info() {
        let idx = make_line_index("a");
        let uri = Url::parse("file:///x.rx").unwrap();
        let err = BorrowError {
            code: ErrorCode::E1001,
            primary: SpanLabel {
                span: Span::new(0, 1, 0, 0),
                label: "primary".into(),
            },
            secondary: Vec::new(),
            help: Vec::new(),
        };
        let lsp = borrow_error_to_lsp(&err, &idx, &uri);
        assert!(lsp.related_information.is_none());
    }

    #[test]
    fn collect_diagnostics_valid_source_is_empty() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        let uri = Url::parse("file:///x.rx").unwrap();
        let diagnostics = collect_diagnostics(&result, &uri);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.severity != Some(DiagnosticSeverity::ERROR)),
            "Expected no errors for valid source: {:?}",
            diagnostics
        );
    }

    #[test]
    fn collect_diagnostics_for_type_mismatch() {
        let src = "def main\n  let x: Int = \"wrong\"\nend\n";
        let result = analyze(src);
        let uri = Url::parse("file:///x.rx").unwrap();
        let diagnostics = collect_diagnostics(&result, &uri);
        assert!(!diagnostics.is_empty(), "Expected diagnostics");
        assert_eq!(diagnostics[0].severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diagnostics[0].source.as_deref(), Some("ruxenc"));
    }

    #[test]
    fn collect_diagnostics_empty_when_no_errors_and_no_borrow_errors() {
        let src = "def main\n  let x = 1\n  let y = 2\n  let z = x + y\nend\n";
        let result = analyze(src);
        let uri = Url::parse("file:///x.rx").unwrap();
        let diagnostics = collect_diagnostics(&result, &uri);
        let error_count = diagnostics
            .iter()
            .filter(|d| d.severity == Some(DiagnosticSeverity::ERROR))
            .count();
        assert_eq!(
            error_count, 0,
            "Expected zero error diagnostics, got {:?}",
            diagnostics
        );
    }
}
