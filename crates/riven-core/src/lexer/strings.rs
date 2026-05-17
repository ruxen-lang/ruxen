use super::*;
use crate::diagnostics::Diagnostic;
use token::*;

impl<'a> Lexer<'a> {
    pub(super) fn lex_string(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;

        // Check for triple-quoted multiline string
        if self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"') {
            self.lex_multiline_string(start_byte, start_line, start_col);
            return;
        }

        self.advance(); // opening "

        let mut parts: Vec<StringPart> = Vec::new();
        let mut current_text = String::new();
        let mut has_interpolation = false;

        loop {
            if self.is_at_end() {
                let span = self.make_span(start_byte, start_line, start_col);
                self.diagnostics.push(Diagnostic::error_with_code(
                    "unterminated string literal",
                    span,
                    "E0002",
                ));
                break;
            }

            let ch = self.current();

            if ch == '"' {
                self.advance(); // closing "
                break;
            }

            if ch == '\\' {
                if let Ok(c) = self.lex_escape_sequence() {
                    current_text.push(c);
                }
                continue;
            }

            if ch == '#' && self.peek_at(1) == Some('{') {
                has_interpolation = true;
                // Save current text
                if !current_text.is_empty() {
                    parts.push(StringPart::Literal(std::mem::take(&mut current_text)));
                }
                // Lex the interpolation expression
                self.advance(); // #
                self.advance(); // {
                let (expr_tokens, spec) = self.lex_interpolation_expr();
                parts.push(StringPart::Expr {
                    tokens: expr_tokens,
                    spec,
                });
                continue;
            }

            current_text.push(ch);
            self.advance();
        }

        if has_interpolation {
            if !current_text.is_empty() {
                parts.push(StringPart::Literal(current_text));
            }
            self.emit(
                TokenKind::InterpolatedString(parts),
                start_byte,
                start_line,
                start_col,
            );
        } else {
            self.emit(
                TokenKind::StringLiteral(current_text),
                start_byte,
                start_line,
                start_col,
            );
        }
    }

    pub(super) fn lex_multiline_string(&mut self, start_byte: usize, start_line: u32, start_col: u32) {
        self.advance(); // "
        self.advance(); // "
        self.advance(); // "

        // Skip optional newline after opening """
        if !self.is_at_end() && self.current() == '\n' {
            self.advance();
        }

        let mut content = String::new();

        loop {
            if self.is_at_end() {
                let span = self.make_span(start_byte, start_line, start_col);
                self.diagnostics.push(Diagnostic::error_with_code(
                    "unterminated multiline string literal",
                    span,
                    "E0002",
                ));
                break;
            }

            if self.current() == '"' && self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"')
            {
                self.advance(); // "
                self.advance(); // "
                self.advance(); // "
                break;
            }

            content.push(self.current());
            self.advance();
        }

        // Strip common leading whitespace
        let stripped = strip_leading_whitespace(&content);
        self.emit(
            TokenKind::StringLiteral(stripped),
            start_byte,
            start_line,
            start_col,
        );
    }

    pub(super) fn lex_raw_string(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;

        self.advance(); // 'r'

        // Count # delimiters
        let mut hash_count = 0;
        while !self.is_at_end() && self.current() == '#' {
            hash_count += 1;
            self.advance();
        }

        // Expect opening "
        if self.is_at_end() || self.current() != '"' {
            let span = self.make_span(start_byte, start_line, start_col);
            self.diagnostics.push(Diagnostic::error(
                "expected '\"' after raw string prefix",
                span,
            ));
            return;
        }
        self.advance(); // "

        let mut content = String::new();

        loop {
            if self.is_at_end() {
                let span = self.make_span(start_byte, start_line, start_col);
                self.diagnostics.push(Diagnostic::error_with_code(
                    "unterminated raw string literal",
                    span,
                    "E0002",
                ));
                break;
            }

            if self.current() == '"' {
                // Check if followed by the right number of #
                let mut matching = true;
                for i in 1..=hash_count {
                    if self.peek_at(i) != Some('#') {
                        matching = false;
                        break;
                    }
                }
                if matching {
                    self.advance(); // "
                    for _ in 0..hash_count {
                        self.advance(); // #
                    }
                    break;
                }
            }

            content.push(self.current());
            self.advance();
        }

        self.emit(
            TokenKind::StringLiteral(content),
            start_byte,
            start_line,
            start_col,
        );
    }

    pub(super) fn lex_escape_sequence(&mut self) -> Result<char, ()> {
        let esc_start_byte = self.byte_pos;
        let esc_start_line = self.line;
        let esc_start_col = self.column;
        self.advance(); // '\'

        if self.is_at_end() {
            let span = self.make_span(esc_start_byte, esc_start_line, esc_start_col);
            self.diagnostics.push(Diagnostic::error_with_code(
                "unexpected end of file in escape sequence",
                span,
                "E0003",
            ));
            return Err(());
        }

        let ch = self.advance();
        match ch {
            '\\' => Ok('\\'),
            '"' => Ok('"'),
            '\'' => Ok('\''),
            'n' => Ok('\n'),
            't' => Ok('\t'),
            'r' => Ok('\r'),
            '0' => Ok('\0'),
            '#' => Ok('#'),
            'u' => {
                if self.is_at_end() || self.current() != '{' {
                    let span = self.make_span(esc_start_byte, esc_start_line, esc_start_col);
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "expected '{' in unicode escape",
                        span,
                        "E0003",
                    ));
                    return Err(());
                }
                self.advance(); // {
                let mut hex = String::new();
                while !self.is_at_end() && self.current() != '}' {
                    hex.push(self.advance());
                }
                if self.is_at_end() {
                    let span = self.make_span(esc_start_byte, esc_start_line, esc_start_col);
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "unterminated unicode escape",
                        span,
                        "E0003",
                    ));
                    return Err(());
                }
                self.advance(); // }
                match u32::from_str_radix(&hex, 16) {
                    Ok(code) => match char::from_u32(code) {
                        Some(c) => Ok(c),
                        None => {
                            let span =
                                self.make_span(esc_start_byte, esc_start_line, esc_start_col);
                            self.diagnostics.push(Diagnostic::error_with_code(
                                format!("invalid unicode code point: U+{:04X}", code),
                                span,
                                "E0003",
                            ));
                            Err(())
                        }
                    },
                    Err(_) => {
                        let span = self.make_span(esc_start_byte, esc_start_line, esc_start_col);
                        self.diagnostics.push(Diagnostic::error_with_code(
                            format!("invalid hex in unicode escape: {}", hex),
                            span,
                            "E0003",
                        ));
                        Err(())
                    }
                }
            }
            other => {
                let span = self.make_span(esc_start_byte, esc_start_line, esc_start_col);
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!("invalid escape sequence: \\{}", other),
                    span,
                    "E0003",
                ));
                Err(())
            }
        }
    }
}

/// Strip common leading whitespace from a multiline string.
fn strip_leading_whitespace(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.is_empty() {
        return String::new();
    }

    // Find the minimum indentation of non-empty lines
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    lines
        .iter()
        .map(|l| {
            if l.len() >= min_indent {
                &l[min_indent..]
            } else {
                l.trim()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
