use super::*;
use crate::diagnostics::Diagnostic;
use token::*;

impl<'a> Lexer<'a> {
    // ── Characters ──

    /// Lex a single-quoted RAW string (ruby-naming.spec.md §3.10a):
    /// content verbatim, no escape processing, no `#{}` interpolation
    /// (double quotes interpolate; char literals are `?a`). Single-line —
    /// the closing `'` must appear before the end of the line.
    ///
    /// There is NO `'a` lifetime sigil (ruby-naming.spec.md §3.3 / G7):
    /// lifetimes are bare lowercase names in the `[...]` parameter slot,
    /// no sigil. A leading `'` with no closing quote on the line is an
    /// unterminated raw string, not a lifetime.
    pub(super) fn lex_single_quote(&mut self) {
        let start_byte = self.byte_pos;
        let start_line = self.line;
        let start_col = self.column;

        self.advance(); // opening '
        let content_start = self.pos;
        while !self.is_at_end() && self.current() != '\'' && self.current() != '\n' {
            self.advance();
        }

        if self.is_at_end() || self.current() != '\'' {
            let span = self.make_span(start_byte, start_line, start_col);
            self.diagnostics.push(Diagnostic::error_with_code(
                "unterminated raw string literal (single quotes are raw strings; \
                 lifetimes use a bare lowercase name in `[...]`, no `'` sigil)",
                span,
                "E0002",
            ));
            // Recover: emit what we scanned so downstream parsing continues.
            let content: String = self.chars[content_start..self.pos].iter().collect();
            self.emit(
                TokenKind::StringLiteral(content),
                start_byte,
                start_line,
                start_col,
            );
            return;
        }

        let content: String = self.chars[content_start..self.pos].iter().collect();
        self.advance(); // closing '
        self.emit(
            TokenKind::StringLiteral(content),
            start_byte,
            start_line,
            start_col,
        );
    }

    /// Lex a Ruby-style `?a` / `?\n` character literal. The opening `?`
    /// has already been consumed; `self.current()` is the char (or the
    /// `\` of an escape). Caller guarantees we're in an expression-
    /// context position (so postfix `?` / `?.` / optional-type `T?`
    /// stay operators).
    pub(super) fn lex_question_char(&mut self, start_byte: usize, start_line: u32, start_col: u32) {
        let ch = if self.current() == '\\' {
            match self.lex_escape_sequence() {
                Ok(c) => c,
                Err(()) => return,
            }
        } else {
            self.advance()
        };
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
        // AND we must NOT be in a tuple-field-access context.
        //
        // Background: for `t.0.10`, the lexer historically fused `0.10`
        // into `FloatLiteral(0.1_f64)` because f64 round-trip drops the
        // trailing zero. The parser then `format!("{}", 0.1)`-split that
        // back into `["0", "1"]` and accessed field 1 instead of field
        // 10 — silent miscompile (parser/expr/calls.rs:72-96).
        //
        // The clean fix is to refuse the float fusion entirely when the
        // PREVIOUS emitted token is `Dot`: in that context the user is
        // chaining tuple-field accesses, so emit the integer alone and
        // leave the trailing `.<digit>` for the next lex iteration to
        // produce as a separate `Dot` + `IntLiteral` pair. Pin test:
        // tuple-field roundtrip preserves multi-digit indices.
        let prev_was_dot = self
            .tokens
            .last()
            .is_some_and(|t| matches!(t.kind, TokenKind::Dot));
        let is_float = !prev_was_dot
            && !self.is_at_end()
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

        // Ruby-style suffixes on lowercase (method/value) identifiers:
        //   * `!`  — bang methods (`sort!`, `lock!`).
        //   * `?`  — predicate methods (`empty?`, `include?`, `any?`).
        // `?` belongs to method names; safe navigation is `&.` (Ruby), not
        // `?.`, so there is no conflict — `coll.empty?` is the method name
        // `empty?`. A trailing `?` after `)`/`]` (the try operator) is
        // unaffected — it is never part of an identifier lex. Uppercase
        // type identifiers do NOT absorb `?` (so `T?` stays an optional type).
        if !self.is_at_end() && (self.current() == '!' || self.current() == '?') {
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

        // ruby-naming.spec.md §3.10: `None` is not a valid spelling — the
        // single empty literal is `nil` (which covers Option::None, the
        // null pointer, and unit). Reject the identifier `None` uniformly
        // here (expression, pattern, and type positions all funnel through
        // this lexer path) with a fix-it pointing at `nil`. We still emit a
        // TypeIdentifier token so the parser keeps making progress and the
        // user sees this one clean error rather than a parse cascade.
        if ident == "None" {
            let span = self.make_span(start_byte, start_line, start_col);
            self.diagnostics.push(Diagnostic::error_with_code(
                "`None` is not valid in Ruxen — use `nil` instead (the single \
                 empty literal: Option::None, null, and unit)",
                span,
                "E0008",
            ));
            self.emit(
                TokenKind::TypeIdentifier(ident),
                start_byte,
                start_line,
                start_col,
            );
            return;
        }

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
                // JS/Ruby positional rule: a `/` opens a `RegexLiteral`
                // only when we're in an expression-context position
                // (see `prev_token_starts_expr_context`). The last
                // emitted token (including a fresh `Newline`) drives
                // the decision — `Newline` itself counts as
                // expression-context because a fresh statement is
                // about to begin.
                let prev = self.tokens.last().map(|t| t.kind.clone());
                if prev_token_starts_expr_context(prev.as_ref()) {
                    self.lex_regex_literal(start_byte, start_line, start_col);
                } else if !self.is_at_end() && self.current() == '=' {
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
                } else if !self.is_at_end() && self.current() == '.' {
                    // `&.` — Ruby safe navigation (`h&.hello&.now`).
                    self.advance();
                    self.emit(TokenKind::AmpDot, start_byte, start_line, start_col);
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
                    if !self.is_at_end() && self.current() == '.' {
                        // `...` — Ruby exclusive range.
                        self.advance();
                        self.emit(TokenKind::DotDotDot, start_byte, start_line, start_col);
                    } else if !self.is_at_end() && self.current() == '=' {
                        // `..=` is the retired Rust inclusive form. There is no
                        // `DotDotEq` token — reject here with a fix-it and
                        // recover as `..` (Ruby's inclusive range) by consuming
                        // the stray `=`.
                        self.advance(); // consume '='
                        let span = self.make_span(start_byte, start_line, start_col);
                        self.diagnostics.push(Diagnostic::error_with_code(
                            "`..=` is not valid Ruxen syntax — ranges are Ruby-shaped: \
                             `..` is inclusive, `...` is exclusive (use `..` here)",
                            span,
                            "E0009",
                        ));
                        self.emit(TokenKind::DotDot, start_byte, start_line, start_col);
                    } else {
                        // `..` — Ruby inclusive range.
                        self.emit(TokenKind::DotDot, start_byte, start_line, start_col);
                    }
                } else {
                    self.emit(TokenKind::Dot, start_byte, start_line, start_col);
                }
            }
            '?' => {
                // Safe navigation is `&.` (not `?.`), and a trailing `?` on
                // an identifier is absorbed as a predicate-method name — so a
                // standalone `?` here is either a char literal (`?a`) or the
                // try operator.
                if !self.is_at_end()
                    && {
                        // ruby-naming.spec.md §3.10a: `?a` / `?\n` is a char
                        // literal, but only in an expression-context position
                        // so postfix-`?` (try) and optional-type `T?` keep
                        // their operator meaning. The char must follow `?`
                        // immediately (no whitespace) and be an alphanumeric,
                        // `_`, or the start of an escape.
                        let c = self.current();
                        let prev = self.tokens.last().map(|t| t.kind.clone());
                        prev_token_starts_expr_context(prev.as_ref())
                            && (c == '\\' || c.is_ascii_alphanumeric() || c == '_')
                    }
                {
                    self.lex_question_char(start_byte, start_line, start_col);
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
            '~' => {
                // `~=` regex-match operator. There is no bare `~` in
                // Ruxen today (no bitwise-not — use `!x` and `^` for
                // the integer surface); a lone `~` falls through to
                // the unexpected-character path below by design.
                if !self.is_at_end() && self.current() == '=' {
                    self.advance();
                    self.emit(TokenKind::TildeEq, start_byte, start_line, start_col);
                } else {
                    let span = self.make_span(start_byte, start_line, start_col);
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!("unexpected character: '{}'", ch),
                        span,
                        "E0006",
                    ));
                }
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

/// Returns true when the most-recently-emitted non-trivia token is in
/// "expression-expected" position — i.e. the next non-whitespace
/// character could legally start a fresh expression. Used by the `/`
/// lex arm to decide between `Slash` (division) and `RegexLiteral`.
/// See `docs/superpowers/specs/2026-05-29-std-regex-design.md` for
/// the full expression-context token set.
pub(super) fn prev_token_starts_expr_context(prev: Option<&TokenKind>) -> bool {
    use TokenKind::*;
    let Some(t) = prev else {
        // No previous token — start of file. A `/` at SOF must be a
        // regex literal (division has no LHS).
        return true;
    };
    matches!(
        t,
        // Structural / position markers that imply "expression next".
        Eof | Newline
        // Opening delimiters.
        | LParen | LBracket | LBrace
        // Separators.
        | Comma | Semicolon | Colon | FatArrow | Arrow
        // Assignment / comparison operators (after these, an
        // expression must follow).
        | Eq | EqEq | NotEq | Lt | Gt | LtEq | GtEq
        // Arithmetic operators and `/=` (also a context-resetter
        // for the RHS).
        | Plus | Minus | Star | SlashEq | Percent
        // Logical operators.
        | AmpAmp | PipePipe | Bang
        // The regex-match operator itself.
        | TildeEq
        // Keywords that introduce an expression position.
        | If | While | Match | Return | When | Else | Elsif
        | In | Do | Unless
    )
}

impl<'a> Lexer<'a> {
    /// Lex a `/pat/flags` regex literal. The opening `/` has already
    /// been consumed by [`lex_operator_or_punct`]. Emits the token on
    /// success; pushes an E1700/E1701/E1703 diagnostic on failure (and
    /// still emits a best-effort token / recovers at end-of-line so
    /// downstream parsing isn't poisoned).
    pub(super) fn lex_regex_literal(&mut self, start_byte: usize, start_line: u32, start_col: u32) {
        // Scan the pattern body.
        let mut pattern = String::new();
        let mut bracket_depth: usize = 0;
        let mut prev_char_escaped = false;
        let mut empty_pattern = false;

        // Special-case: empty pattern `//` — closing `/` is the
        // immediate next character.
        if !self.is_at_end() && self.current() == '/' {
            let span = self.make_span(start_byte, start_line, start_col);
            self.diagnostics.push(Diagnostic::error_with_code(
                "empty regex pattern",
                span,
                "E1703",
            ));
            self.advance(); // consume the closing `/`
            empty_pattern = true;
        }

        if !empty_pattern {
            loop {
                if self.is_at_end() {
                    let span = self.make_span(start_byte, start_line, start_col);
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "unterminated regex literal",
                        span,
                        "E1701",
                    ));
                    // Recovery: emit a best-effort token so downstream
                    // parsing has something to chew on.
                    self.emit(
                        TokenKind::RegexLiteral {
                            pattern,
                            flags: String::new(),
                        },
                        start_byte,
                        start_line,
                        start_col,
                    );
                    return;
                }
                let c = self.current();
                if c == '\n' {
                    let span = self.make_span(start_byte, start_line, start_col);
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "unterminated regex literal",
                        span,
                        "E1701",
                    ));
                    self.emit(
                        TokenKind::RegexLiteral {
                            pattern,
                            flags: String::new(),
                        },
                        start_byte,
                        start_line,
                        start_col,
                    );
                    return;
                }

                // Bracket-depth only tracks unescaped class openers.
                // PCRE2 handles `()` and `{}` inside the pattern; we
                // only need `[…]` here so the `/` inside a class is
                // not treated as a terminator.
                if !prev_char_escaped {
                    if c == '[' {
                        bracket_depth += 1;
                    } else if c == ']' && bracket_depth > 0 {
                        bracket_depth -= 1;
                    }
                }

                // Closing `/` only when not escaped AND not inside a
                // character class.
                if c == '/' && !prev_char_escaped && bracket_depth == 0 {
                    self.advance(); // consume closing /
                    break;
                }

                // Track escape for the NEXT iteration. Consecutive
                // backslashes alternate the escape state, so `\\/` is
                // `\\` + `/` (closing the literal).
                prev_char_escaped = (c == '\\') && !prev_char_escaped;
                pattern.push(c);
                self.advance();
            }
        }

        // Flag suffix. Each of `i m s g x` may appear at most once.
        // Anything else (any other ASCII letter, or a repeat) is E1700.
        let mut flags = String::new();
        loop {
            if self.is_at_end() {
                break;
            }
            let c = self.current();
            if !c.is_ascii_alphabetic() {
                break;
            }
            match c {
                'i' | 'm' | 's' | 'g' | 'x' => {
                    if flags.contains(c) {
                        let span = self.make_span(start_byte, start_line, start_col);
                        self.diagnostics.push(Diagnostic::error_with_code(
                            format!("regex flag '{}' specified more than once", c),
                            span,
                            "E1700",
                        ));
                        // Consume so we don't loop forever on the same
                        // duplicate character; flag set stays as-is.
                        self.advance();
                    } else {
                        flags.push(c);
                        self.advance();
                    }
                }
                other => {
                    let span = self.make_span(start_byte, start_line, start_col);
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!("unrecognised regex flag '{}'", other),
                        span,
                        "E1700",
                    ));
                    // Consume the bad flag char so we can keep scanning;
                    // if anything else valid follows we still pick it up.
                    self.advance();
                }
            }
        }

        self.emit(
            TokenKind::RegexLiteral { pattern, flags },
            start_byte,
            start_line,
            start_col,
        );
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
