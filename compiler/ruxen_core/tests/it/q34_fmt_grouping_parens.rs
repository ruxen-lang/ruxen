//! Q34 — `ruxen fmt` dropped grouping parentheses, silently changing
//! arithmetic (`docs/dev/gui-stack-v1-issues.md` §Q34). The parser keeps no
//! `Paren` node, so the formatter must RE-DERIVE grouping from operator
//! precedence: any operand whose precedence is looser than its position
//! requires is re-parenthesised. The law is reparse-identity — the formatted
//! output must parse to the SAME tree (the printer's fully-parenthesised
//! canonical form) as the source.

use ruxen_core::formatter;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::printer::PrettyPrinter;
use ruxen_core::parser::Parser;

/// Span-blind canonical dump of a program: the AST printer fully
/// parenthesises every operator, so two trees are structurally equal iff
/// their dumps match.
fn canonical(src: &str) -> String {
    let toks = Lexer::new(src)
        .tokenize()
        .unwrap_or_else(|e| panic!("lex {src:?}: {e:?}"));
    let prog = Parser::new(toks)
        .parse()
        .unwrap_or_else(|e| panic!("parse {src:?}: {e:?}"));
    PrettyPrinter::new().print_program(&prog)
}

/// fmt(src) must re-parse to the same tree AND be idempotent.
fn assert_reparse_identical(src: &str) {
    let r = formatter::format(src);
    assert!(r.errors.is_empty(), "fmt {src:?} errored: {:?}", r.errors);
    assert_eq!(
        canonical(src),
        canonical(&r.output),
        "fmt changed the parse tree\n--- src ---\n{src}\n--- fmt ---\n{}\n--- src tree ---\n{}\n--- fmt tree ---\n{}",
        r.output,
        canonical(src),
        canonical(&r.output),
    );
    let r2 = formatter::format(&r.output);
    assert_eq!(r.output, r2.output, "fmt not idempotent on {src:?}");
}

#[test]
fn the_original_q34_repro() {
    // The exact quiver slider-math expression that silently broke.
    let src = "def main\n  let v = (rel * span + track_w / 2) / track_w\nend\n";
    let r = formatter::format(src);
    assert!(
        r.output.contains("(rel * span + track_w / 2) / track_w"),
        "grouping parens dropped; output:\n{}",
        r.output
    );
    assert_reparse_identical(src);
}

#[test]
fn mixed_arithmetic_groupings() {
    for body in [
        "(a + b) * c",
        "a * (b + c)",
        "(a + b) / (c - d)",
        "a - (b - c)",
        "a - b - c",
        "(a - b) - c",
        "a / (b * c)",
        "a / b * c",
        "(a + b + c) * d",
        "x * (y + z) - w",
    ] {
        assert_reparse_identical(&format!("def m\n  let q = {body}\nend\n"));
    }
}

#[test]
fn logical_and_comparison_groupings() {
    for body in [
        "(a || b) && c",
        "a || (b && c)",
        "(a && b) || (c && d)",
        "!(a && b)",
        "!(a || b)",
        "(a == b) == c",
        "a == (b == c)",
    ] {
        assert_reparse_identical(&format!("def m\n  let q = {body}\nend\n"));
    }
}

#[test]
fn bitwise_and_shift_groupings() {
    for body in [
        "(a | b) & c",
        "a | (b & c)",
        "(a ^ b) | c",
        "(a << b) + c",
        "a << (b + c)",
        "(a & b) << c",
    ] {
        assert_reparse_identical(&format!("def m\n  let q = {body}\nend\n"));
    }
}

#[test]
fn unary_and_cast_groupings() {
    for body in [
        "-(a + b)",
        "-(a * b)",
        "(a + b) as Int",
        "(a - b) as Float",
        "-a + b",
        "-a * b",
        "a + -b",
    ] {
        assert_reparse_identical(&format!("def m\n  let q = {body}\nend\n"));
    }
}
