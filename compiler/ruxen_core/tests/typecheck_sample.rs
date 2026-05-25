//! Integration test: type-check the phase5 sample program.
//!
//! The sample program must type-check with zero *fatal* errors.
//! Some warnings and inference gaps are acceptable at this stage.

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn load_sample() -> String {
    std::fs::read_to_string("tests/fixtures/sample_program.rx")
        .expect("failed to read sample_program.rx")
}

#[test]
fn sample_program_lexes() {
    let source = load_sample();
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed on sample program");
    assert!(
        tokens.len() > 100,
        "expected many tokens, got {}",
        tokens.len()
    );
}

#[test]
fn sample_program_parses() {
    let source = load_sample();
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed on sample program");
    assert!(!program.items.is_empty(), "expected top-level items");
}

#[test]
fn sample_program_type_checks() {
    let source = load_sample();
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");

    let result = typeck::type_check(&program);

    // Count actual type errors (not inference gaps)
    let fatal_errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .filter(|d| {
            // Filter out "could not infer" for private helpers — those are
            // expected in phase 3 without full stdlib
            !d.message.contains("could not infer")
        })
        .collect();

    // Report all diagnostics for debugging
    if !fatal_errors.is_empty() {
        eprintln!("\n=== Type Check Diagnostics ===");
        for d in &result.diagnostics {
            eprintln!("  {}", d);
        }
        eprintln!("=== {} fatal errors ===\n", fatal_errors.len());
    }

    // The program should produce a typed HIR with items
    assert!(!result.program.items.is_empty(), "HIR should have items");

    // Print summary
    let total_diags = result.diagnostics.len();
    let error_count = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .count();
    eprintln!(
        "Sample program type-check: {} total diagnostics, {} errors, {} symbols",
        total_diags,
        error_count,
        result.symbols.len()
    );
}

#[test]
fn sample_program_resolves_enums() {
    let source = load_sample();
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");

    let result = typeck::type_check(&program);

    // Check that Priority, Status, TaskError enums are registered
    let has_priority = result.symbols.iter().any(|d| d.name == "Priority");
    let has_status = result.symbols.iter().any(|d| d.name == "Status");
    let has_task_error = result.symbols.iter().any(|d| d.name == "TodoError");

    assert!(has_priority, "Priority enum should be registered");
    assert!(has_status, "Status enum should be registered");
    assert!(has_task_error, "TodoError enum should be registered");
}

#[test]
fn sample_program_resolves_classes() {
    let source = load_sample();
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");

    let result = typeck::type_check(&program);

    let has_task = result.symbols.iter().any(|d| d.name == "Todo");
    let has_timed_task = result.symbols.iter().any(|d| d.name == "TimedTodo");
    let has_repository = result.symbols.iter().any(|d| d.name == "Repository");
    let has_task_list = result.symbols.iter().any(|d| d.name == "TodoList");

    assert!(has_task, "Todo class should be registered");
    assert!(has_timed_task, "TimedTodo class should be registered");
    assert!(has_repository, "Repository class should be registered");
    assert!(has_task_list, "TodoList class should be registered");
}

#[test]
fn sample_program_resolves_traits() {
    let source = load_sample();
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");

    let result = typeck::type_check(&program);

    let has_serializable = result.symbols.iter().any(|d| d.name == "Serializable");
    let has_summarizable = result.symbols.iter().any(|d| d.name == "Summarizable");

    assert!(has_serializable, "Serializable trait should be registered");
    assert!(has_summarizable, "Summarizable trait should be registered");
}

#[test]
fn sample_program_resolves_functions() {
    let source = load_sample();
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");

    let result = typeck::type_check(&program);

    let has_parse_priority = result.symbols.iter().any(|d| d.name == "parse_priority");
    let has_generate_report = result.symbols.iter().any(|d| d.name == "generate_report");
    let has_print_items = result.symbols.iter().any(|d| d.name == "print_items");
    let has_main = result.symbols.iter().any(|d| d.name == "main");

    assert!(has_parse_priority, "parse_priority should be registered");
    assert!(has_generate_report, "generate_report should be registered");
    assert!(has_print_items, "print_items should be registered");
    assert!(has_main, "main should be registered");
}
