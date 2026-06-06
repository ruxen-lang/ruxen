//! Pin tests for Feature B — general `[T: Bound]` / `where T: Bound`
//! enforcement on call sites (`docs/specs/system/generic-compiler.spec.md`,
//! Task B). The red/green pin the plan asks for: a `def needs[T: Greet]`
//! called with a non-satisfying type → a clean E-coded error (E1015); with
//! a satisfying type → clean.
//!
//! Diagnostics are asserted by code (matching the convention in
//! `tests/concurrency_negative.rs`). Fixtures live in
//! `compiler/ruxen_core/tests/fixtures/ruxen/trait_bound_*.rx` (per the
//! team rule against inline `r#"..."#` Ruxen source in `.rs` pin tests).

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn typeck_errors(source: &str) -> Vec<Diagnostic> {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    let result = typeck::type_check(&prog);
    result
        .diagnostics
        .into_iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect()
}

fn count_with_code(errs: &[Diagnostic], code: &str) -> usize {
    errs.iter()
        .filter(|d| d.code.as_deref() == Some(code))
        .count()
}

// ─── RED: unsatisfied bound on a harvested generic param → E1015 ────

#[test]
fn generic_param_bound_unsatisfied_emits_e1015() {
    let errs = typeck_errors(&rx("trait_bound_generic_arg_unsatisfied_e1015"));
    assert!(
        count_with_code(&errs, "E1015") >= 1,
        "expected E1015 for needs[T: Greet](&Plain) where Plain does not include Greet, got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

// ─── GREEN: satisfied bound typechecks with no bound diagnostic ──────

#[test]
fn generic_param_bound_satisfied_is_clean() {
    let errs = typeck_errors(&rx("trait_bound_generic_arg_satisfied_clean"));
    assert_eq!(
        count_with_code(&errs, "E1015"),
        0,
        "needs[T: Greet](&Greeter) where Greeter includes Greet must NOT \
         emit a bound diagnostic, got: {:?}",
        errs.iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}
