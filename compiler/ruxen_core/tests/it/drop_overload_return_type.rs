//! An overloaded `drop` is renamed `drop__overload<N>` by the resolver. The
//! "public function must have an explicit return type" check defaults void
//! methods (`drop`/`display`/`init`/…) to Unit — but it matched the bare name,
//! so an overload-renamed `drop` variant spuriously errored. Guard the fix.

use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn errors(source: &str) -> Vec<String> {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    typeck::type_check(&prog)
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .map(|d| d.message)
        .collect()
}

#[test]
fn overloaded_drop_does_not_require_explicit_return_type() {
    let errs = errors(&rx("drop_overload_return"));
    let offending: Vec<&String> = errs
        .iter()
        .filter(|m| m.contains("must have an explicit return type"))
        .collect();
    assert!(
        offending.is_empty(),
        "overload-renamed drop must default to Unit, not require an explicit return type; got: {offending:?}"
    );
}
