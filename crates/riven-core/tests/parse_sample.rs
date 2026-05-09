use riven_core::lexer::Lexer;
use riven_core::parser::ast::*;
use riven_core::parser::Parser;

#[test]
fn test_sample_program_parses_without_errors() {
    let source = std::fs::read_to_string("tests/fixtures/sample_program.rvn")
        .expect("failed to read sample program");
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed on sample program");
    assert!(
        program.items.len() > 10,
        "expected many top-level items, got {}",
        program.items.len()
    );
}

#[test]
fn test_sample_first_item_is_enum_priority() {
    let source = std::fs::read_to_string("tests/fixtures/sample_program.rvn")
        .expect("failed to read sample program");
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");

    match &program.items[0] {
        TopLevelItem::Enum(e) => {
            assert_eq!(e.name, "Priority");
            assert_eq!(e.variants.len(), 4);
        }
        other => panic!(
            "expected enum Priority, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn test_sample_contains_expected_items() {
    let source = std::fs::read_to_string("tests/fixtures/sample_program.rvn")
        .expect("failed to read sample program");
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");

    let mut enums = 0;
    let mut classes = 0;
    let mut impls = 0;
    let mut traits = 0;
    let mut functions = 0;
    for item in &program.items {
        match item {
            TopLevelItem::Enum(_) => enums += 1,
            TopLevelItem::Class(_) => classes += 1,
            TopLevelItem::Impl(_) => impls += 1,
            TopLevelItem::Trait(_) => traits += 1,
            TopLevelItem::Function(_) => functions += 1,
            _ => {}
        }
    }
    assert!(enums >= 3, "expected >= 3 enums, got {}", enums);
    assert!(traits >= 2, "expected >= 2 traits, got {}", traits);
    assert!(classes >= 3, "expected >= 3 classes, got {}", classes);
    assert!(impls >= 4, "expected >= 4 impl blocks, got {}", impls);
    assert!(functions >= 4, "expected >= 4 functions, got {}", functions);
}
