pub mod token;

mod format_specs;
mod strings;
mod tokens;

use crate::diagnostics::Diagnostic;
use token::*;

pub struct Lexer<'a> {
    source: &'a str,
    chars: Vec<char>,
    pos: usize,      // index into chars
    byte_pos: usize, // byte offset in source
    line: u32,
    column: u32,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            chars: source.chars().collect(),
            pos: 0,
            byte_pos: 0,
            line: 1,
            column: 1,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    pub fn source(&self) -> &'a str {
        self.source
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, Vec<Diagnostic>> {
        while !self.is_at_end() {
            self.skip_whitespace();
            if self.is_at_end() {
                break;
            }

            let ch = self.current();

            match ch {
                '\n' => self.lex_newline(),
                '#' => self.lex_comment_or_hash(),
                '"' => self.lex_string(),
                '\'' => self.lex_char(),
                'r' if self.peek_at(1) == Some('"') || self.peek_at(1) == Some('#') => {
                    self.lex_raw_string()
                }
                '0'..='9' => self.lex_number(),
                'a'..='z' | '_' => self.lex_identifier_or_keyword(),
                'A'..='Z' => self.lex_type_identifier_or_keyword(),
                _ => self.lex_operator_or_punct(),
            }
        }

        // Emit EOF
        let eof_span = Span::new(self.byte_pos, self.byte_pos, self.line, self.column);
        self.tokens.push(Token::new(TokenKind::Eof, eof_span));

        if self
            .diagnostics
            .iter()
            .any(|d| d.level == crate::diagnostics::DiagnosticLevel::Error)
        {
            Err(self.diagnostics.clone())
        } else {
            Ok(self.tokens.clone())
        }
    }

    // ── Helpers ──

    fn is_at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn current(&self) -> char {
        self.chars[self.pos]
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> char {
        let ch = self.chars[self.pos];
        self.byte_pos += ch.len_utf8();
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        ch
    }

    fn skip_whitespace(&mut self) {
        while !self.is_at_end() {
            match self.current() {
                ' ' | '\t' | '\r' => {
                    self.advance();
                }
                _ => break,
            }
        }
    }

    fn make_span(&self, start_byte: usize, start_line: u32, start_col: u32) -> Span {
        Span::new(start_byte, self.byte_pos, start_line, start_col)
    }

    fn emit(&mut self, kind: TokenKind, start_byte: usize, start_line: u32, start_col: u32) {
        let span = self.make_span(start_byte, start_line, start_col);
        self.tokens.push(Token::new(kind, span));
    }

    /// Returns the last non-Newline token kind, for deciding line continuation.
    fn last_significant_token(&self) -> Option<&TokenKind> {
        self.tokens
            .iter()
            .rev()
            .find(|t| t.kind != TokenKind::Newline)
            .map(|t| &t.kind)
    }

    // ── Newline ──

    fn lex_newline(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;

        // Consume the newline
        self.advance();

        // Consume consecutive newlines and whitespace between them
        while !self.is_at_end() {
            match self.current() {
                '\n' => {
                    self.advance();
                }
                ' ' | '\t' | '\r' => {
                    self.advance();
                }
                _ => break,
            }
        }

        // Suppress newline if the last token implies continuation
        if let Some(last) = self.last_significant_token() {
            if last.continues_line() {
                return;
            }
        }

        // Don't emit newline at the start (no tokens yet) or after another newline
        if self.tokens.is_empty() {
            return;
        }
        if let Some(last) = self.tokens.last() {
            if last.kind == TokenKind::Newline {
                return;
            }
        }

        self.emit(TokenKind::Newline, start_byte, start_line, start_col);
    }

    // ── Comments ──

    fn lex_comment_or_hash(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;

        // We are at '#'
        if self.peek_at(1) == Some('=') {
            // Block comment #= ... =#
            self.lex_block_comment(start_byte, start_line, start_col);
        } else if self.peek_at(1) == Some('#') {
            // Doc comment ##
            self.lex_doc_comment(start_byte, start_line, start_col);
        } else {
            // Line comment
            while !self.is_at_end() && self.current() != '\n' {
                self.advance();
            }
            // Don't emit anything for line comments; the newline will be handled normally
        }
    }

    fn lex_block_comment(&mut self, start_byte: usize, start_line: u32, start_col: u32) {
        self.advance(); // #
        self.advance(); // =
        let mut depth = 1u32;

        while !self.is_at_end() && depth > 0 {
            if self.current() == '#' && self.peek_at(1) == Some('=') {
                self.advance();
                self.advance();
                depth += 1;
            } else if self.current() == '=' && self.peek_at(1) == Some('#') {
                self.advance();
                self.advance();
                depth -= 1;
            } else {
                self.advance();
            }
        }

        if depth > 0 {
            let span = self.make_span(start_byte, start_line, start_col);
            self.diagnostics.push(Diagnostic::error_with_code(
                "unterminated block comment",
                span,
                "E0001",
            ));
        }
    }

    fn lex_doc_comment(&mut self, start_byte: usize, start_line: u32, start_col: u32) {
        self.advance(); // first #
        self.advance(); // second #

        // Skip optional leading space
        if !self.is_at_end() && self.current() == ' ' {
            self.advance();
        }

        let content_start = self.pos;
        while !self.is_at_end() && self.current() != '\n' {
            self.advance();
        }
        let content: String = self.chars[content_start..self.pos].iter().collect();
        self.emit(
            TokenKind::DocComment(content),
            start_byte,
            start_line,
            start_col,
        );
    }
}

#[cfg(test)]
mod tests;
