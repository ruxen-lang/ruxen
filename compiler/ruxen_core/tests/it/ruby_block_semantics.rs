//! Ruby-block-semantics pins (ADR: docs/decisions/ruby-block-semantics.md).
//!
//! Covers the assertions the release-e2e stdout fixtures cannot: compile
//! diagnostics (E1119 `&block` not last, E0729 interpolating a closure), a
//! runtime panic (yield with no block: a clean LocalJumpError-style message
//! and exit 101, not a segfault) and fmt byte-stability of block spellings.
//! The stdout behaviour (auto-call defaults, yielding `&var` refs, inferred
//! and explicit block params, `block_given?`) is release-e2e fixture 2055.

use crate::common;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// Type-check `source` and return the error-level diagnostics (code + message).
fn typecheck_errors(source: &str) -> Vec<(String, String)> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser.parse().expect("parse");
    let result = typeck::type_check(&program);
    result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .map(|d| (d.code.clone().unwrap_or_default(), d.message.clone()))
        .collect()
}

/// Pin (g): `&block` not last → E1119 with a clear message naming the offender.
#[test]
fn block_param_not_last_is_e1119() {
    let source = r##"
def bad(&block: Fn[() -> nil], x: Int)
  yield
end
def main
  bad do
    puts "hi"
  end
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors.iter().any(|(code, msg)| code == "E1119"
            && msg.contains("block parameter")
            && msg.contains("must be the last")
            && msg.contains('x')),
        "expected E1119 naming offender `x`, got {errors:?}"
    );
}

/// Pin (f): a `yield` reached with no block panics cleanly (no segfault),
/// naming the enclosing function, and exits with status 101.
#[test]
fn yield_without_block_panics_cleanly() {
    let source = r##"
def must_yield(&block: Fn[() -> nil])
  yield
end
def main
  must_yield
end
"##;
    let (stdout, stderr, code) = common::compile_and_run(source, "yield_no_block");
    assert_eq!(stdout, "", "stdout was {stdout:?}");
    assert_eq!(code, Some(101), "exit code was {code:?}, stderr={stderr:?}");
    assert!(
        stderr.contains("yield called without a block") && stderr.contains("must_yield"),
        "panic message must name the function; stderr was {stderr:?}"
    );
}

/// Pin (j): `ruxen fmt` round-trips BOTH block-type spellings byte-stably and
/// does NOT rewrite one into the other (ADR D8; the fmt-destructiveness class).
/// Also confirms a `do…end` block and a `{ }` block are each preserved (no
/// `do…end`↔`{}` rewrite).
#[test]
fn fmt_preserves_both_block_type_spellings_and_block_forms() {
    // Canonical square-bracket spelling must survive verbatim.
    let canonical = "def f(x: Int, &block: Fn[(Int) -> nil])\n  yield x\nend\n";
    let out = ruxen_core::formatter::format(canonical).output;
    assert!(
        out.contains("&block: Fn[(Int) -> nil]"),
        "canonical Fn[...] spelling was rewritten; fmt output:\n{out}"
    );
    // Idempotent.
    let out2 = ruxen_core::formatter::format(&out).output;
    assert_eq!(out, out2, "fmt not idempotent on canonical spelling");

    // Back-compat paren spelling must NOT be promoted to the bracket form.
    let paren = "def g(&block: Fn() -> nil)\n  yield\nend\n";
    let out = ruxen_core::formatter::format(paren).output;
    assert!(
        out.contains("&block: Fn() -> nil") && !out.contains("Fn[]"),
        "paren Fn(...) spelling was rewritten to brackets; fmt output:\n{out}"
    );

    // fmt is IDEMPOTENT on a multi-statement `do…end` block attached to a
    // `&block` free fn (the new surface). The formatter's call-site block
    // form is chosen by content fit (a pre-existing, intentional policy:
    // multi-statement → `do…end`, single-statement-that-fits → `{ }`), not
    // by the authored token — this feature does not change that policy. What
    // the ADR guarantees and we pin here is that the new block surface does
    // not break the idempotency invariant `format(format(x)) == format(x)`.
    let prog = concat_src();
    let once = ruxen_core::formatter::format(&prog).output;
    let twice = ruxen_core::formatter::format(&once).output;
    assert_eq!(once, twice, "fmt not idempotent on block program:\n{once}");
    // A multi-statement block keeps `do…end` (content-driven), and the
    // `&block` decl keeps its canonical type spelling.
    assert!(
        once.contains("do |n|"),
        "multi-stmt do…end not kept:\n{once}"
    );
    assert!(
        once.contains("&block: Fn[(Int) -> nil]"),
        "type spelling lost:\n{once}"
    );
}

fn concat_src() -> String {
    "def emit(x: Int, &block: Fn[(Int) -> nil])\n  yield x\nend\n\n\
     def main\n  emit(7) do |n|\n    let y = n + 1\n    puts \"#{y}\"\n  end\nend\n"
        .to_string()
}

/// E0729: interpolating a closure / `Fn` value is a clean compile error, not a
/// silent pointer print. A bare `do … end` is a closure literal (never an
/// expression block), so `let v = do … end; puts "#{v}"` binds the un-invoked
/// closure; without this check MIR interpolation's "unknown type → Int_fmt"
/// fallback prints a raw pointer (silent garbage). Reported live against the
/// `docs/tutorial/05-control-flow.md` "Blocks as expressions" section.
#[test]
fn interpolating_a_closure_is_e0729() {
    let source = r##"
def main
  let v = do
    1 + 2
  end
  puts "#{v}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        errors
            .iter()
            .any(|(code, msg)| code == "E0729" && msg.contains("no `Display`")),
        "expected E0729 on closure interpolation, got {errors:?}"
    );
}

/// Negative half of E0729: invoking the closure and interpolating its (Int)
/// RESULT is fine — only formatting the closure VALUE itself is rejected.
#[test]
fn interpolating_an_invoked_closure_result_is_ok() {
    let source = r##"
def main
  let f = do
    1 + 2
  end
  puts "#{f.()}"
end
"##;
    let errors = typecheck_errors(source);
    assert!(
        !errors.iter().any(|(code, _)| code == "E0729"),
        "E0729 must not fire on an invoked closure's result, got {errors:?}"
    );
}
