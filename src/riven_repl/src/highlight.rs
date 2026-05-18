//! Basic syntax highlighting for REPL input.
//!
//! Phase 1: keyword highlighting only.

use std::borrow::Cow;

use rustyline::highlight::Highlighter;

/// Basic keyword highlighter for Riven REPL input.
pub struct RivenHighlighter;

impl Highlighter for RivenHighlighter {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        // Phase 1: no highlighting (return as-is)
        Cow::Borrowed(line)
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        default: bool,
    ) -> Cow<'b, str> {
        if default {
            Cow::Owned(format!("\x1b[1;34m{}\x1b[0m", prompt))
        } else {
            Cow::Borrowed(prompt)
        }
    }
}
