//! FFI declarations: `lib "name" ... end` blocks and their `def` entries.

use crate::lexer::token::TokenKind;
use crate::parser::ast::*;
use crate::parser::Parser;

impl Parser {
    /// Parse `lib Name ... end`
    pub(super) fn parse_lib_decl(&mut self, link_attrs: Vec<LinkAttr>) -> LibDecl {
        let start = self.current_span();
        self.advance(); // consume `lib`
        self.skip_newlines();

        // ruby-naming.spec.md §3.7: `lib "name" ... end` takes a string
        // literal link name. The legacy TypeIdentifier form
        // (`lib LibM ... end`) is retained for transitional sources.
        let name = match self.current_kind().clone() {
            TokenKind::StringLiteral(s) => {
                self.advance();
                s
            }
            TokenKind::TypeIdentifier(n) => {
                self.advance();
                n
            }
            _ => {
                self.error("expected lib name — a string literal (`lib \"c\"`) or TypeIdentifier");
                "_Error".to_string()
            }
        };

        // ruby-naming.spec.md §3.7: `lib "x", version: "3", path: "..."`
        // — options follow as keyword arguments. Accept and discard for
        // now; the typeck/codegen plumbing for these options lands in a
        // follow-up.
        while self.eat(TokenKind::Comma) {
            self.skip_newlines();
            if matches!(self.current_kind(), TokenKind::Identifier(_)) {
                self.advance(); // option name
                if self.eat(TokenKind::Colon) {
                    // option value — accept any literal
                    if matches!(
                        self.current_kind(),
                        TokenKind::StringLiteral(_)
                            | TokenKind::IntLiteral(_, _)
                            | TokenKind::True
                            | TokenKind::False
                    ) {
                        self.advance();
                    }
                }
            } else {
                break;
            }
        }

        self.skip_newlines();
        let mut functions = Vec::new();

        while !self.at(TokenKind::End) && !self.at(TokenKind::Eof) {
            let __progress = self.pos;
            self.skip_newlines();
            // Doc comments (`##`) may precede each FFI `def`, exactly as in
            // class/mixin bodies and at top level — stdlib packages such as
            // `std.regex` document every FFI decl. Consume them here so a
            // documented `lib` block parses. The formatter re-derives doc
            // comments from source text (its own CommentMap), so they are
            // preserved across `ruxen fmt` without living on the AST node.
            let _docs = self.collect_doc_comments();
            self.skip_newlines();
            if self.at(TokenKind::End) || self.at(TokenKind::Eof) {
                break;
            }
            if self.at(TokenKind::Def) {
                functions.push(self.parse_ffi_function());
            } else {
                self.error(&format!(
                    "expected `def` in lib block, found {:?}",
                    self.current_kind()
                ));
                self.advance();
            }
            self.expect_terminator();
            self.ensure_loop_progress(__progress);
        }

        self.expect(TokenKind::End);
        let span = self.span_from(&start);

        LibDecl {
            name,
            functions,
            link_attrs,
            span,
        }
    }

    /// Parse a single FFI function declaration: `def name(params) -> RetType`,
    /// optionally with an explicit C-symbol alias: `def name as "<sym>"(params) -> RetType`.
    ///
    /// When the `as "..."` clause is present, the Ruxen-side name (`name`)
    /// is the user-facing identifier — what call sites and `use` statements
    /// reference — while the string literal is the verbatim C symbol that
    /// the linker resolves against `library/std/<pkg>/runtime/*.c`. This is the
    /// per-decl rename surface that stdlib self-hosting (#06.8) needs so
    /// that a Ruxen method like `File.open` can bind to `ruxen_file_open`
    /// without forcing the Ruxen identifier to be `ruxen_file_open` too.
    fn parse_ffi_function(&mut self) -> FfiFunction {
        let start = self.current_span();
        self.advance(); // consume `def`
        self.skip_newlines();

        // Ruxen convention (ruby-naming.spec.md §3.4a): `def NAME(...)` is
        // an instance method, `def self.NAME(...)` is a class method. The
        // same convention applies inside FFI lib blocks — a `def self.foo`
        // FFI decl is a class method bound to a C symbol; `def foo` is an
        // instance method that passes its receiver as the first arg to
        // the C symbol. The flag lives on the AST so the resolver can
        // propagate it to FnSignature.is_class_method.
        let is_class_method = if matches!(self.current_kind(), TokenKind::SelfValue)
            && matches!(self.peek_kind(), TokenKind::Dot)
        {
            self.advance(); // consume `self`
            self.advance(); // consume `.`
            true
        } else {
            false
        };

        let name = self.expect_any_identifier();

        // Optional `as "<c-symbol>"` rename clause. The C symbol is taken
        // verbatim — no mangling, no namespacing — same contract as
        // Rust's `#[link_name = "..."]`. If the link symbol does not
        // exist at link time, the linker fails (the correct failure
        // mode for an explicit FFI binding).
        let c_symbol = if self.eat(TokenKind::As) {
            self.skip_newlines();
            match self.current_kind().clone() {
                TokenKind::StringLiteral(s) => {
                    self.advance();
                    Some(s)
                }
                _ => {
                    self.error(
                        "expected a string literal after `as` in FFI def — \
                         `def name as \"ruxen_c_symbol\"(...)`",
                    );
                    None
                }
            }
        } else {
            None
        };

        let mut params = Vec::new();
        let mut is_variadic = false;

        if self.at(TokenKind::LParen) {
            self.advance(); // consume (
            self.skip_newlines();

            while !self.at(TokenKind::RParen) && !self.at(TokenKind::Eof) {
                // Check for variadic `...`
                if self.at(TokenKind::DotDot) {
                    self.advance(); // consume ..
                    if self.at(TokenKind::Dot) {
                        self.advance(); // consume the third .
                    }
                    is_variadic = true;
                    self.skip_newlines();
                    break;
                }

                // `self` as the first param of an FFI def explicitly
                // marks an instance method (ruby-naming.spec.md §3.4a):
                // `def name(self, x: Int)`. The token is purely a sugar
                // marker — the receiver type is determined by the
                // enclosing class/mixin, and the resolver prepends it
                // to the FfiFuncDecl's `param_types` so the C symbol's
                // declared cranelift signature matches the call shape
                // (which prepends `self` to arg_values for any non-
                // static method). We consume the token here without
                // pushing a synthetic param so the parser doesn't have
                // to invent a `Self` type that resolve cannot bind.
                if params.is_empty() && self.at(TokenKind::SelfValue) {
                    self.advance(); // consume `self`
                    if !self.eat(TokenKind::Comma) {
                        break;
                    }
                    self.skip_newlines();
                    continue;
                }

                let param_start = self.current_span();
                let param_name = self.expect_any_identifier();
                self.expect(TokenKind::Colon);
                self.skip_newlines();
                let param_type = self.parse_type();
                let param_span = self.span_from(&param_start);

                params.push(FfiParam {
                    name: param_name,
                    type_expr: param_type,
                    span: param_span,
                });

                if !self.eat(TokenKind::Comma) {
                    break;
                }
                self.skip_newlines();
            }

            self.skip_newlines();
            self.expect(TokenKind::RParen);
        }

        let return_type = if self.eat(TokenKind::Arrow) {
            self.skip_newlines();
            Some(self.parse_type())
        } else {
            None
        };

        let span = self.span_from(&start);
        FfiFunction {
            name,
            is_class_method,
            c_symbol,
            params,
            return_type,
            is_variadic,
            span,
        }
    }
}
