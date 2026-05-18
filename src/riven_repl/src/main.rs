//! Riven REPL — Interactive shell for the Riven programming language.
//!
//! Uses Cranelift JIT for in-process compilation and execution.

#[allow(dead_code)]
mod capture;
#[allow(dead_code)]
mod commands;
mod completion;
#[allow(dead_code)]
mod display;
#[allow(dead_code)]
mod env;
mod eval;
mod highlight;
#[allow(dead_code)]
mod jit;
mod session;
mod validate;

use std::borrow::Cow;

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Config, Context, EditMode, Editor, Helper};

use completion::RivenCompleter;
use highlight::RivenHighlighter;
use session::ReplSession;
use validate::RivenValidator;

/// Split a piped-stdin buffer into logical REPL chunks.
///
/// Accumulates lines until delimiter balance returns to zero (no
/// open `def`/`class`/`enum`/`(`/`[`/`{` etc.), then emits the
/// accumulated chunk. Used when stdin is not a TTY so multi-line
/// items like `class Foo ... end` are evaluated as one input.
fn split_repl_chunks(input: &str) -> Vec<String> {
    use riven_core::lexer::token::TokenKind;
    use riven_core::lexer::Lexer;

    let mut chunks = Vec::new();
    let mut current = String::new();
    for raw_line in input.split_inclusive('\n') {
        current.push_str(raw_line);
        let trimmed = current.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Commands are single-line: emit immediately.
        if trimmed.starts_with(':') {
            chunks.push(std::mem::take(&mut current));
            continue;
        }
        let mut block: i32 = 0;
        let mut paren: i32 = 0;
        let mut bracket: i32 = 0;
        let mut brace: i32 = 0;
        let mut unclosed_string = false;
        let mut unclosed_block_comment = false;
        let mut meaningful_tokens = 0usize;
        match Lexer::new(&current).tokenize() {
            Ok(tokens) => {
                // Pre-pass: for each line, record
                //   - whether it contains `->` (match arm / one-line fn)
                //   - whether it contains `=` / `=>` with a body after (one-line
                //     expression form).
                //
                // Also walk the token stream once to identify `def` tokens
                // that appear to be trait-style one-line declarations (no
                // body). A `def` whose line has no body-start (no `=`, no
                // Newline-before-body) and which sits directly inside a
                // `trait ... end` is a signature and should NOT count as
                // a block-opener.
                #[derive(Clone, Copy, PartialEq)]
                enum LineKind {
                    Normal,
                    HasArrow,
                }
                let mut line_kinds: Vec<LineKind> = Vec::new();
                let mut cur = LineKind::Normal;
                for t in &tokens {
                    match &t.kind {
                        TokenKind::Arrow | TokenKind::FatArrow => cur = LineKind::HasArrow,
                        TokenKind::Newline => {
                            line_kinds.push(cur);
                            cur = LineKind::Normal;
                        }
                        TokenKind::Eof => {
                            line_kinds.push(cur);
                        }
                        _ => {}
                    }
                }

                // Identify `def` tokens that are trait-style signatures
                // (no body). Two conditions must both hold:
                //   (1) The `def` sits directly inside a `trait` at the top
                //       of the opener stack (simulated while walking).
                //   (2) The next meaningful token after the def-line's
                //       Newline is `def`, `pub`, `protected`, `type`, or
                //       `end` — i.e. the trait body either continues with
                //       another decl or closes without the def having a
                //       body of its own.
                let mut def_is_signature: Vec<bool> = vec![false; tokens.len()];
                {
                    let mut sim_stack: Vec<&'static str> = Vec::new();
                    let mut sim_in_type = false;
                    for (i, t) in tokens.iter().enumerate() {
                        match &t.kind {
                            // ruby-naming.spec.md §3.4: `mixin` replaces
                            // `trait`. Body-only `def`s inside a mixin
                            // are signatures (no body); the def-sig
                            // detector below uses the `"trait"` marker.
                            TokenKind::Mixin => sim_stack.push("trait"),
                            TokenKind::Class => sim_stack.push("class"),
                            TokenKind::Struct => sim_stack.push("struct"),
                            TokenKind::Enum => sim_stack.push("enum"),
                            // `extension X` body is structurally an impl
                            // block: methods have bodies, so we keep
                            // mirroring the historical impl behaviour.
                            TokenKind::Extension if !sim_in_type => sim_stack.push("impl"),
                            TokenKind::Def => {
                                // Only consider signature if directly inside
                                // a trait at this point.
                                if sim_stack.last().copied() == Some("trait") {
                                    // Look ahead to next meaningful token
                                    // after this line's Newline.
                                    let mut j = i + 1;
                                    while j < tokens.len()
                                        && !matches!(
                                            tokens[j].kind,
                                            TokenKind::Newline | TokenKind::Eof
                                        )
                                    {
                                        j += 1;
                                    }
                                    let mut k = j + 1;
                                    while k < tokens.len() {
                                        match &tokens[k].kind {
                                            TokenKind::Newline | TokenKind::DocComment(_) => k += 1,
                                            _ => break,
                                        }
                                    }
                                    if let Some(t2) = tokens.get(k) {
                                        let is_sig_follower = matches!(
                                            t2.kind,
                                            TokenKind::Def
                                                | TokenKind::Protected
                                                | TokenKind::Type
                                                | TokenKind::End
                                        );
                                        if is_sig_follower {
                                            def_is_signature[i] = true;
                                        }
                                    }
                                }
                                if !def_is_signature[i] {
                                    sim_stack.push("def");
                                }
                            }
                            TokenKind::While
                            | TokenKind::Loop
                            | TokenKind::Match
                            | TokenKind::Do
                            | TokenKind::For
                            | TokenKind::If
                            | TokenKind::Module => {
                                // Rough simulation — doesn't matter for the
                                // trait-signature decision, just need
                                // accurate trait stack nesting.
                                sim_stack.push("ctrl");
                            }
                            TokenKind::End => {
                                sim_stack.pop();
                            }
                            _ => {}
                        }
                        // Very light type-position tracking for `impl` so we
                        // don't treat `-> impl Trait` as an impl block.
                        sim_in_type = matches!(
                            t.kind,
                            TokenKind::Arrow
                                | TokenKind::FatArrow
                                | TokenKind::Colon
                                | TokenKind::Amp
                                | TokenKind::Comma
                                | TokenKind::LParen
                        );
                    }
                }

                // Per-token line index.
                let mut line_idx = 0usize;
                let mut opener_stack: Vec<&'static str> = Vec::new();
                let mut impl_header_pending = false;
                // Track the last non-trivia token on the current line so we
                // can tell whether `impl` here is a type position (preceded
                // by `:`, `&`, `,`, `(`, `->`, `=`, etc.) vs a statement
                // start.
                #[derive(Clone, Copy, PartialEq)]
                enum PrevKind {
                    None,
                    TypeContext, // `:`, `&`, `,`, `(`, `->`, `=`, etc.
                    Other,
                }
                let mut prev_on_line = PrevKind::None;
                for (tok_idx, t) in tokens.iter().enumerate() {
                    let on_arrow_line =
                        matches!(line_kinds.get(line_idx).copied(), Some(LineKind::HasArrow));
                    match &t.kind {
                        // ruby-naming.spec.md §3.4: `mixin` replaces `trait`.
                        TokenKind::Mixin => {
                            block += 1;
                            opener_stack.push("mixin");
                            meaningful_tokens += 1;
                        }
                        // ruby-naming.spec.md §3.4a: `extension` block.
                        TokenKind::Extension => {
                            block += 1;
                            opener_stack.push("extension");
                            meaningful_tokens += 1;
                        }
                        TokenKind::Class => {
                            block += 1;
                            opener_stack.push("class");
                            meaningful_tokens += 1;
                        }
                        TokenKind::Struct => {
                            block += 1;
                            opener_stack.push("struct");
                            meaningful_tokens += 1;
                        }
                        TokenKind::Enum => {
                            block += 1;
                            opener_stack.push("enum");
                            meaningful_tokens += 1;
                        }
                        TokenKind::Module => {
                            block += 1;
                            opener_stack.push("module");
                            meaningful_tokens += 1;
                        }
                        TokenKind::For => {
                            if impl_header_pending {
                                impl_header_pending = false;
                            } else {
                                block += 1;
                                opener_stack.push("ctrl");
                            }
                            meaningful_tokens += 1;
                        }
                        TokenKind::While | TokenKind::Loop | TokenKind::Match | TokenKind::Do => {
                            block += 1;
                            opener_stack.push("ctrl");
                            meaningful_tokens += 1;
                        }
                        TokenKind::Def => {
                            // A signature-only `def` (no body — inside a
                            // trait, followed by another decl or `end`) does
                            // not open an `end`-closed scope.
                            if def_is_signature[tok_idx] {
                                // Signature, no block.
                            } else {
                                block += 1;
                                opener_stack.push("def");
                            }
                            meaningful_tokens += 1;
                        }
                        TokenKind::If => {
                            if !on_arrow_line {
                                block += 1;
                                opener_stack.push("if");
                            }
                            meaningful_tokens += 1;
                        }
                        TokenKind::End => {
                            block -= 1;
                            opener_stack.pop();
                            meaningful_tokens += 1;
                        }
                        TokenKind::LParen => {
                            paren += 1;
                            meaningful_tokens += 1;
                        }
                        TokenKind::RParen => {
                            paren -= 1;
                            meaningful_tokens += 1;
                        }
                        TokenKind::LBracket => {
                            bracket += 1;
                            meaningful_tokens += 1;
                        }
                        TokenKind::RBracket => {
                            bracket -= 1;
                            meaningful_tokens += 1;
                        }
                        TokenKind::LBrace => {
                            brace += 1;
                            meaningful_tokens += 1;
                        }
                        TokenKind::RBrace => {
                            brace -= 1;
                            meaningful_tokens += 1;
                        }
                        TokenKind::Newline => {
                            line_idx += 1;
                            impl_header_pending = false;
                            prev_on_line = PrevKind::None;
                            continue;
                        }
                        TokenKind::DocComment(_) => {
                            continue;
                        }
                        TokenKind::Eof => break,
                        _ => {
                            meaningful_tokens += 1;
                        }
                    }
                    // Update prev_on_line for the NEXT token's context check.
                    prev_on_line = match &t.kind {
                        TokenKind::Colon
                        | TokenKind::Amp
                        | TokenKind::Comma
                        | TokenKind::LParen
                        | TokenKind::LBracket
                        | TokenKind::Arrow
                        | TokenKind::FatArrow
                        | TokenKind::Eq => PrevKind::TypeContext,
                        TokenKind::Newline | TokenKind::Eof | TokenKind::DocComment(_) => {
                            prev_on_line
                        }
                        _ => PrevKind::Other,
                    };
                }
            }
            Err(diags) => {
                for d in &diags {
                    if d.message.contains("unterminated block comment") {
                        unclosed_block_comment = true;
                    } else if d.message.contains("unterminated string literal") {
                        unclosed_string = true;
                    }
                }
                if !unclosed_string && current.chars().filter(|&c| c == '"').count() % 2 != 0 {
                    unclosed_string = true;
                }
            }
        }
        let balanced = block <= 0
            && paren <= 0
            && bracket <= 0
            && brace <= 0
            && !unclosed_string
            && !unclosed_block_comment;
        if balanced {
            // Skip chunks that are pure comments / whitespace / doc comments —
            // the parser would otherwise report "Incomplete" and the REPL
            // would surface it as a spurious error in piped mode.
            if meaningful_tokens == 0 {
                current.clear();
            } else {
                chunks.push(std::mem::take(&mut current));
            }
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Combined rustyline helper implementing all traits.
struct RivenHelper {
    completer: RivenCompleter,
    highlighter: RivenHighlighter,
    validator: RivenValidator,
}

impl Completer for RivenHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        self.completer.complete(line, pos, ctx)
    }
}

impl Highlighter for RivenHelper {
    fn highlight<'l>(&self, line: &'l str, pos: usize) -> Cow<'l, str> {
        self.highlighter.highlight(line, pos)
    }

    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        default: bool,
    ) -> Cow<'b, str> {
        self.highlighter.highlight_prompt(prompt, default)
    }
}

impl Hinter for RivenHelper {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        None
    }
}

impl Validator for RivenHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        self.validator.validate(ctx)
    }
}

impl Helper for RivenHelper {}

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PRIMARY_PROMPT: &str = "riven> ";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        match args[1].as_str() {
            "--version" | "-V" => {
                println!("riven-repl {}", VERSION);
                return;
            }
            "--help" | "-h" => {
                println!("riven-repl {} — interactive Riven REPL", VERSION);
                println!();
                println!("Usage: riven-repl");
                println!();
                println!("Once inside, type :help for REPL commands.");
                return;
            }
            _ => {}
        }
    }

    println!("Riven {} REPL — Type :help for commands", VERSION);

    let mut session = match ReplSession::new() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to initialize REPL: {}", e);
            std::process::exit(1);
        }
    };

    // Non-TTY stdin (piped input): read the whole stream and split into
    // logical chunks by lexer-balance so multi-line `class`/`enum`/`trait`
    // bodies stay together. rustyline's line-at-a-time readline only
    // marks a chunk incomplete if `eval_input` errors with Incomplete,
    // but the parser isn't reliable about that for all opener keywords
    // — a whole-stream pass is robust.
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        use std::io::Read;
        let mut buf = String::new();
        if std::io::stdin().read_to_string(&mut buf).is_ok() {
            let chunks = split_repl_chunks(&buf);
            if std::env::var("REPL_DEBUG_CHUNKS").is_ok() {
                for (i, c) in chunks.iter().enumerate() {
                    eprintln!("--- chunk {} ---\n{}---", i, c);
                }
            }
            for chunk in chunks {
                let trimmed = chunk.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match eval::eval_input(&mut session, &chunk) {
                    eval::EvalResult::Ok(Some(output)) => println!("{}", output),
                    eval::EvalResult::Ok(None) => {}
                    eval::EvalResult::Command(output) => println!("{}", output),
                    eval::EvalResult::Quit => {
                        println!("Goodbye!");
                        return;
                    }
                    eval::EvalResult::Incomplete => {
                        eprintln!("Error: Incomplete input: {}", trimmed);
                    }
                    eval::EvalResult::Error(msg) => eprintln!("{}", msg),
                }
            }
        }
        println!("Goodbye!");
        return;
    }

    let config = Config::builder()
        .edit_mode(EditMode::Emacs)
        .auto_add_history(true)
        .build();

    let helper = RivenHelper {
        completer: RivenCompleter,
        highlighter: RivenHighlighter,
        validator: RivenValidator,
    };

    let mut editor: Editor<RivenHelper, _> = match Editor::with_config(config) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("Failed to create editor: {}", e);
            std::process::exit(1);
        }
    };
    editor.set_helper(Some(helper));

    let _ = editor.load_history(&session.history_path);

    // Accumulates continuation lines while the evaluator reports
    // `Incomplete` (e.g., an unclosed `def ... end` block when input is
    // piped, where rustyline's interactive multi-line handling does not
    // kick in).
    let mut pending: String = String::new();

    loop {
        let prompt = if pending.is_empty() {
            PRIMARY_PROMPT
        } else {
            "..... "
        };
        let readline = editor.readline(prompt);

        match readline {
            Ok(line) => {
                if pending.is_empty() && line.trim().is_empty() {
                    continue;
                }

                // Combine pending input with the new line.
                let to_eval = if pending.is_empty() {
                    line.clone()
                } else {
                    let mut combined = std::mem::take(&mut pending);
                    combined.push('\n');
                    combined.push_str(&line);
                    combined
                };

                match eval::eval_input(&mut session, &to_eval) {
                    eval::EvalResult::Ok(Some(output)) => println!("{}", output),
                    eval::EvalResult::Ok(None) => {}
                    eval::EvalResult::Command(output) => println!("{}", output),
                    eval::EvalResult::Quit => {
                        println!("Goodbye!");
                        break;
                    }
                    eval::EvalResult::Incomplete => {
                        // Keep accumulating until the parser accepts it.
                        pending = to_eval;
                    }
                    eval::EvalResult::Error(msg) => eprintln!("{}", msg),
                }
            }
            Err(ReadlineError::Interrupted) => {
                pending.clear();
                println!("^C");
                continue;
            }
            Err(ReadlineError::Eof) => {
                println!("Goodbye!");
                break;
            }
            Err(err) => {
                eprintln!("Error: {:?}", err);
                break;
            }
        }
    }

    let _ = editor.save_history(&session.history_path);
}
#[cfg(test)]
mod tests {
    use super::split_repl_chunks;

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
}
