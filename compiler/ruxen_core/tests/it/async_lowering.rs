//! Pin for the async-lowering await scan: an `.await` inside an
//! enum-variant constructor argument must take the await-aware path.
//!
//! This stays a typecheck pin because the program does not link yet
//! (`Option.Some(f.await)`, see TODO.md Known issues). The other async
//! lowering behaviour is covered by the release-e2e fixtures 720-731, 792,
//! 2052 and 2067.

use ruxen_core::diagnostics::{Diagnostic, DiagnosticLevel};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// Reads a fixture file from `tests/fixtures/ruxen/` per the team's
/// no-inline-rx-source-in-pin-tests rule.
fn rx(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/ruxen")
        .join(format!("{name}.rx"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e))
}

fn typeck_result(source: &str) -> typeck::TypeCheckResult {
    let mut lx = Lexer::new(source);
    let toks = lx.tokenize().expect("lex");
    let mut p = Parser::new(toks);
    let prog = p.parse().expect("parse");
    typeck::type_check(&prog)
}

fn error_messages(diags: &[Diagnostic]) -> Vec<String> {
    diags
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .map(|d| d.message.clone())
        .collect()
}

/// `let x = Option.Some(make_int().await)` — the `.await` lives inside
/// an `ExprKind::EnumVariant` argument. The OLD hand-rolled
/// `expr_contains_await` had a `_ => false` arm and missed `EnumVariant`,
/// so the fn routed to the no-await lowering path and the resolver then
/// misreported E1110 ("`.await` only valid inside async def"). After the
/// Visit migration the await-scan recurses into the variant arg, so the
/// await-aware path is taken and NO E1110 is emitted.
#[test]
fn await_inside_enum_variant_arg_is_detected() {
    let result = typeck_result(&rx("async_await_in_enum_variant"));
    let codes: Vec<String> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .filter_map(|d| d.code.clone())
        .collect();
    assert!(
        !codes.iter().any(|c| c == "E1110"),
        "must NOT misreport E1110 for `.await` inside an enum-variant arg; got codes: {:?} ({:?})",
        codes,
        error_messages(&result.diagnostics)
    );
}
