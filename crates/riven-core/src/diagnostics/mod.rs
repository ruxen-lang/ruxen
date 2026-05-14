use crate::lexer::token::Span;
use std::fmt;

pub mod codes;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Help,
}

impl fmt::Display for DiagnosticLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DiagnosticLevel::Error => write!(f, "error"),
            DiagnosticLevel::Warning => write!(f, "warning"),
            DiagnosticLevel::Help => write!(f, "help"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub level: DiagnosticLevel,
    pub message: String,
    pub span: Span,
    pub code: Option<String>,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>, span: Span) -> Self {
        Self {
            level: DiagnosticLevel::Error,
            message: message.into(),
            span,
            code: None,
        }
    }

    pub fn error_with_code(
        message: impl Into<String>,
        span: Span,
        code: impl Into<String>,
    ) -> Self {
        Self {
            level: DiagnosticLevel::Error,
            message: message.into(),
            span,
            code: Some(code.into()),
        }
    }

    pub fn warning(message: impl Into<String>, span: Span) -> Self {
        Self {
            level: DiagnosticLevel::Warning,
            message: message.into(),
            span,
            code: None,
        }
    }

    pub fn warning_with_code(
        message: impl Into<String>,
        span: Span,
        code: impl Into<String>,
    ) -> Self {
        Self {
            level: DiagnosticLevel::Warning,
            message: message.into(),
            span,
            code: Some(code.into()),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code_str = match &self.code {
            Some(c) => format!("[{}] ", c),
            None => String::new(),
        };
        write!(
            f,
            "{}{}: {} (at {}:{})",
            code_str, self.level, self.message, self.span.line, self.span.column
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::token::Span;

    fn sample_span() -> Span {
        Span::new(0, 5, 1, 1)
    }

    #[test]
    fn error_constructor_sets_level_and_message() {
        let diag = Diagnostic::error("oops", sample_span());
        assert_eq!(diag.level, DiagnosticLevel::Error);
        assert_eq!(diag.message, "oops");
        assert!(diag.code.is_none());
    }

    #[test]
    fn warning_constructor_sets_level_and_message() {
        let diag = Diagnostic::warning("careful", sample_span());
        assert_eq!(diag.level, DiagnosticLevel::Warning);
        assert_eq!(diag.message, "careful");
        assert!(diag.code.is_none());
    }

    // NOTE: `Diagnostic::info` was listed in the test spec but does not exist
    // on the current `Diagnostic` struct; the module only supports Error,
    // Warning, and a Help variant (no corresponding constructor). See the
    // `DiagnosticLevel` enum in this module.
    #[test]
    #[ignore = "Diagnostic::info does not exist in the current API"]
    fn info_constructor_missing() {
        // Intentionally left ignored — no `Diagnostic::info` constructor.
    }

    #[test]
    fn error_with_code_sets_code_field() {
        let diag = Diagnostic::error_with_code("bad thing", sample_span(), "E0042");
        assert_eq!(diag.level, DiagnosticLevel::Error);
        assert_eq!(diag.message, "bad thing");
        assert_eq!(diag.code.as_deref(), Some("E0042"));
    }

    // NOTE: The spec asked for `error_with_note` / `error_with_help` but these
    // constructors do not exist, and the struct has no `note`/`help` fields.
    // Only `level`, `message`, `span`, and `code` are present today.
    #[test]
    #[ignore = "Diagnostic has no `note` field or `error_with_note` constructor"]
    fn error_with_note_missing() {}

    #[test]
    #[ignore = "Diagnostic has no `help` field or `error_with_help` constructor"]
    fn error_with_help_missing() {}

    #[test]
    fn code_field_defaults_to_none() {
        let e = Diagnostic::error("m", sample_span());
        let w = Diagnostic::warning("m", sample_span());
        assert!(e.code.is_none());
        assert!(w.code.is_none());
    }

    #[test]
    fn diagnostic_preserves_span_fields() {
        let span = Span::new(10, 20, 5, 3);
        let diag = Diagnostic::error("boom", span.clone());
        assert_eq!(diag.span, span);
        assert_eq!(diag.span.start, 10);
        assert_eq!(diag.span.end, 20);
        assert_eq!(diag.span.line, 5);
        assert_eq!(diag.span.column, 3);
    }

    #[test]
    fn diagnostic_level_equality_and_inequality() {
        assert_eq!(DiagnosticLevel::Error, DiagnosticLevel::Error);
        assert_eq!(DiagnosticLevel::Warning, DiagnosticLevel::Warning);
        assert_eq!(DiagnosticLevel::Help, DiagnosticLevel::Help);
        assert_ne!(DiagnosticLevel::Error, DiagnosticLevel::Warning);
        assert_ne!(DiagnosticLevel::Warning, DiagnosticLevel::Help);
        assert_ne!(DiagnosticLevel::Error, DiagnosticLevel::Help);
    }

    // NOTE: DiagnosticLevel does not implement PartialOrd/Ord; only equality
    // is defined. Test documents the current semantics.
    #[test]
    fn diagnostic_level_copy_semantics() {
        let lvl = DiagnosticLevel::Error;
        let copy = lvl; // Copy, not move
        assert_eq!(lvl, copy);
    }

    #[test]
    fn diagnostic_level_display() {
        assert_eq!(format!("{}", DiagnosticLevel::Error), "error");
        assert_eq!(format!("{}", DiagnosticLevel::Warning), "warning");
        assert_eq!(format!("{}", DiagnosticLevel::Help), "help");
    }

    #[test]
    fn diagnostic_display_without_code() {
        let diag = Diagnostic::error("something broke", Span::new(0, 1, 7, 2));
        let rendered = format!("{}", diag);
        assert_eq!(rendered, "error: something broke (at 7:2)");
    }

    #[test]
    fn diagnostic_display_with_code() {
        let diag = Diagnostic::error_with_code("oops", Span::new(0, 1, 3, 9), "E1");
        let rendered = format!("{}", diag);
        assert_eq!(rendered, "[E1] error: oops (at 3:9)");
    }

    #[test]
    fn diagnostic_debug_does_not_panic() {
        let diag = Diagnostic::warning("heads up", sample_span());
        // Just ensure the Debug impl is usable; its exact content is not
        // guaranteed but it must not panic.
        let _ = format!("{:?}", diag);
        let _ = format!("{:?}", DiagnosticLevel::Help);
    }

    #[test]
    fn diagnostic_clone_preserves_all_fields() {
        let diag = Diagnostic::error_with_code("dup", sample_span(), "E99");
        let cloned = diag.clone();
        assert_eq!(cloned, diag);
        assert_eq!(cloned.level, diag.level);
        assert_eq!(cloned.message, diag.message);
        assert_eq!(cloned.code, diag.code);
        assert_eq!(cloned.span, diag.span);
    }

    #[test]
    fn diagnostic_partial_eq_with_different_messages() {
        let a = Diagnostic::error("a", sample_span());
        let b = Diagnostic::error("b", sample_span());
        assert_ne!(a, b);
    }

    #[test]
    fn diagnostic_accepts_string_and_str_messages() {
        let from_str = Diagnostic::error("literal", sample_span());
        let from_owned = Diagnostic::error(String::from("literal"), sample_span());
        assert_eq!(from_str.message, from_owned.message);
    }

    #[test]
    fn error_with_code_accepts_string_and_str_for_code() {
        let a = Diagnostic::error_with_code("m", sample_span(), "E42");
        let b = Diagnostic::error_with_code("m", sample_span(), String::from("E42"));
        assert_eq!(a.code, b.code);
    }
}
