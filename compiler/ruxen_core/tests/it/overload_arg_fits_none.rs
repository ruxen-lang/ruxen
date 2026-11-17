//! A call of an overloaded free function whose argument fits no overload is
//! E0742. The resolver falls back to an overload by arity alone, and the
//! argument was passed anyway: `p(2.5)` handed a double to the `&String`
//! overload of `p` and the binary segfaulted.

use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn coded_errors(name: &str) -> Vec<(Option<String>, String)> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    let source =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
    let tokens = Lexer::new(&source).tokenize().expect("lex");
    let program = Parser::new(tokens).parse().expect("parse");
    typeck::type_check(&program)
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .map(|d| (d.code, d.message))
        .collect()
}

#[test]
fn argument_fitting_no_overload_is_e0742() {
    let errors = coded_errors("overload_arg_fits_none");
    assert_eq!(
        errors,
        vec![(
            Some("E0742".to_string()),
            "no overload of `show` takes a `Float` argument".to_string()
        )],
        "only the Float call is rejected"
    );
}
