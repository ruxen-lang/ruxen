//! Tab completion for the REPL.
//!
//! Phase 1: keyword and command completion only.

use rustyline::completion::{Completer, Pair};
use rustyline::Context;

/// Basic keyword and command completer.
pub struct RuxenCompleter;

// Keywords surfaced by tab-completion. Tracks §3.14 of the surface-syntax
// spec — retired keywords (`trait`, `impl`, `mut`, `pub`, `None`) are not
// listed; their replacements (`mixin`, `extension`, `var`, public-default
// section markers, `nil`) are.
const KEYWORDS: &[&str] = &[
    "def",
    "end",
    "class",
    "struct",
    "enum",
    "mixin",
    "extension",
    "include",
    "exclude",
    "let",
    "var",
    "if",
    "elsif",
    "else",
    "match",
    "while",
    "for",
    "in",
    "loop",
    "do",
    "return",
    "true",
    "false",
    "self",
    "Self",
    "public",
    "private",
    "protected",
    "use",
    "module",
    "package",
    "some",
    "any",
    "where",
    "as",
    "lib",
    "unsafe",
    "consume",
    "inline",
    "layout",
    "break",
    "continue",
    "Some",
    "nil",
    "Ok",
    "Err",
];

const COMMANDS: &[&str] = &[":help", ":quit", ":exit", ":q", ":reset", ":type"];

impl Completer for RuxenCompleter {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let (start, word) = extract_word(line, pos);

        if word.is_empty() {
            return Ok((pos, Vec::new()));
        }

        let mut candidates = Vec::new();

        if word.starts_with(':') {
            for &cmd in COMMANDS {
                if cmd.starts_with(word) {
                    candidates.push(Pair {
                        display: cmd.to_string(),
                        replacement: cmd.to_string(),
                    });
                }
            }
        } else {
            for &kw in KEYWORDS {
                if kw.starts_with(word) {
                    candidates.push(Pair {
                        display: kw.to_string(),
                        replacement: kw.to_string(),
                    });
                }
            }
        }

        Ok((start, candidates))
    }
}

fn extract_word(line: &str, pos: usize) -> (usize, &str) {
    let bytes = line.as_bytes();
    let mut start = pos;
    while start > 0 {
        let b = bytes[start - 1];
        if b.is_ascii_alphanumeric() || b == b'_' || b == b':' {
            start -= 1;
        } else {
            break;
        }
    }
    (start, &line[start..pos])
}
