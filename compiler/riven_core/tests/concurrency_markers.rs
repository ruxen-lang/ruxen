//! Pin tests for `docs/specs/ownership/send_sync_enforcement.spec.md` —
//! POSITIVE side (B1, B2, B10, B12).
//!
//! These fixtures must typecheck cleanly. The matching negative
//! diagnostics live in `concurrency_negative.rs`.
//!
//! Fixtures: `compiler/riven_core/tests/fixtures/riven/concurrency_*.rvn`
//! (per the team rule against inline `r#"..."#` Riven source in `.rs`
//! pin tests).

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

fn rvn(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/riven")
        .join(format!("{name}.rvn"));
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

fn assert_no_concurrency_diagnostic(name: &str) {
    let source = rvn(name);
    let errs = typeck_errors(&source);
    let concurrency: Vec<_> = errs
        .iter()
        .filter(|d| {
            matches!(
                d.code.as_deref(),
                Some("E1100") | Some("E1101") | Some("E1102") | Some("E1011") | Some("E1012")
            )
        })
        .collect();
    assert!(
        concurrency.is_empty(),
        "expected {name} to be clean of Send/Sync diagnostics, got: {:?}",
        concurrency
            .iter()
            .map(|d| (&d.code, &d.message))
            .collect::<Vec<_>>()
    );
}

// ─── B1 — marker mixins resolve ─────────────────────────────────────

#[test]
fn send_sync_marker_mixins_register_and_resolve() {
    // `include Send` / `include Sync` parse + resolve without raising
    // E1014 (the old "manual Send/Sync must be unsafe" rule was
    // promoted to allow plain markers per spec B10).
    let source = rvn("concurrency_send_sync_marker_mixins_resolve");
    let errs = typeck_errors(&source);
    let e1014: Vec<_> = errs
        .iter()
        .filter(|d| d.code.as_deref() == Some("E1014"))
        .collect();
    assert!(
        e1014.is_empty(),
        "expected no E1014 on plain `include Send` / `include Sync`, got: {:?}",
        e1014.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

// ─── B2 — built-in containers auto-derive Send/Sync ─────────────────

#[test]
fn builtin_containers_auto_derive_send_sync_when_t_is() {
    // `Array[Int]` is Send because Int is Send; wrapping in Mutex /
    // SharedSync does not trip E1101 / E1102.
    assert_no_concurrency_diagnostic("concurrency_builtin_container_send_auto_derives");
}

// ─── B10 — user class opt-in via `include Send` ─────────────────────

#[test]
fn user_class_with_include_send_passes_thread_spawn() {
    // A user class with `include Send` typechecks through both
    // Mutex.new (E1101) and Thread.spawn (E1100). Without B10's
    // opt-in path, this fixture would fire both diagnostics.
    assert_no_concurrency_diagnostic("concurrency_user_class_include_send_passes");
}

// ─── B12 — `include unsafe Send` escape hatch ───────────────────────

#[test]
fn unsafe_include_send_overrides_auto_derive() {
    // `include unsafe Send` accepts a class regardless of fields.
    // For v1 (no user-class auto-derive) it behaves identically to
    // plain `include Send`; we pin both forms.
    assert_no_concurrency_diagnostic("concurrency_unsafe_include_send_overrides");
}
