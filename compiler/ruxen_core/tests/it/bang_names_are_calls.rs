//! B87 — a free function whose name ends in `!` must be a CALL, and a call
//! that cannot be lowered must be a hard diagnostic, never a silent no-op.
//!
//! The defect: `parser/expr/atoms.rs` classified every identifier ending in
//! `!` as a macro call, stripped the `!`, and emitted `ExprKind::MacroCall`.
//! Nothing downstream recognised names like `shout`, and MIR's macro lowering
//! ended in a `_ => Ok(None)` catch-all that emitted no code, so `shout!(5)`
//! compiled clean and the call vanished. `exit!` could not ship because of it.
//!
//! Two invariants are pinned here, both structural — the observable-output
//! side lives in `tests/release-e2e/cases/1880`-`1887`.
//!
//! 1. The parser produces a `MacroCall` only for a name in
//!    `parser::macros::BUILTIN_MACRO_NAMES`; every other `name!` is a call.
//! 2. An unrecognised macro that somehow reaches HIR is rejected with
//!    **E1126** instead of being dropped.

use ruxen_core::lexer::Lexer;
use ruxen_core::parser::ast::{ExprKind, Program, Statement, TopLevelItem};
use ruxen_core::parser::macros::is_builtin_macro;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

fn parse(source: &str) -> Program {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lexer failed");
    let mut parser = Parser::new(tokens);
    parser.parse().expect("parser failed")
}

fn diagnostics(program: &Program) -> Vec<(String, Option<String>)> {
    typeck::type_check(program)
        .diagnostics
        .iter()
        .map(|d| (d.message.clone(), d.code.clone()))
        .collect()
}

/// The single expression statement in `main`'s body.
fn main_first_expr(program: &Program) -> &ExprKind {
    let func = program
        .items
        .iter()
        .find_map(|item| match item {
            TopLevelItem::Function(f) if f.name == "main" => Some(f),
            _ => None,
        })
        .expect("no `main`");
    match func.body.statements.first().expect("empty `main`") {
        Statement::Expression(e) => &e.kind,
        Statement::Let(_) => panic!("expected an expression statement"),
    }
}

#[test]
fn an_undefined_bang_call_is_a_hard_error_not_a_no_op() {
    // The heart of B87: silence was the bug. Whatever the message, the call
    // must not compile.
    let program = parse(
        "\
def main
  nosuch!(5)
end
",
    );
    let diags = diagnostics(&program);
    assert!(
        diags.iter().any(|(m, _)| m.contains("nosuch!")),
        "an undefined `!` call must be reported, got: {diags:#?}"
    );
}

#[test]
fn the_panic_macro_still_parses_as_a_macro() {
    let program = parse(
        "\
def main
  panic!(\"boom\")
end
",
    );
    match main_first_expr(&program) {
        ExprKind::MacroCall { name, args, .. } => {
            assert_eq!(name, "panic", "the `!` is stripped for a real macro");
            assert_eq!(args.len(), 1);
        }
        other => panic!("`panic!` must stay a macro, got {other:?}"),
    }
}

#[test]
fn an_unrecognised_macro_reaching_hir_is_rejected_with_e1126() {
    // No source can produce this any more — the parser only builds a
    // `MacroCall` for a registered name — so the backstop is exercised by
    // rewriting a parsed `panic!` node into an unregistered one. This is the
    // shape that used to lower to nothing at all.
    let mut program = parse(
        "\
def main
  panic!(\"boom\")
end
",
    );
    let func = program
        .items
        .iter_mut()
        .find_map(|item| match item {
            TopLevelItem::Function(f) if f.name == "main" => Some(f),
            _ => None,
        })
        .expect("no `main`");
    match func.body.statements.first_mut().expect("empty `main`") {
        Statement::Expression(e) => match &mut e.kind {
            ExprKind::MacroCall { name, .. } => {
                assert!(!is_builtin_macro("assert"), "`assert` must be unregistered");
                *name = "assert".to_string();
            }
            other => panic!("expected a MacroCall, got {other:?}"),
        },
        Statement::Let(_) => panic!("expected an expression statement"),
    }

    let diags = diagnostics(&program);
    assert!(
        diags
            .iter()
            .any(|(m, code)| code.as_deref() == Some("E1126") && m.contains("assert!")),
        "an unrecognised macro must be E1126, got: {diags:#?}"
    );
}

#[test]
fn every_registered_macro_name_is_bang_free() {
    // Callers strip the `!` before consulting the registry; a stray `!` in the
    // table would silently disable the lookup and re-open B87 for that name.
    for name in ruxen_core::parser::macros::BUILTIN_MACRO_NAMES {
        assert!(
            !name.ends_with('!'),
            "`{name}` must be registered without its `!`"
        );
    }
}

#[test]
fn a_retired_collection_macro_is_rejected_with_e1126() {
    // F-14: `vec!` / `array!` / `map!` / `hash!` / `set!` are retired in
    // favour of the literal forms. They must fail in resolve with a coded
    // diagnostic, not survive to an uncoded MIR error.
    for name in ["vec", "array", "map", "hash", "set"] {
        let program = parse(&format!("def main\n  {name}!(1, 2)\nend\n"));
        let diags = diagnostics(&program);
        assert!(
            diags
                .iter()
                .any(|(m, code)| code.as_deref() == Some("E1126")
                    && m.contains(&format!("`{name}!`"))
                    && m.contains("retired")),
            "`{name}!` must be E1126 with a migration message, got: {diags:#?}"
        );
    }
}
