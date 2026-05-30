//! Tests for the piped-stdin `split_repl_chunks` helper. Moved here
//! from the inline `mod tests` block in `lib.rs` when the crate
//! grew a top-level `tests/` directory module.

use crate::split_repl_chunks;

#[test]
fn empty_input_yields_no_chunks() {
    assert!(split_repl_chunks("").is_empty());
}

#[test]
fn whitespace_only_yields_no_chunks() {
    assert!(split_repl_chunks("   \n  \n").is_empty());
    assert!(split_repl_chunks("\n").is_empty());
}

#[test]
fn single_expression_without_newline_is_one_chunk() {
    let chunks = split_repl_chunks("1 + 2");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].trim(), "1 + 2");
}

#[test]
fn single_expression_with_newline_is_one_chunk() {
    let chunks = split_repl_chunks("1 + 2\n");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], "1 + 2\n");
}

#[test]
fn two_expressions_two_chunks() {
    let chunks = split_repl_chunks("1 + 1\n2 + 2\n");
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0], "1 + 1\n");
    assert_eq!(chunks[1], "2 + 2\n");
}

#[test]
fn def_end_block_is_single_chunk() {
    let src = "def foo\n  1\nend\n";
    let chunks = split_repl_chunks(src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], src);
}

#[test]
fn def_end_without_trailing_newline_is_single_chunk() {
    let src = "def foo\n  1\nend";
    let chunks = split_repl_chunks(src);
    assert_eq!(chunks.len(), 1);
    // Accumulated chunk contents must match input.
    assert_eq!(chunks[0], src);
}

#[test]
fn class_with_nested_def_is_single_chunk() {
    // class(+1), def(+1), end(-1), end(-1) = 0 → balanced on last line.
    let src = "class Foo\n  def bar\n    1\n  end\nend\n";
    let chunks = split_repl_chunks(src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], src);
}

#[test]
fn command_on_own_line_emitted_immediately() {
    let chunks = split_repl_chunks(":help\n");
    assert_eq!(chunks.len(), 1);
    assert!(chunks[0].starts_with(":help"));
}

#[test]
fn command_without_newline_is_single_chunk() {
    let chunks = split_repl_chunks(":quit");
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].trim(), ":quit");
}

#[test]
fn command_then_expression_two_chunks() {
    let chunks = split_repl_chunks(":help\n1 + 2\n");
    assert_eq!(chunks.len(), 2);
    assert!(chunks[0].starts_with(":help"));
    assert!(chunks[1].contains("1 + 2"));
}

#[test]
fn unbalanced_paren_accumulates_then_balances() {
    // Open `(` on one line, close on next: should end as one chunk.
    let src = "(1 +\n 2)\n";
    let chunks = split_repl_chunks(src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], src);
}

#[test]
fn unbalanced_bracket_accumulates_then_balances() {
    let src = "[1,\n 2,\n 3]\n";
    let chunks = split_repl_chunks(src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], src);
}

#[test]
fn unclosed_string_accumulates_until_eof_flush() {
    // Lexer rejects the unclosed literal; the `"`-count fallback keeps
    // accumulating until EOF. The tail flush still emits the buffer so
    // the caller can report a clean error rather than silently
    // swallowing the input.
    let src = "\"hello";
    let chunks = split_repl_chunks(src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], src);
}

#[test]
fn multiline_triple_quoted_string_is_single_chunk() {
    // `"""a\nb"""` — opener and closer on different lines: the first
    // line alone is unterminated, so accumulation continues until the
    // closing `"""` balances delimiters.
    let src = "\"\"\"abc\ndef\"\"\"\n";
    let chunks = split_repl_chunks(src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], src);
}

#[test]
fn let_binding_is_single_chunk() {
    let src = "let x = 42\n";
    let chunks = split_repl_chunks(src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], src);
}

#[test]
fn sequential_let_and_expr_two_chunks() {
    let chunks = split_repl_chunks("let x = 1\nx + 1\n");
    assert_eq!(chunks.len(), 2);
    assert!(chunks[0].contains("let x = 1"));
    assert!(chunks[1].contains("x + 1"));
}

#[test]
fn def_then_call_yields_two_chunks() {
    // A complete `def ... end` followed by a separate expression.
    let src = "def id(x: Int) -> Int\n  x\nend\nid(5)\n";
    let chunks = split_repl_chunks(src);
    assert_eq!(chunks.len(), 2);
    assert!(chunks[0].contains("def id"));
    assert!(chunks[0].trim_end().ends_with("end"));
    assert!(chunks[1].contains("id(5)"));
}

#[test]
fn blank_lines_between_chunks_are_skipped() {
    let chunks = split_repl_chunks("1\n\n2\n");
    // The blank line is skipped by the `trimmed.is_empty()` guard and
    // never appears as its own chunk.
    assert_eq!(chunks.len(), 2);
    assert!(chunks[0].trim().starts_with('1'));
    assert!(chunks[1].trim().starts_with('2'));
}

#[test]
fn trailing_open_def_is_flushed_at_eof() {
    // `def foo` with no matching `end` never balances; the tail flush
    // still emits the accumulated text so the caller can report an
    // "Incomplete input" error instead of dropping the input on the floor.
    let src = "def foo\n";
    let chunks = split_repl_chunks(src);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0], src);
}
