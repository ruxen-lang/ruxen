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
fn test_sample_second_item_is_enum_priority() {
    let source = std::fs::read_to_string("tests/fixtures/sample_program.rvn")
        .expect("failed to read sample program");
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parser failed");

    match &program.items[1] {
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

    // ruby-naming.spec.md: legacy top-level `impl Trait for Type`
    // blocks are folded into the surrounding type body via `include`
    // directives. The sample fixture therefore has no `TopLevelItem::Impl`
    // entries any more; instead, the count moves to per-class `inner_impls`.
    let mut enums = 0;
    let mut classes = 0;
    let mut mixins = 0;
    let mut functions = 0;
    let mut inner_includes = 0;
    for item in &program.items {
        match item {
            TopLevelItem::Enum(e) => {
                enums += 1;
                inner_includes += e.inner_impls.len();
            }
            TopLevelItem::Class(c) => {
                classes += 1;
                inner_includes += c.inner_impls.len();
            }
            TopLevelItem::Mixin(_) => mixins += 1,
            TopLevelItem::Function(_) => functions += 1,
            _ => {}
        }
    }
    assert!(enums >= 3, "expected >= 3 enums, got {}", enums);
    assert!(mixins >= 2, "expected >= 2 mixins, got {}", mixins);
    assert!(classes >= 3, "expected >= 3 classes, got {}", classes);
    assert!(
        inner_includes >= 4,
        "expected >= 4 inline include directives across types, got {}",
        inner_includes
    );
    assert!(functions >= 4, "expected >= 4 functions, got {}", functions);
}
