use super::*;
use crate::diagnostics::Diagnostic;
use token::*;

impl<'a> Lexer<'a> {
    /// Phase 2 #06.B: lex `expr[:spec]` for `"#{expr:spec}"`. The
    /// expression is delimited by either the closing `}` (no spec) or
    /// by `:` at the OUTERMOST grouping level (spec follows, terminated
    /// by `}`).
    ///
    /// "Outermost" means: brace depth 1 (the interpolation `#{...}`
    /// itself), paren depth 0, bracket depth 0. This is critical —
    /// `:` is also Ruxen syntax for named arguments
    /// (`Shape.Circle(radius: 2.0)`), associated-type qualifiers
    /// (`Self::Item`), and let-binding type ascriptions
    /// (`let x: Int`). Only treat `:` as a spec-start when no nested
    /// grouping is open; otherwise it's part of the expression.
    /// Returns the expression tokens plus the parsed `FormatSpec`
    /// (default if no spec was present).
    pub(super) fn lex_interpolation_expr(
        &mut self,
    ) -> (Vec<Token>, crate::lexer::token::FormatSpec) {
        let mut spec = crate::lexer::token::FormatSpec::default();
        let mut brace_depth = 1u32; // we've already consumed #{
        let mut paren_depth = 0u32;
        let mut bracket_depth = 0u32;

        // Interpolation tokens accumulate in the MAIN `self.tokens` buffer
        // while we lex, and are drained out at the end. This is load-bearing
        // for the context-sensitive `/` (regex-vs-division) and `?`
        // (char-vs-try) decisions: those read `self.tokens.last()`, which must
        // be the PREVIOUS INTERPOLATION token, not the token before the
        // enclosing string. Removing each token immediately (the old code)
        // made `#{w / 2}` after a `(` lex `/` as a regex literal — so
        // `puts("#{w / 2}")` mis-lexed while `puts "#{w / 2}"` did not.
        let interp_start = self.tokens.len();
        // Index up to which we've folded paren/bracket depth (so a later `:`
        // knows whether it's inside a grouping construct).
        let mut scanned = interp_start;

        while !self.is_at_end() && brace_depth > 0 {
            self.skip_whitespace();
            if self.is_at_end() {
                break;
            }

            let ch = self.current();

            // Format-spec start. Only fire at the absolute outermost
            // level — see the doc-comment above for why this matters.
            if ch == ':'
                && brace_depth == 1
                && paren_depth == 0
                && bracket_depth == 0
                && self.peek_at(1) != Some(':')
            // `::` (path qualifier) is not a spec start.
            {
                self.advance(); // consume `:`
                spec = self.lex_format_spec();
                // `lex_format_spec` consumes up to and including the
                // closing `}`; we're done with this interpolation.
                break;
            }

            if ch == '}' {
                brace_depth -= 1;
                if brace_depth == 0 {
                    self.advance();
                    break;
                }
                let sb = self.byte_pos;
                let sl = self.line;
                let sc = self.column;
                self.advance();
                self.tokens.push(Token::new(
                    TokenKind::RBrace,
                    Span::new(sb, self.byte_pos, sl, sc),
                ));
                continue;
            }

            if ch == '{' {
                brace_depth += 1;
                let sb = self.byte_pos;
                let sl = self.line;
                let sc = self.column;
                self.advance();
                self.tokens.push(Token::new(
                    TokenKind::LBrace,
                    Span::new(sb, self.byte_pos, sl, sc),
                ));
                continue;
            }

            // Lex one token; it is emitted into `self.tokens` (kept there so
            // `self.tokens.last()` is the previous interpolation token).
            match ch {
                '\n' => {
                    self.advance();
                    continue;
                }
                '"' => self.lex_string(),
                '\'' => self.lex_single_quote(),
                '0'..='9' => self.lex_number(),
                'a'..='z' | '_' => self.lex_identifier_or_keyword(),
                'A'..='Z' => self.lex_type_identifier_or_keyword(),
                _ => self.lex_operator_or_punct(),
            }

            // Fold paren/bracket depth for any newly emitted tokens WITHOUT
            // removing them (they stay in `self.tokens` for the prev-token
            // context decisions above; drained after the loop).
            while scanned < self.tokens.len() {
                match self.tokens[scanned].kind {
                    TokenKind::LParen => paren_depth += 1,
                    TokenKind::RParen => paren_depth = paren_depth.saturating_sub(1),
                    TokenKind::LBracket => bracket_depth += 1,
                    TokenKind::RBracket => bracket_depth = bracket_depth.saturating_sub(1),
                    _ => {}
                }
                scanned += 1;
            }
        }

        // Drain the interpolation tokens out of the main buffer.
        let tokens = self.tokens.split_off(interp_start);
        (tokens, spec)
    }

    /// Phase 2 #06.B: lex the format-spec characters between `:` and
    /// the closing `}`. Grammar (subset of Rust's):
    ///
    /// ```text
    /// spec := [fill align] [width] ['.' precision] ['?']
    /// fill  := any non-`}` non-`:` non-digit char
    /// align := '<' | '>' | '^'
    /// width := <digit>+
    /// precision := <digit>+
    /// ```
    ///
    /// Consumes up to and including the closing `}`. Phase B3 emits
    /// `E0007` for malformed specs (e.g. `.` without precision digits,
    /// stray non-whitespace characters after the well-formed prefix)
    /// while still recovering by consuming through `}` so downstream
    /// phases can keep going.
    pub(super) fn lex_format_spec(&mut self) -> crate::lexer::token::FormatSpec {
        use crate::lexer::token::FormatSpec;
        let mut spec = FormatSpec::default();

        // Step 1: optional `fill align` prefix. Detect by peeking
        // two chars: if char₂ ∈ {<,>,^} then char₁ is fill.
        if let (Some(_c1), Some(c2)) = (self.peek_at(0), self.peek_at(1)) {
            if matches!(c2, '<' | '>' | '^') {
                spec.fill = self.peek_at(0);
                self.advance(); // consume fill
                spec.align = self.peek_at(0);
                self.advance(); // consume align
            }
        }

        // Step 2: bare `align` (no fill).
        if spec.align.is_none() {
            if let Some(c) = self.peek_at(0) {
                if matches!(c, '<' | '>' | '^') {
                    spec.align = Some(c);
                    self.advance();
                }
            }
        }

        // Step 3: width (digits).
        let mut width_str = String::new();
        while let Some(c) = self.peek_at(0) {
            if c.is_ascii_digit() {
                width_str.push(c);
                self.advance();
            } else {
                break;
            }
        }
        if !width_str.is_empty() {
            spec.width = width_str.parse::<usize>().ok();
        }

        // Step 4: optional `.precision`. `.` without a following digit
        // is malformed (E0007).
        if self.peek_at(0) == Some('.') {
            let dot_byte = self.byte_pos;
            let dot_line = self.line;
            let dot_col = self.column;
            self.advance(); // consume `.`
            let mut prec_str = String::new();
            while let Some(c) = self.peek_at(0) {
                if c.is_ascii_digit() {
                    prec_str.push(c);
                    self.advance();
                } else {
                    break;
                }
            }
            if !prec_str.is_empty() {
                spec.precision = prec_str.parse::<usize>().ok();
            } else {
                let span = Span::new(dot_byte, self.byte_pos, dot_line, dot_col);
                self.diagnostics.push(Diagnostic::error_with_code(
                    "malformed format spec: `.` must be followed by precision digits",
                    span,
                    "E0007",
                ));
            }
        }

        // Step 5: optional `?` debug flag.
        if self.peek_at(0) == Some('?') {
            spec.debug = true;
            self.advance();
        }

        // Step 6: consume up to the closing `}`. Whitespace is
        // tolerated; any other character is malformed (E0007). We
        // still consume them so the outer interpolation loop sees a
        // balanced `}` and downstream phases keep running.
        let mut stray_start: Option<(usize, u32, u32)> = None;
        let mut stray_end: usize = self.byte_pos;
        while let Some(c) = self.peek_at(0) {
            if c == '}' {
                self.advance(); // consume `}`
                break;
            }
            if c.is_whitespace() {
                // Tolerated. Flush any pending stray run first so the
                // diagnostic span doesn't include the whitespace.
                if let Some((sb, sl, sc)) = stray_start.take() {
                    let span = Span::new(sb, stray_end, sl, sc);
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "malformed format spec: unexpected character(s)",
                        span,
                        "E0007",
                    ));
                }
                self.advance();
                continue;
            }
            if stray_start.is_none() {
                stray_start = Some((self.byte_pos, self.line, self.column));
            }
            self.advance();
            stray_end = self.byte_pos;
        }
        if let Some((sb, sl, sc)) = stray_start {
            let span = Span::new(sb, stray_end, sl, sc);
            self.diagnostics.push(Diagnostic::error_with_code(
                "malformed format spec: unexpected character(s)",
                span,
                "E0007",
            ));
        }

        spec
    }
}
