//! A bare field name inside a method (`x` instead of `@x`) is E0734
//! "undefined variable" with a help line naming the class that declares the
//! field (Ruby's rule: a bare name is a local variable or a method call,
//! never an instance variable). Before the fix it compiled and read 0,
//! false or a null string (`TODO.md#t-bare-field-read`).

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// Reads a fixture file from `tests/fixtures/ruxen/`.
fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn errors(fixture: &str) -> Vec<Diagnostic> {
    let source = rx(fixture);
    let tokens = Lexer::new(&source).tokenize().expect("lex");
    let program = Parser::new(tokens).parse().expect("parse");
    typeck::type_check(&program)
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

/// The E0734 diagnostics reported for `name`.
fn undefined(fixture: &str, name: &str) -> Vec<Diagnostic> {
    let all = errors(fixture);
    let hits: Vec<Diagnostic> = all
        .iter()
        .filter(|d| {
            d.code.as_deref() == Some("E0734")
                && d.message == format!("undefined variable `{name}`")
        })
        .cloned()
        .collect();
    assert!(
        !hits.is_empty(),
        "{fixture}: expected E0734 for `{name}`, got: {:#?}",
        all.iter()
            .map(|d| (d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
    hits
}

fn instance_help(field: &str, class: &str) -> String {
    format!(
        "`{field}` is a field of `{class}`; read it as `@{field}`. \
         A bare name is a local variable or a method call."
    )
}

#[test]
fn bare_field_in_instance_method_suggests_ivar() {
    let hits = undefined("bare_field_instance", "x");
    assert_eq!(hits[0].help, vec![instance_help("x", "Pt")]);
}

#[test]
fn bare_field_in_blocks_and_lambda_suggests_ivar() {
    let hits = undefined("bare_field_blocks", "x");
    assert_eq!(hits.len(), 3, "one E0734 per block body: {hits:#?}");
    for hit in &hits {
        assert_eq!(hit.help, vec![instance_help("x", "Acc")]);
    }
}

#[test]
fn bare_field_in_generic_class_suggests_ivar() {
    let hits = undefined("bare_field_generic", "v");
    assert_eq!(hits[0].help, vec![instance_help("v", "Box")]);
}

#[test]
fn bare_field_in_class_method_says_there_is_no_instance() {
    let hits = undefined("bare_field_class_method", "x");
    assert_eq!(
        hits[0].help,
        vec![
            "`x` is a field of `Pt`, but a class method has no instance; \
             read it from an instance, such as `pt.x`."
                .to_string()
        ]
    );
}

#[test]
fn bare_inherited_field_names_the_ancestor() {
    let hits = undefined("bare_field_inherited", "x");
    assert_eq!(hits[0].help, vec![instance_help("x", "Base")]);
}

#[test]
fn bare_inherited_field_names_an_ancestor_declared_later() {
    let hits = undefined("bare_field_parent_after", "x");
    assert_eq!(hits[0].help, vec![instance_help("x", "Base")]);
}

#[test]
fn bare_inherited_field_through_a_middle_class_declared_last() {
    let hits = undefined("bare_field_middle_last", "x");
    assert_eq!(hits[0].help, vec![instance_help("x", "Base")]);
}

#[test]
fn bare_field_compound_assignment_is_e0734_not_e1006() {
    let hits = undefined("bare_field_compound", "x");
    assert_eq!(hits[0].help, vec![instance_help("x", "Pt")]);
    assert!(
        !errors("bare_field_compound")
            .iter()
            .any(|d| d.code.as_deref() == Some("E1006")),
        "`x += 1` must not report E1006"
    );
}

#[test]
fn bare_field_in_struct_method_suggests_ivar() {
    let hits = undefined("bare_field_struct", "x");
    assert_eq!(hits[0].help, vec![instance_help("x", "Pair")]);
}

#[test]
fn undefined_name_that_is_not_a_field_has_no_field_help() {
    let hits = undefined("bare_field_typo", "totl");
    assert!(hits[0].help.is_empty(), "no field help: {:?}", hits[0].help);
}
