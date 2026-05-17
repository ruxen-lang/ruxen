use super::*;
use crate::diagnostics::Diagnostic;
use token::*;

impl<'a> Lexer<'a> {
    // ── Characters ──

    pub(super) fn lex_char(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;

        // Disambiguate lifetime vs char literal:
        // 'a' is a char, 'a (not followed by ') is a lifetime
        if self
            .peek_at(1)
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            // Check if this is 'x' (char) or 'ident (lifetime)
            let mut look = 2;
            while self
                .peek_at(look)
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                look += 1;
            }
            if self.peek_at(look) != Some('\'') {
                // It's a lifetime: 'a, 'input, etc.
                self.advance(); // '
                let name_start = self.pos;
                while !self.is_at_end()
                    && (self.current().is_ascii_alphanumeric() || self.current() == '_')
                {
                    self.advance();
                }
                let name: String = self.chars[name_start..self.pos].iter().collect();
                self.emit(TokenKind::Lifetime(name), start_byte, start_line, start_col);
                return;
            }
        }

        self.advance(); // opening '

        if self.is_at_end() {
            let span = self.make_span(start_byte, start_line, start_col);
            self.diagnostics.push(Diagnostic::error_with_code(
                "unterminated character literal",
                span,
                "E0005",
            ));
            return;
        }

        let ch = if self.current() == '\\' {
            match self.lex_escape_sequence() {
                Ok(c) => c,
                Err(()) => return,
            }
        } else {
            self.advance()
        };

        if self.is_at_end() || self.current() != '\'' {
            let span = self.make_span(start_byte, start_line, start_col);
            self.diagnostics.push(Diagnostic::error_with_code(
                "unterminated character literal, expected closing '",
                span,
                "E0005",
            ));
            return;
        }

        self.advance(); // closing '
        self.emit(
            TokenKind::CharLiteral(ch),
            start_byte,
            start_line,
            start_col,
        );
    }

    // ── Numbers ──

    pub(super) fn lex_number(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;
        let start_pos = self.pos;

        let first = self.advance();

        // Check for prefixed literals: 0x, 0b, 0o
        if first == '0' && !self.is_at_end() {
            match self.current() {
                'x' | 'X' => {
                    self.advance();
                    self.lex_prefixed_int(start_byte, start_line, start_col, 16, "hex");
                    return;
                }
                'b' if self
                    .peek_at(1)
                    .is_some_and(|c| c == '0' || c == '1' || c == '_') =>
                {
                    self.advance();
                    self.lex_prefixed_int(start_byte, start_line, start_col, 2, "binary");
                    return;
                }
                'o' | 'O' => {
                    self.advance();
                    self.lex_prefixed_int(start_byte, start_line, start_col, 8, "octal");
                    return;
                }
                _ => {}
            }
        }

        // Decimal integer or float
        while !self.is_at_end() && (self.current().is_ascii_digit() || self.current() == '_') {
            self.advance();
        }

        // Check for float: must be '.' followed by a digit (NOT '..' which is range)
        let is_float = !self.is_at_end()
            && self.current() == '.'
            && self.peek_at(1).is_some_and(|c| c.is_ascii_digit());

        if is_float {
            self.advance(); // .
            while !self.is_at_end() && (self.current().is_ascii_digit() || self.current() == '_') {
                self.advance();
            }

            // Scientific notation
            if !self.is_at_end() && (self.current() == 'e' || self.current() == 'E') {
                self.advance();
                if !self.is_at_end() && (self.current() == '+' || self.current() == '-') {
                    self.advance();
                }
                while !self.is_at_end()
                    && (self.current().is_ascii_digit() || self.current() == '_')
                {
                    self.advance();
                }
            }

            // Float suffix
            let suffix = self.try_float_suffix();

            let raw: String = self.chars[start_pos..self.pos]
                .iter()
                .filter(|c| **c != '_')
                .collect();
            // Strip suffix from raw
            let num_str = strip_suffix_str(&raw, &suffix);

            match num_str.parse::<f64>() {
                Ok(val) => self.emit(
                    TokenKind::FloatLiteral(val, suffix),
                    start_byte,
                    start_line,
                    start_col,
                ),
                Err(_) => {
                    let span = self.make_span(start_byte, start_line, start_col);
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!("invalid float literal: {}", num_str),
                        span,
                        "E0004",
                    ));
                }
            }
        } else {
            // Integer - check for suffix
            let suffix = self.try_int_suffix();

            let raw: String = self.chars[start_pos..self.pos]
                .iter()
                .filter(|c| **c != '_')
                .collect();
            let num_str = strip_suffix_str(&raw, &suffix);

            match num_str.parse::<i64>() {
                Ok(val) => self.emit(
                    TokenKind::IntLiteral(val, suffix),
                    start_byte,
                    start_line,
                    start_col,
                ),
                Err(_) => {
                    let span = self.make_span(start_byte, start_line, start_col);
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!("invalid integer literal: {}", num_str),
                        span,
                        "E0004",
                    ));
                }
            }
        }
    }

    pub(super) fn lex_prefixed_int(
        &mut self,
        start_byte: usize,
        start_line: u32,
        start_col: u32,
        radix: u32,
        name: &str,
    ) {
        let digit_start = self.pos;

        let valid_digit = |c: char| -> bool {
            match radix {
                2 => c == '0' || c == '1',
                8 => ('0'..='7').contains(&c),
                16 => c.is_ascii_hexdigit(),
                _ => false,
            }
        };

        while !self.is_at_end() && (valid_digit(self.current()) || self.current() == '_') {
            self.advance();
        }

        if self.pos == digit_start {
            let span = self.make_span(start_byte, start_line, start_col);
            self.diagnostics.push(Diagnostic::error_with_code(
                format!("invalid {} literal: no digits after prefix", name),
                span,
                "E0004",
            ));
            return;
        }

        let suffix = self.try_int_suffix();

        let digits: String = self.chars[digit_start..self.pos]
            .iter()
            .filter(|c| **c != '_')
            .collect();
        let digit_str = strip_suffix_str(&digits, &suffix);

        match i64::from_str_radix(digit_str, radix) {
            Ok(val) => self.emit(
                TokenKind::IntLiteral(val, suffix),
                start_byte,
                start_line,
                start_col,
            ),
            Err(_) => {
                let span = self.make_span(start_byte, start_line, start_col);
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!("invalid {} literal", name),
                    span,
                    "E0004",
                ));
            }
        }
    }

    pub(super) fn try_int_suffix(&mut self) -> Option<NumericSuffix> {
        self.try_numeric_suffix(true)
    }

    pub(super) fn try_float_suffix(&mut self) -> Option<NumericSuffix> {
        self.try_numeric_suffix(false)
    }

    pub(super) fn try_numeric_suffix(&mut self, is_int: bool) -> Option<NumericSuffix> {
        if self.is_at_end() {
            return None;
        }

        let remaining: String = self.chars[self.pos..].iter().collect();

        // Try longest suffixes first
        let suffixes: &[(&str, NumericSuffix, bool)] = &[
            ("isize", NumericSuffix::ISize, true),
            ("usize", NumericSuffix::USize, true),
            ("i64", NumericSuffix::I64, true),
            ("i32", NumericSuffix::I32, true),
            ("i16", NumericSuffix::I16, true),
            ("i8", NumericSuffix::I8, true),
            ("u64", NumericSuffix::U64, true),
            ("u32", NumericSuffix::U32, true),
            ("u16", NumericSuffix::U16, true),
            ("u8", NumericSuffix::U8, true),
            ("f64", NumericSuffix::F64, false),
            ("f32", NumericSuffix::F32, false),
            ("u", NumericSuffix::U, true),
        ];

        for &(text, ref suffix, int_only) in suffixes {
            if !is_int && int_only && *suffix != NumericSuffix::U {
                // float can have f32/f64 but not int suffixes
                continue;
            }
            if is_int && !int_only {
                // int can have int suffixes and also f32/f64 (wait — no, skip float suffixes for ints)
                continue;
            }
            if remaining.starts_with(text) {
                // Ensure the suffix isn't followed by identifier chars
                let after = remaining.chars().nth(text.len());
                if after.is_none_or(|c| !c.is_alphanumeric() && c != '_') {
                    for _ in 0..text.len() {
                        self.advance();
                    }
                    return Some(*suffix);
                }
            }
        }

        // Special case: float suffixes on int context are actually floats
        // But we handle that at parse level. For now, just return None.
        None
    }

    // ── Identifiers & Keywords ──

    pub(super) fn lex_identifier_or_keyword(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;
        let start_pos = self.pos;

        // Consume [a-z_][a-zA-Z0-9_]*
        self.advance();
        while !self.is_at_end() && (self.current().is_ascii_alphanumeric() || self.current() == '_')
        {
            self.advance();
        }

        let ident: String = self.chars[start_pos..self.pos].iter().collect();

        // Check for `&mut` — if we just lexed "mut" and the previous token was `&`
        // Actually, `&mut` is handled in operator lexing. Here we just handle identifiers.

        // Check for ! suffix on identifiers (e.g., unwrap!, panic!)
        // Note: ? is NOT consumed as an identifier suffix because it conflicts
        // with the ? try operator and ?. safe navigation. The parser will handle
        // method names like is_empty? by combining identifier + ? tokens.
        if !self.is_at_end() && self.current() == '!' {
            let suffix = self.advance();
            let full_ident: String = format!("{}{}", ident, suffix);
            self.emit(
                TokenKind::Identifier(full_ident),
                start_byte,
                start_line,
                start_col,
            );
            return;
        }

        // Check if it's a keyword
        if let Some(kw) = lookup_keyword(&ident) {
            self.emit(kw, start_byte, start_line, start_col);
        } else {
            self.emit(
                TokenKind::Identifier(ident),
                start_byte,
                start_line,
                start_col,
            );
        }
    }

    pub(super) fn lex_type_identifier_or_keyword(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;
        let start_pos = self.pos;

        self.advance();
        while !self.is_at_end() && (self.current().is_ascii_alphanumeric() || self.current() == '_')
        {
            self.advance();
        }

        let ident: String = self.chars[start_pos..self.pos].iter().collect();

        // Check for ! suffix (same logic as identifiers)
        if !self.is_at_end() && self.current() == '!' {
            let suffix = self.advance();
            let full_ident: String = format!("{}{}", ident, suffix);
            self.emit(
                TokenKind::TypeIdentifier(full_ident),
                start_byte,
                start_line,
                start_col,
            );
            return;
        }

        // Keywords that start with uppercase
        if let Some(kw) = lookup_keyword(&ident) {
            self.emit(kw, start_byte, start_line, start_col);
        } else {
            self.emit(
                TokenKind::TypeIdentifier(ident),
                start_byte,
                start_line,
                start_col,
            );
        }
    }

    // ── Operators & Punctuation ──

    pub(super) fn lex_operator_or_punct(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;
        let ch = self.advance();

        match ch {
            '+' => {
                if !self.is_at_end() && self.current() == '=' {
                    self.advance();
                    self.emit(TokenKind::PlusEq, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Plus, start_byte, start_line, start_col);
                }
            }
            '-' => {
                if !self.is_at_end() && self.current() == '>' {
                    self.advance();
                    self.emit(TokenKind::Arrow, start_byte, start_line, start_col);
                } else if !self.is_at_end() && self.current() == '=' {
                    self.advance();
                    self.emit(TokenKind::MinusEq, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Minus, start_byte, start_line, start_col);
                }
            }
            '*' => {
                if !self.is_at_end() && self.current() == '=' {
                    self.advance();
                    self.emit(TokenKind::StarEq, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Star, start_byte, start_line, start_col);
                }
            }
            '/' => {
                if !self.is_at_end() && self.current() == '=' {
                    self.advance();
                    self.emit(TokenKind::SlashEq, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Slash, start_byte, start_line, start_col);
                }
            }
            '%' => {
                if !self.is_at_end() && self.current() == '=' {
                    self.advance();
                    self.emit(TokenKind::PercentEq, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Percent, start_byte, start_line, start_col);
                }
            }
            '=' => {
                if !self.is_at_end() && self.current() == '=' {
                    self.advance();
                    self.emit(TokenKind::EqEq, start_byte, start_line, start_col);
                } else if !self.is_at_end() && self.current() == '>' {
                    self.advance();
                    self.emit(TokenKind::FatArrow, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Eq, start_byte, start_line, start_col);
                }
            }
            '!' => {
                if !self.is_at_end() && self.current() == '=' {
                    self.advance();
                    self.emit(TokenKind::NotEq, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Bang, start_byte, start_line, start_col);
                }
            }
            '<' => {
                if !self.is_at_end() && self.current() == '=' {
                    self.advance();
                    self.emit(TokenKind::LtEq, start_byte, start_line, start_col);
                } else if !self.is_at_end() && self.current() == '<' {
                    self.advance();
                    self.emit(TokenKind::Shl, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Lt, start_byte, start_line, start_col);
                }
            }
            '>' => {
                if !self.is_at_end() && self.current() == '=' {
                    self.advance();
                    self.emit(TokenKind::GtEq, start_byte, start_line, start_col);
                } else if !self.is_at_end() && self.current() == '>' {
                    self.advance();
                    self.emit(TokenKind::Shr, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Gt, start_byte, start_line, start_col);
                }
            }
            '&' => {
                if !self.is_at_end() && self.current() == '&' {
                    self.advance();
                    self.emit(TokenKind::AmpAmp, start_byte, start_line, start_col);
                } else if !self.is_at_end() && self.current() == 'v' {
                    // Check for &var — single-token writable-reference marker
                    // (variant kept as `AmpMut` internally; surface is `&var`).
                    if self.peek_at(1) == Some('a') && self.peek_at(2) == Some('r') {
                        // Make sure 'var' is a complete word
                        let after_var = self.peek_at(3);
                        if after_var.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_') {
                            self.advance(); // v
                            self.advance(); // a
                            self.advance(); // r
                            self.emit(TokenKind::AmpMut, start_byte, start_line, start_col);
                            return;
                        }
                    }
                    self.emit(TokenKind::Amp, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Amp, start_byte, start_line, start_col);
                }
            }
            '|' => {
                if !self.is_at_end() && self.current() == '|' {
                    self.advance();
                    self.emit(TokenKind::PipePipe, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Pipe, start_byte, start_line, start_col);
                }
            }
            '^' => {
                self.emit(TokenKind::Caret, start_byte, start_line, start_col);
            }
            '.' => {
                if !self.is_at_end() && self.current() == '.' {
                    self.advance();
                    if !self.is_at_end() && self.current() == '=' {
                        self.advance();
                        self.emit(TokenKind::DotDotEq, start_byte, start_line, start_col);
                    } else {
                        self.emit(TokenKind::DotDot, start_byte, start_line, start_col);
                    }
                } else {
                    self.emit(TokenKind::Dot, start_byte, start_line, start_col);
                }
            }
            '?' => {
                if !self.is_at_end() && self.current() == '.' {
                    self.advance();
                    self.emit(TokenKind::QuestionDot, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Question, start_byte, start_line, start_col);
                }
            }
            '@' => {
                self.emit(TokenKind::At, start_byte, start_line, start_col);
            }
            ':' => {
                if !self.is_at_end() && self.current() == ':' {
                    self.advance();
                    self.emit(TokenKind::ColonColon, start_byte, start_line, start_col);
                } else {
                    self.emit(TokenKind::Colon, start_byte, start_line, start_col);
                }
            }
            ';' => {
                self.emit(TokenKind::Semicolon, start_byte, start_line, start_col);
            }
            ',' => {
                self.emit(TokenKind::Comma, start_byte, start_line, start_col);
            }
            '(' => {
                self.emit(TokenKind::LParen, start_byte, start_line, start_col);
            }
            ')' => {
                self.emit(TokenKind::RParen, start_byte, start_line, start_col);
            }
            '[' => {
                self.emit(TokenKind::LBracket, start_byte, start_line, start_col);
            }
            ']' => {
                self.emit(TokenKind::RBracket, start_byte, start_line, start_col);
            }
            '{' => {
                self.emit(TokenKind::LBrace, start_byte, start_line, start_col);
            }
            '}' => {
                self.emit(TokenKind::RBrace, start_byte, start_line, start_col);
            }
            '\\' => {
                // Line continuation with backslash — just skip the newline
                if !self.is_at_end() && self.current() == '\n' {
                    self.advance();
                } else {
                    let span = self.make_span(start_byte, start_line, start_col);
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!("unexpected character: '{}'", ch),
                        span,
                        "E0006",
                    ));
                }
            }
            _ => {
                let span = self.make_span(start_byte, start_line, start_col);
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!("unexpected character: '{}'", ch),
                    span,
                    "E0006",
                ));
            }
        }
    }
}

/// Strip a numeric suffix string from a raw number string for parsing.
pub(super) fn strip_suffix_str<'a>(raw: &'a str, suffix: &Option<NumericSuffix>) -> &'a str {
    match suffix {
        None => raw,
        Some(s) => {
            let suffix_len = match s {
                NumericSuffix::I8 | NumericSuffix::U8 => 2,
                NumericSuffix::I16
                | NumericSuffix::U16
                | NumericSuffix::I32
                | NumericSuffix::U32
                | NumericSuffix::I64
                | NumericSuffix::U64
                | NumericSuffix::F32
                | NumericSuffix::F64 => 3,
                NumericSuffix::U => 1,
                NumericSuffix::ISize => 5,
                NumericSuffix::USize => 5,
            };
            &raw[..raw.len() - suffix_len]
        }
    }
}
