use crate::lexer::token::Span;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    E1001, // use after move
    E1002, // can't mut-borrow while immutably borrowed
    E1003, // can't immut-borrow while mutably borrowed
    E1004, // can't move out of borrowed reference
    E1005, // borrow outlives owner
    E1006, // assign to immutable variable
    E1007, // can't mut-borrow immutable variable
    E1008, // value moved into closure
    E1009, // can't move while borrowed
    E1010, // returned reference outlives local
    E1011, // value is not Send
    E1012, // value is not Sync
    E1013, // non-'static capture in send-required closure
    E1014, // invalid manual Send/Sync impl form
}

impl ErrorCode {
    pub fn title(&self) -> &'static str {
        match self {
            ErrorCode::E1001 => "value used after move",
            ErrorCode::E1002 => "cannot borrow as mutable — already borrowed as immutable",
            ErrorCode::E1003 => "cannot borrow as immutable — already borrowed as mutable",
            ErrorCode::E1004 => "cannot move out of borrowed reference",
            ErrorCode::E1005 => "borrow outlives owner",
            ErrorCode::E1006 => "cannot assign to immutable variable",
            ErrorCode::E1007 => "cannot borrow immutable variable as mutable",
            ErrorCode::E1008 => "value moved into closure, cannot be used outside",
            ErrorCode::E1009 => "cannot move value — currently borrowed",
            ErrorCode::E1010 => "returned reference outlives local value",
            ErrorCode::E1011 => "value is not `Send`",
            ErrorCode::E1012 => "value is not `Sync`",
            ErrorCode::E1013 => "non-`'static` value captured by send-required closure",
            ErrorCode::E1014 => "invalid manual `Send` / `Sync` implementation",
        }
    }

    pub fn code_str(&self) -> &'static str {
        match self {
            ErrorCode::E1001 => "E1001",
            ErrorCode::E1002 => "E1002",
            ErrorCode::E1003 => "E1003",
            ErrorCode::E1004 => "E1004",
            ErrorCode::E1005 => "E1005",
            ErrorCode::E1006 => "E1006",
            ErrorCode::E1007 => "E1007",
            ErrorCode::E1008 => "E1008",
            ErrorCode::E1009 => "E1009",
            ErrorCode::E1010 => "E1010",
            ErrorCode::E1011 => "E1011",
            ErrorCode::E1012 => "E1012",
            ErrorCode::E1013 => "E1013",
            ErrorCode::E1014 => "E1014",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SpanLabel {
    pub span: Span,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct BorrowError {
    pub code: ErrorCode,
    pub primary: SpanLabel,
    pub secondary: Vec<SpanLabel>,
    pub help: Vec<String>,
}

impl fmt::Display for BorrowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "error[{}]: {}", self.code.code_str(), self.code.title())?;
        writeln!(
            f,
            "  --> {}:{}",
            self.primary.span.line, self.primary.span.column
        )?;
        for label in &self.secondary {
            writeln!(
                f,
                "   | {}:{} — {}",
                label.span.line, label.span.column, label.label
            )?;
        }
        writeln!(
            f,
            "   | {}:{} — {}",
            self.primary.span.line, self.primary.span.column, self.primary.label
        )?;
        for h in &self.help {
            writeln!(f, "   = help: {}", h)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::token::Span;

    #[test]
    fn use_after_move_display() {
        let err = BorrowError {
            code: ErrorCode::E1001,
            primary: SpanLabel {
                span: Span::new(100, 104, 8, 11),
                label: "used here, but it's already gone".into(),
            },
            secondary: vec![
                SpanLabel {
                    span: Span::new(50, 54, 6, 5),
                    label: "value created here".into(),
                },
                SpanLabel {
                    span: Span::new(70, 74, 7, 11),
                    label: "value given to `consume()` here".into(),
                },
            ],
            help: vec![
                "pass a borrow instead: `consume(&data)`".into(),
                "or clone: `consume(data.clone)`".into(),
            ],
        };
        let rendered = format!("{}", err);
        assert!(rendered.contains("E1001"));
        assert!(rendered.contains("value used after move"));
        assert!(rendered.contains("used here, but it's already gone"));
        assert!(rendered.contains("help: pass a borrow instead"));
    }

    const ALL_ERROR_CODES: &[ErrorCode] = &[
        ErrorCode::E1001,
        ErrorCode::E1002,
        ErrorCode::E1003,
        ErrorCode::E1004,
        ErrorCode::E1005,
        ErrorCode::E1006,
        ErrorCode::E1007,
        ErrorCode::E1008,
        ErrorCode::E1009,
        ErrorCode::E1010,
        ErrorCode::E1011,
        ErrorCode::E1012,
        ErrorCode::E1013,
        ErrorCode::E1014,
    ];

    #[test]
    fn borrow_check_codes_appear_in_central_registry() {
        use crate::diagnostics::codes::is_registered;
        for ec in ALL_ERROR_CODES {
            assert!(
                is_registered(ec.code_str()),
                "{} not in diagnostics::codes::REGISTRY",
                ec.code_str()
            );
        }
    }

    #[test]
    fn all_error_codes_have_titles() {
        for code in ALL_ERROR_CODES {
            let title = code.title();
            assert!(!title.is_empty(), "{:?} has no title", code);
        }
    }
}
