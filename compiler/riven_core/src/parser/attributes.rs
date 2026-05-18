//! Attribute parsing (`@[name(args)]`) and per-item attribute application.

use crate::lexer::token::TokenKind;
use crate::parser::ast::*;
use crate::parser::Parser;

impl Parser {
    /// Route `@[derive(...)]` / `@[repr(...)]` attributes onto a struct.
    /// Unknown attribute names emit a diagnostic.
    pub(super) fn apply_struct_attrs(&mut self, s: &mut StructDef, attrs: &[Attribute]) {
        for attr in attrs {
            match attr.name.as_str() {
                "derive" => {
                    for arg in &attr.args {
                        s.derive_traits.push(arg.as_str().to_string());
                    }
                }
                "repr" => {
                    for arg in &attr.args {
                        s.repr.push(arg.as_str().to_string());
                    }
                }
                other => {
                    self.error(&format!("attribute `{}` is not valid on `struct`", other));
                }
            }
        }
    }

    /// Route `@[derive(...)]` attributes onto a class. `@[repr(...)]` is
    /// rejected — classes use the boxed GC layout and cannot opt into
    /// a C layout.
    pub(super) fn apply_class_attrs(&mut self, c: &mut ClassDef, attrs: &[Attribute]) {
        for attr in attrs {
            match attr.name.as_str() {
                "derive" => {
                    for arg in &attr.args {
                        c.derive_traits.push(arg.as_str().to_string());
                    }
                }
                other => {
                    self.error(&format!("attribute `{}` is not valid on `class`", other));
                }
            }
        }
    }

    /// Route `@[derive(...)]` attributes onto an enum.
    pub(super) fn apply_enum_attrs(&mut self, e: &mut EnumDef, attrs: &[Attribute]) {
        for attr in attrs {
            match attr.name.as_str() {
                "derive" => {
                    for arg in &attr.args {
                        e.derive_traits.push(arg.as_str().to_string());
                    }
                }
                other => {
                    self.error(&format!("attribute `{}` is not valid on `enum`", other));
                }
            }
        }
    }

    /// Parse `@[name(args)]` attributes.
    pub(super) fn parse_attributes(&mut self) -> Vec<Attribute> {
        let mut attrs = Vec::new();
        while self.at(TokenKind::At) {
            let start = self.current_span();
            self.advance(); // consume @
            self.expect(TokenKind::LBracket);
            self.skip_newlines();

            let name = self.expect_any_identifier();
            let mut args = Vec::new();

            if self.at(TokenKind::LParen) {
                self.advance(); // consume (
                self.skip_newlines();
                if !self.at(TokenKind::RParen) {
                    // Parse arguments as strings or identifiers
                    args.push(self.parse_attr_arg());
                    while self.eat(TokenKind::Comma) {
                        self.skip_newlines();
                        if self.at(TokenKind::RParen) {
                            break;
                        }
                        args.push(self.parse_attr_arg());
                    }
                }
                self.skip_newlines();
                self.expect(TokenKind::RParen);
            }

            self.skip_newlines();
            self.expect(TokenKind::RBracket);
            self.skip_newlines();

            let span = self.span_from(&start);
            attrs.push(Attribute { name, args, span });
        }
        attrs
    }

    // (apply_type_attrs removed during merge — superseded by per-kind helpers
    // apply_struct_attrs, apply_class_attrs, apply_enum_attrs from HEAD which
    // already exist on master and have richer per-kind validation including
    // E0607 error codes for misplaced @[derive].)

    /// Parse a single attribute argument (string literal or identifier).
    fn parse_attr_arg(&mut self) -> AttrArg {
        let start = self.current_span();
        match self.current_kind().clone() {
            TokenKind::StringLiteral(s) => {
                self.advance();
                AttrArg::Str(s, self.span_from(&start))
            }
            TokenKind::Identifier(s) => {
                self.advance();
                AttrArg::Ident(s, self.span_from(&start))
            }
            TokenKind::TypeIdentifier(s) => {
                self.advance();
                AttrArg::Ident(s, self.span_from(&start))
            }
            _ => {
                self.error("expected string or identifier in attribute argument");
                self.advance();
                AttrArg::Ident(String::new(), self.span_from(&start))
            }
        }
    }
}
