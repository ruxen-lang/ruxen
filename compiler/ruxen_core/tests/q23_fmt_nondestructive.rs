//! Q23 — `ruxen fmt` was destructive. Two faults, both fixed here:
//!
//! (a) Doc-comment stripping. `##` doc comments on a method NESTED inside a
//!     class/struct/enum/impl/mixin body were dropped: `format_program`
//!     emitted leading comments only for TOP-LEVEL items, and methods are
//!     formatted by direct `format_func_def` calls that bypassed that path.
//!     Now `format_func_with_leading_comments` emits them at every nested
//!     site (mirroring the class-body `lib` FFI-def doc handling).
//!
//! (b) Top-level `Tester.describe(...) do … end` (the shape of every
//!     `tests/*.rx`) made the formatter error at 1:1, because
//!     `parse_top_level_item` rejected a top-level expression statement. The
//!     SHARED parser now accepts a clean top-level expression statement as a
//!     `TopLevelItem::Expr`, so the formatter round-trips it. The direct
//!     compile path still rejects it (resolve → E0728); `ruxen test` wraps
//!     test files in a synthesised `def main` before compiling, so it is
//!     unaffected.

use ruxen_core::formatter;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;

fn fmt(src: &str) -> formatter::FormatResult {
    formatter::format(src)
}

/// (a) Doc comments on a class method survive formatting and round-trip.
#[test]
fn class_method_doc_comments_preserved() {
    let src = "class Canvas\n  ## Draw a path on the canvas.\n  ## Takes a list of points.\n  def draw_path(pts: Int) -> nil\n    self\n  end\nend\n";
    let r = fmt(src);
    assert!(r.errors.is_empty(), "format errored: {:?}", r.errors);
    assert!(
        r.output.contains("## Draw a path on the canvas."),
        "first doc line stripped; output:\n{}",
        r.output
    );
    assert!(
        r.output.contains("## Takes a list of points."),
        "second doc line stripped; output:\n{}",
        r.output
    );
    // Idempotence — the sacred formatter invariant.
    let r2 = fmt(&r.output);
    assert_eq!(r.output, r2.output, "not idempotent");
}

/// (a) Doc comments on an `extension`-block (impl) method survive too.
#[test]
fn extension_method_doc_comments_preserved() {
    let src =
        "extension Foo\n  ## Documented extension method.\n  def bar -> Int\n    1\n  end\nend\n";
    let r = fmt(src);
    assert!(r.errors.is_empty(), "format errored: {:?}", r.errors);
    assert!(
        r.output.contains("## Documented extension method."),
        "extension-method doc stripped; output:\n{}",
        r.output
    );
    let r2 = fmt(&r.output);
    assert_eq!(r.output, r2.output, "not idempotent");
}

/// (b) A top-level `Tester.describe(...) do … end` file formats cleanly
/// (no parse error) and round-trips idempotently.
#[test]
fn top_level_describe_formats_cleanly() {
    let src =
        "Tester.describe(\"d\") do |t: &var Tester|\n  t.it(\"works\") do\n    1\n  end\nend\n";
    let r = fmt(src);
    assert!(
        r.errors.is_empty(),
        "formatter parse errors on top-level Tester.describe: {:?}",
        r.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert!(
        r.output.contains("Tester.describe"),
        "describe body dropped; output:\n{}",
        r.output
    );
    let r2 = fmt(&r.output);
    assert_eq!(r.output, r2.output, "not idempotent");
}

/// (b) The shared main parser ACCEPTS the top-level describe (1 item) — the
/// formatter parser is the same parser, so they stay consistent.
#[test]
fn main_parser_accepts_top_level_describe() {
    let src = "Tester.describe(\"d\") do |t: &var Tester|\n  1\nend\n";
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    let program = parser
        .parse()
        .expect("parse should accept top-level expr stmt");
    assert_eq!(
        program.items.len(),
        1,
        "expected exactly one top-level item"
    );
}

/// Negative: a genuine top-level garbage token sequence (not a clean
/// expression landing on a boundary) still errors — the new expr-stmt path
/// must not swallow real top-level-declaration typos.
#[test]
fn top_level_garbage_still_errors() {
    let src = "foo bar baz\n";
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize().expect("lex");
    let mut parser = Parser::new(tokens);
    assert!(
        parser.parse().is_err(),
        "top-level `foo bar baz` should still be a parse error"
    );
}

// ─── Q30 — `ruxen fmt` must not rewrite builder-closure call shapes into a
//     crashing form. Both GUI agents hit this: `fmt` was dropping the no-arg
//     `||` closure header and stripping `()` off a zero-arg call, turning
//     compiling code into a documented segfault shape. These round-trip pins
//     lock the three shapes the fix preserves.

/// (Q30) A no-arg closure header `{ || … }` must survive formatting — the AST
/// can't tell it from a no-pipe `{ … }` block, and emitting the bare-brace form
/// is a documented GUI-stack crash shape (the closure re-parses ambiguously).
#[test]
fn q30_no_arg_closure_header_preserved() {
    let src =
        "def main\n  let app = { || App.build({ |ui, root| root.text(\"hi\") }) }\n  app\nend\n";
    let r = fmt(src);
    assert!(r.errors.is_empty(), "format errored: {:?}", r.errors);
    assert!(
        r.output.contains("{ || App.build"),
        "no-arg closure header `||` dropped; output:\n{}",
        r.output
    );
    // The inner builder block must stay a BRACE block, never auto-convert to
    // `do…end` (a free-function `do…end` block-arg is a crash shape here).
    assert!(
        r.output.contains("App.build({ |ui, root|"),
        "brace block-arg rewritten (do…end?); output:\n{}",
        r.output
    );
    assert!(
        !r.output.contains("do |ui, root|"),
        "brace block-arg auto-converted to do…end; output:\n{}",
        r.output
    );
    let r2 = fmt(&r.output);
    assert_eq!(r.output, r2.output, "not idempotent");
}

/// (Q30) A zero-arg CALL expression keeps its parens: `row_height()` must not
/// become a bare-name reference `row_height` (a call → identifier semantic
/// change). A `Call` AST node only exists when the source wrote `()`.
#[test]
fn q30_zero_arg_call_keeps_parens() {
    let src = "def main\n  let h = row_height()\n  h\nend\n";
    let r = fmt(src);
    assert!(r.errors.is_empty(), "format errored: {:?}", r.errors);
    assert!(
        r.output.contains("row_height()"),
        "parens stripped off zero-arg call; output:\n{}",
        r.output
    );
    let r2 = fmt(&r.output);
    assert_eq!(r.output, r2.output, "not idempotent");
}

/// (Q30) The full builder shape both GUI agents wrote — closure header, brace
/// block-arg, and zero-arg call together — round-trips byte-for-byte.
#[test]
fn q30_builder_shape_round_trips() {
    let src = concat!(
        "def main\n",
        "  let app = { || App.build({ |ui, root| root.text(\"hi\") }) }\n",
        "  let h = row_height()\n",
        "  app\n",
        "end\n",
    );
    let r = fmt(src);
    assert!(r.errors.is_empty(), "format errored: {:?}", r.errors);
    assert_eq!(
        r.output, src,
        "builder shape was rewritten by fmt; output:\n{}",
        r.output
    );
    let r2 = fmt(&r.output);
    assert_eq!(r.output, r2.output, "not idempotent");
}
