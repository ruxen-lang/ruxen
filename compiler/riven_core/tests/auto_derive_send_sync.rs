//! B2 pin tests for `docs/specs/system/zero_rust_stdlib_classes.spec.md`
//! — transitive Send / Sync auto-derive walker.
//!
//! Pre-B2, `hir/types.rs::is_send_with_inner` hardcoded match
//! arms for `Mutex` / `SharedSync` / `Box` / `JoinHandle` / `Sender`
//! / `Receiver` / `AtomicI64` / `AtomicBool` / `AtomicUsize` /
//! `MutexGuard` / `ReadGuard` / `WriteGuard`. Every new generic
//! container had to edit that match. B2 replaces it with a generic
//! walker driven by `include Send` / `include !Send` / `include
//! unsafe Send` directives on the .rvn class declaration; the rule
//! is:
//!
//! 1. `include !Send` → not Send. Escape hatch.
//! 2. `include Send` → Send iff every generic arg is Send AND every
//!    field is Send. Transitive auto-derive.
//! 3. No include directive → not Send (no auto-derive for user
//!    classes).
//!
//! Same shape applies to Sync (`include !Sync` / `include Sync`).
//!
//! These pins verify the generic walker matches the hardcoded match's
//! answers for the canonical stdlib classes — proving B2 didn't
//! regress any pre-B2 behaviour.

use riven_core::diagnostics::DiagnosticLevel;
use riven_core::hir::types::Ty;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

fn typecheck_minimal_user_program() -> typeck::TypeCheckResult {
    // Bare main — the user program contributes nothing; we just need
    // the stdlib classes registered through bootstrap so the symbol
    // table has Mutex / SharedSync / MutexGuard / atomics / channels.
    let src = "def main\nend\n";
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "setup typeck errors: {:?}", errors);
    result
}

#[test]
fn transitive_send_iff_all_generic_params_send() {
    let result = typecheck_minimal_user_program();

    // Mutex[Int]: Send (Int satisfies Send, Mutex declares `include Send`).
    let mutex_int = Ty::Class {
        name: "Mutex".to_string(),
        generic_args: vec![Ty::Int],
    };
    assert!(
        mutex_int.is_send_with(&result.symbols),
        "Mutex[Int] must be Send"
    );

    // SharedSync[Int]: Send.
    let arc_int = Ty::Class {
        name: "SharedSync".to_string(),
        generic_args: vec![Ty::Int],
    };
    assert!(
        arc_int.is_send_with(&result.symbols),
        "SharedSync[Int] must be Send"
    );

    // JoinHandle[Int]: Send.
    let jh_int = Ty::Class {
        name: "JoinHandle".to_string(),
        generic_args: vec![Ty::Int],
    };
    assert!(
        jh_int.is_send_with(&result.symbols),
        "JoinHandle[Int] must be Send"
    );

    // Sender[Int] / Receiver[Int]: Send.
    let sender_int = Ty::Class {
        name: "Sender".to_string(),
        generic_args: vec![Ty::Int],
    };
    assert!(
        sender_int.is_send_with(&result.symbols),
        "Sender[Int] must be Send"
    );
    let receiver_int = Ty::Class {
        name: "Receiver".to_string(),
        generic_args: vec![Ty::Int],
    };
    assert!(
        receiver_int.is_send_with(&result.symbols),
        "Receiver[Int] must be Send"
    );

    // Atomics (non-generic): Send.
    for atomic in &["AtomicI64", "AtomicBool", "AtomicUsize"] {
        let atom = Ty::Class {
            name: atomic.to_string(),
            generic_args: vec![],
        };
        assert!(
            atom.is_send_with(&result.symbols),
            "{} must be Send",
            atomic
        );
    }
}

#[test]
fn transitive_sync_iff_all_generic_params_sync() {
    let result = typecheck_minimal_user_program();

    let mutex_int = Ty::Class {
        name: "Mutex".to_string(),
        generic_args: vec![Ty::Int],
    };
    assert!(
        mutex_int.is_sync_with(&result.symbols),
        "Mutex[Int] must be Sync"
    );
    let arc_int = Ty::Class {
        name: "SharedSync".to_string(),
        generic_args: vec![Ty::Int],
    };
    assert!(
        arc_int.is_sync_with(&result.symbols),
        "SharedSync[Int] must be Sync"
    );
    for atomic in &["AtomicI64", "AtomicBool", "AtomicUsize"] {
        let atom = Ty::Class {
            name: atomic.to_string(),
            generic_args: vec![],
        };
        assert!(
            atom.is_sync_with(&result.symbols),
            "{} must be Sync",
            atomic
        );
    }
}

#[test]
fn include_negative_send_overrides_transitive() {
    // MutexGuard declares `include !Send` → never Send, even with
    // `T = Int` which IS Send.
    let result = typecheck_minimal_user_program();
    let guard_int = Ty::Class {
        name: "MutexGuard".to_string(),
        generic_args: vec![Ty::Int],
    };
    assert!(
        !guard_int.is_send_with(&result.symbols),
        "MutexGuard[Int] must NOT be Send (`include !Send` carve-out)"
    );
}

#[test]
fn user_class_without_include_send_is_not_send() {
    // A user class with no `include Send` is NOT Send even if all its
    // fields would satisfy Send. This is the strict mode (different
    // from structs/enums, which DO auto-derive via field walk).
    use riven_core::parser::ast::Program;

    let src = "\
class Foo\n  x: Int\n  def init(v: Int)\n    self.x = v\n  end\nend\n\ndef main\n  let _ = Foo.new(7)\nend\n";
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program: Program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "setup typeck errors: {:?}", errors);

    let foo = Ty::Class {
        name: "Foo".to_string(),
        generic_args: vec![],
    };
    assert!(
        !foo.is_send_with(&result.symbols),
        "Foo without `include Send` must NOT be Send under strict-mode rules"
    );
}
