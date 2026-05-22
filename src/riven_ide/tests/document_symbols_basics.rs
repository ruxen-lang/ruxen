//! `textDocument/documentSymbol` — outline tests.
//!
//! Per feedback_no_inline_rvn_in_pin_tests.md, every Riven source
//! lives in a `.rvn` fixture file under
//! `src/riven_ide/tests/fixtures/document_symbols/`.

use lsp_types::SymbolKind;
use riven_ide::analysis::analyze;
use riven_ide::document_symbols::document_symbols;

fn load(stem: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/document_symbols")
        .join(format!("{}.rvn", stem));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

#[test]
fn class_surfaces_as_parent_with_methods_and_fields() {
    let src = load("class_with_methods_and_fields");
    let result = analyze(&src);
    let symbols = document_symbols(&result);

    let counter = symbols
        .iter()
        .find(|s| s.name == "Counter")
        .expect("Counter class missing from outline");
    assert_eq!(counter.kind, SymbolKind::CLASS);

    let children = counter
        .children
        .as_ref()
        .expect("Counter should have children");
    let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"value"), "fields: {:?}", names);
    assert!(names.contains(&"step"), "fields: {:?}", names);
    assert!(names.contains(&"increment"), "methods: {:?}", names);
    assert!(names.contains(&"get"), "methods: {:?}", names);

    let value = children.iter().find(|c| c.name == "value").unwrap();
    assert_eq!(value.kind, SymbolKind::FIELD);
    let inc = children.iter().find(|c| c.name == "increment").unwrap();
    assert_eq!(inc.kind, SymbolKind::METHOD);
}

#[test]
fn single_function_surfaces_as_function_leaf() {
    let src = load("single_function");
    let result = analyze(&src);
    let symbols = document_symbols(&result);

    let greet = symbols
        .iter()
        .find(|s| s.name == "greet")
        .expect("greet missing");
    assert_eq!(greet.kind, SymbolKind::FUNCTION);
    // A leaf — no children
    assert!(greet.children.is_none() || greet.children.as_ref().unwrap().is_empty());
}

#[test]
fn enum_surfaces_with_variants_as_enum_members() {
    let src = load("enum_with_variants");
    let result = analyze(&src);
    let symbols = document_symbols(&result);

    let color = symbols
        .iter()
        .find(|s| s.name == "Color")
        .expect("Color enum missing");
    assert_eq!(color.kind, SymbolKind::ENUM);

    let children = color
        .children
        .as_ref()
        .expect("Color should have variants as children");
    let names: Vec<&str> = children.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"Red"), "variants: {:?}", names);
    assert!(names.contains(&"Green"), "variants: {:?}", names);
    assert!(names.contains(&"Blue"), "variants: {:?}", names);
    for c in children {
        assert_eq!(
            c.kind,
            SymbolKind::ENUM_MEMBER,
            "{} should be ENUM_MEMBER",
            c.name
        );
    }
}

#[test]
fn mixed_top_level_assigns_correct_kinds() {
    let src = load("mixed_top_level");
    let result = analyze(&src);
    let symbols = document_symbols(&result);

    let by_name = |n: &str| symbols.iter().find(|s| s.name == n);

    let max_size = by_name("MAX_SIZE").expect("MAX_SIZE const missing");
    assert_eq!(max_size.kind, SymbolKind::CONSTANT);

    let point = by_name("Point").expect("Point class missing");
    assert_eq!(point.kind, SymbolKind::CLASS);

    let shape = by_name("Shape").expect("Shape enum missing");
    assert_eq!(shape.kind, SymbolKind::ENUM);

    let make_origin = by_name("make_origin").expect("make_origin missing");
    assert_eq!(make_origin.kind, SymbolKind::FUNCTION);
}

#[test]
fn empty_source_yields_no_symbols() {
    let result = analyze("");
    let symbols = document_symbols(&result);
    assert!(symbols.is_empty(), "expected empty, got {:?}", symbols);
}

#[test]
fn parse_error_yields_no_symbols() {
    // `def\nend\n` — malformed function, parse error → no program
    let result = analyze("def\nend\n");
    let symbols = document_symbols(&result);
    assert!(
        symbols.is_empty(),
        "parse-error source must produce no symbols, got {:?}",
        symbols
    );
}

#[test]
fn class_symbol_range_covers_class_body() {
    let src = load("class_with_methods_and_fields");
    let result = analyze(&src);
    let symbols = document_symbols(&result);
    let counter = symbols.iter().find(|s| s.name == "Counter").unwrap();
    // The class spans multiple lines — start.line must be < end.line.
    assert!(
        counter.range.start.line < counter.range.end.line,
        "Counter range should span multiple lines: {:?}",
        counter.range
    );
}
