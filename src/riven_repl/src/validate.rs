//! Multi-line input detection via rustyline Validator.
//!
//! Checks delimiter balance to determine if input is incomplete
//! and needs continuation lines.

use rustyline::validate::{ValidationContext, ValidationResult, Validator};

use riven_core::lexer::token::TokenKind;
use riven_core::lexer::Lexer;

/// Validates REPL input for completeness (delimiter balance).
pub struct RivenValidator;

impl Validator for RivenValidator {
    fn validate(&self, ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        let input = ctx.input();
        if input.trim().is_empty() {
            return Ok(ValidationResult::Valid(None));
        }

        // Commands are always complete
        if input.trim_start().starts_with(':') {
            return Ok(ValidationResult::Valid(None));
        }

        // Lex the input and check delimiter balance
        let mut lexer = Lexer::new(input);
        let tokens = match lexer.tokenize() {
            Ok(tokens) => tokens,
            Err(_) => {
                // If lexing fails, it might be due to unclosed string
                // Check for unclosed string literal
                if input.chars().filter(|&c| c == '"').count() % 2 != 0 {
                    return Ok(ValidationResult::Incomplete);
                }
                return Ok(ValidationResult::Valid(None));
            }
        };

        let mut block_depth: i32 = 0;
        let mut paren_depth: i32 = 0;
        let mut bracket_depth: i32 = 0;
        let mut brace_depth: i32 = 0;

        for tok in &tokens {
            match &tok.kind {
                // Block openers (matched by `end`)
                TokenKind::Def
                | TokenKind::Class
                | TokenKind::Struct
                | TokenKind::Enum
                // ruby-naming.spec.md §3.4 / §3.4a:
                | TokenKind::Mixin
                | TokenKind::Extension
                | TokenKind::Module
                | TokenKind::If
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Loop
                | TokenKind::Match => block_depth += 1,
                TokenKind::Do => block_depth += 1,
                TokenKind::End => block_depth -= 1,
                TokenKind::LParen => paren_depth += 1,
                TokenKind::RParen => paren_depth -= 1,
                TokenKind::LBracket => bracket_depth += 1,
                TokenKind::RBracket => bracket_depth -= 1,
                TokenKind::LBrace => brace_depth += 1,
                TokenKind::RBrace => brace_depth -= 1,
                TokenKind::Eof => break,
                _ => {}
            }
        }

        if block_depth > 0 || paren_depth > 0 || bracket_depth > 0 || brace_depth > 0 {
            Ok(ValidationResult::Incomplete)
        } else {
            Ok(ValidationResult::Valid(None))
        }
    }
}
