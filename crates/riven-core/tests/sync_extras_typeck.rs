//! Pin tests for `docs/specs/stdlib/sync.spec.md` typeck surface.
//!
//! The `std_sync_concurrency_surface_typechecks_cleanly` test covers
//! the umbrella; this file adds **named** pins for individual methods
//! that previously typechecked transitively (`try_lock`,
//! `into_inner`, `deref_mut`, `Arc.strong_count`, `Arc.weak_count`).
//! All assertions are typeck-only — sync.spec.md B4/B5 docs the
//! runtime gap.

use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::typeck;

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

fn assert_clean(source: &str) {
    let errs = typeck_errors(source);
    assert!(
        errs.is_empty(),
        "expected no typeck errors, got: {:?}",
        errs.iter().map(|d| &d.message).collect::<Vec<_>>()
    );
}

/// `Mutex.try_lock() -> Option[MutexGuard[T]]` typechecks.
#[test]
fn mutex_try_lock_returns_option() {
    assert_clean(
        r#"
use std.sync.{Mutex, MutexGuard}

def main
  let m: Mutex[Int] = Mutex.new(0)
  let guard: Option[MutexGuard[Int]] = m.try_lock()
end
"#,
    );
}

/// `Mutex.into_inner() -> Result[T, PoisonError]` typechecks.
#[test]
fn mutex_into_inner_returns_result() {
    assert_clean(
        r#"
use std.sync.{Mutex, PoisonError}

def main
  let m: Mutex[Int] = Mutex.new(42)
  let inner: Result[Int, PoisonError] = m.into_inner()
end
"#,
    );
}

/// `MutexGuard.deref_mut() -> &mut T` typechecks.
#[test]
fn mutex_guard_deref_mut_returns_mut_ref() {
    assert_clean(
        r#"
use std.sync.{Mutex, MutexGuard}

def main
  let m: Mutex[Int] = Mutex.new(0)
  let guard: MutexGuard[Int] = m.lock!()
  let r: &mut Int = guard.deref_mut()
end
"#,
    );
}

/// `Arc.strong_count()` and `weak_count()` return `USize`.
#[test]
fn arc_count_methods_return_usize() {
    assert_clean(
        r#"
use std.sync.Arc

def main
  let a: Arc[Int] = Arc.new(99)
  let s: USize = a.strong_count()
  let w: USize = a.weak_count()
end
"#,
    );
}

/// `Arc.deref()` returns `&T`.
#[test]
fn arc_deref_returns_ref() {
    assert_clean(
        r#"
use std.sync.Arc

def main
  let a: Arc[Int] = Arc.new(7)
  let r: &Int = a.deref()
end
"#,
    );
}
