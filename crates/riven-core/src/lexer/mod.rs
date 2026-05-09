pub mod token;

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

    // ── Strings ──

    fn lex_string(&mut self) {
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
                let expr_tokens = self.lex_interpolation_expr();
                parts.push(StringPart::Expr(expr_tokens));
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

    fn lex_multiline_string(&mut self, start_byte: usize, start_line: u32, start_col: u32) {
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

    fn lex_raw_string(&mut self) {
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

    fn lex_escape_sequence(&mut self) -> Result<char, ()> {
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

    fn lex_interpolation_expr(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();
        let mut depth = 1u32; // we've already consumed #{

        while !self.is_at_end() && depth > 0 {
            self.skip_whitespace();
            if self.is_at_end() {
                break;
            }

            let ch = self.current();

            if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    self.advance();
                    break;
                }
                let sb = self.byte_pos;
                let sl = self.line;
                let sc = self.column;
                self.advance();
                tokens.push(Token::new(
                    TokenKind::RBrace,
                    Span::new(sb, self.byte_pos, sl, sc),
                ));
                continue;
            }

            if ch == '{' {
                depth += 1;
                let sb = self.byte_pos;
                let sl = self.line;
                let sc = self.column;
                self.advance();
                tokens.push(Token::new(
                    TokenKind::LBrace,
                    Span::new(sb, self.byte_pos, sl, sc),
                ));
                continue;
            }

            // Lex one token and capture it
            let before = self.tokens.len();
            match ch {
                '\n' => {
                    self.advance();
                    continue;
                }
                '"' => self.lex_string(),
                '\'' => self.lex_char(),
                '0'..='9' => self.lex_number(),
                'a'..='z' | '_' => self.lex_identifier_or_keyword(),
                'A'..='Z' => self.lex_type_identifier_or_keyword(),
                _ => self.lex_operator_or_punct(),
            }

            // Move any newly emitted tokens to our local vec
            while self.tokens.len() > before {
                tokens.push(self.tokens.remove(before));
            }
        }

        tokens
    }

    // ── Characters ──

    fn lex_char(&mut self) {
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

    fn lex_number(&mut self) {
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

    fn lex_prefixed_int(
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

    fn try_int_suffix(&mut self) -> Option<NumericSuffix> {
        self.try_numeric_suffix(true)
    }

    fn try_float_suffix(&mut self) -> Option<NumericSuffix> {
        self.try_numeric_suffix(false)
    }

    fn try_numeric_suffix(&mut self, is_int: bool) -> Option<NumericSuffix> {
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

    fn lex_identifier_or_keyword(&mut self) {
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

    fn lex_type_identifier_or_keyword(&mut self) {
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

    fn lex_operator_or_punct(&mut self) {
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
                } else if !self.is_at_end() && self.current() == 'm' {
                    // Check for &mut
                    if self.peek_at(1) == Some('u') && self.peek_at(2) == Some('t') {
                        // Make sure 'mut' is a complete word
                        let after_mut = self.peek_at(3);
                        if after_mut.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_') {
                            self.advance(); // m
                            self.advance(); // u
                            self.advance(); // t
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

/// Strip a numeric suffix string from a raw number string for parsing.
fn strip_suffix_str<'a>(raw: &'a str, suffix: &Option<NumericSuffix>) -> &'a str {
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

#[cfg(test)]
mod tests;
