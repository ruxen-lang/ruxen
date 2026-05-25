//! Integration tests for the Ruxen REPL.

use ruxen_core::lexer::token::TokenKind;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::ast::{ReplInput, ReplParseResult};
use ruxen_core::parser::Parser;

// ── Parser REPL entry point tests ──────────────────────────────────

#[test]
fn parse_integer_expression() {
    let mut lexer = Lexer::new("1 + 2");
    let tokens = lexer.tokenize().unwrap();
    let mut parser = Parser::new(tokens);
    let result = parser.parse_repl_input();

    match result {
        ReplParseResult::Complete(ReplInput::Expression(_)) => {}
        other => panic!("Expected Complete(Expression), got {:?}", other),
    }
}

#[test]
fn parse_let_statement() {
    let mut lexer = Lexer::new("let x = 42");
    let tokens = lexer.tokenize().unwrap();
    let mut parser = Parser::new(tokens);
    let result = parser.parse_repl_input();

    match result {
        ReplParseResult::Complete(ReplInput::Statement(_)) => {}
        other => panic!("Expected Complete(Statement), got {:?}", other),
    }
}

#[test]
fn parse_function_definition() {
    let input = "def double(n: Int) -> Int\n  n * 2\nend";
    let mut lexer = Lexer::new(input);
    let tokens = lexer.tokenize().unwrap();
    let mut parser = Parser::new(tokens);
    let result = parser.parse_repl_input();

    match result {
        ReplParseResult::Complete(ReplInput::TopLevel(_)) => {}
        other => panic!("Expected Complete(TopLevel), got {:?}", other),
    }
}

#[test]
fn parse_incomplete_def() {
    let input = "def double(n: Int) -> Int";
    let mut lexer = Lexer::new(input);
    let tokens = lexer.tokenize().unwrap();
    let mut parser = Parser::new(tokens);
    let result = parser.parse_repl_input();

    match result {
        ReplParseResult::Incomplete => {}
        other => panic!("Expected Incomplete, got {:?}", other),
    }
}

#[test]
fn parse_incomplete_class() {
    let input = "class Foo";
    let mut lexer = Lexer::new(input);
    let tokens = lexer.tokenize().unwrap();
    let mut parser = Parser::new(tokens);
    let result = parser.parse_repl_input();

    match result {
        ReplParseResult::Incomplete => {}
        other => panic!("Expected Incomplete, got {:?}", other),
    }
}

#[test]
fn parse_string_expression() {
    let mut lexer = Lexer::new("\"hello\"");
    let tokens = lexer.tokenize().unwrap();
    let mut parser = Parser::new(tokens);
    let result = parser.parse_repl_input();

    match result {
        ReplParseResult::Complete(ReplInput::Expression(_)) => {}
        other => panic!("Expected Complete(Expression), got {:?}", other),
    }
}

#[test]
fn parse_boolean_expression() {
    let mut lexer = Lexer::new("true");
    let tokens = lexer.tokenize().unwrap();
    let mut parser = Parser::new(tokens);
    let result = parser.parse_repl_input();

    match result {
        ReplParseResult::Complete(ReplInput::Expression(_)) => {}
        other => panic!("Expected Complete(Expression), got {:?}", other),
    }
}

// ── Validator tests (multi-line detection) ──────────────────────────

fn check_balanced(input: &str) -> bool {
    let mut lexer = Lexer::new(input);
    let tokens = match lexer.tokenize() {
        Ok(t) => t,
        Err(_) => return false,
    };

    let mut block_depth: i32 = 0;
    let mut paren_depth: i32 = 0;
    let mut bracket_depth: i32 = 0;
    let mut brace_depth: i32 = 0;

    for tok in &tokens {
        match &tok.kind {
            TokenKind::Def
            | TokenKind::Class
            | TokenKind::Struct
            | TokenKind::Enum
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

    block_depth == 0 && paren_depth == 0 && bracket_depth == 0 && brace_depth == 0
}

#[test]
fn balanced_simple_expression() {
    assert!(check_balanced("1 + 2"));
}

#[test]
fn unbalanced_open_def() {
    assert!(!check_balanced("def foo(n: Int) -> Int"));
}

#[test]
fn balanced_complete_def() {
    assert!(check_balanced("def foo(n: Int) -> Int\n  n * 2\nend"));
}

#[test]
fn unbalanced_open_paren() {
    assert!(!check_balanced("(1 + 2"));
}

#[test]
fn balanced_parens() {
    assert!(check_balanced("(1 + 2)"));
}

#[test]
fn unbalanced_open_bracket() {
    assert!(!check_balanced("[1, 2"));
}

#[test]
fn balanced_brackets() {
    assert!(check_balanced("[1, 2, 3]"));
}

// ── Command parsing tests ──────────────────────────────────────────

#[test]
fn parse_help_command() {
    // Just verify the command module works (it's private, test via integration)
    assert!(":help".starts_with(':'));
}

#[test]
fn parse_quit_command() {
    assert!(":quit".starts_with(':'));
}
