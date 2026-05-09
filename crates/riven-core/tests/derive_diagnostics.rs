use riven_core::diagnostics::Diagnostic;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

fn parse_errors(path: &str) -> Vec<Diagnostic> {
    let source = std::fs::read_to_string(path).expect("fixture should exist");
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer should succeed");
    let mut parser = Parser::new(tokens);
    parser
        .parse()
        .expect_err("fixture should fail during parse")
}

fn typecheck_diagnostics(path: &str) -> Vec<Diagnostic> {
    let source = std::fs::read_to_string(path).expect("fixture should exist");
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize().expect("lexer should succeed");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("fixture should parse");
    typeck::type_check(&program).diagnostics
}

fn has_code(diags: &[Diagnostic], code: &str) -> bool {
    diags.iter().any(|diag| diag.code.as_deref() == Some(code))
}

#[test]
fn derive_copy_clone_fixture_typechecks_cleanly() {
    let diags = typecheck_diagnostics("tests/fixtures/derive/derive_copy_clone_ok.rvn");
    assert!(
        !diags
            .iter()
            .any(|diag| diag.level == riven_core::diagnostics::DiagnosticLevel::Error),
        "unexpected errors: {:?}",
        diags
    );
}

#[test]
fn derive_invalid_target_reports_e0607() {
    let diags = parse_errors("tests/fixtures/derive/e0607_derive_on_fn.rvn");
    assert!(has_code(&diags, "E0607"), "diagnostics: {:?}", diags);
}

#[test]
fn derive_validation_reports_expected_codes() {
    let cases = [
        (
            "tests/fixtures/derive/e0601_copy_non_copy_field.rvn",
            "E0601",
        ),
        (
            "tests/fixtures/derive/e0602_copy_without_clone.rvn",
            "E0602",
        ),
        ("tests/fixtures/derive/e0603_copy_on_class.rvn", "E0603"),
        (
            "tests/fixtures/derive/e0604_eq_without_partialeq.rvn",
            "E0604",
        ),
        (
            "tests/fixtures/derive/e0605_default_enum_without_default.rvn",
            "E0605",
        ),
        (
            "tests/fixtures/derive/e0606_ord_without_eq_partialord.rvn",
            "E0606",
        ),
        ("tests/fixtures/derive/e0608_unknown_derive.rvn", "E0608"),
        ("tests/fixtures/derive/e0609_duplicate_derive.rvn", "E0609"),
    ];

    for (path, code) in cases {
        let diags = typecheck_diagnostics(path);
        assert!(has_code(&diags, code), "{} missing in {:?}", code, diags);
    }
}
