//! Multi-line input detection via rustyline Validator.
//!
//! Checks delimiter balance (the parser's `has_unclosed_delimiters`, the
//! same check `parse_repl_input` uses) to determine if input is incomplete
//! and needs continuation lines.

use rustyline::validate::{ValidationContext, ValidationResult, Validator};

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;

/// Validates REPL input for completeness (delimiter balance).
pub struct RuxenValidator;

impl Validator for RuxenValidator {
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

        if Parser::has_unclosed_delimiters(&tokens) {
            Ok(ValidationResult::Incomplete)
        } else {
            Ok(ValidationResult::Valid(None))
        }
    }
}
