//! Ruxen REPL — Interactive shell for the Ruxen programming language.
//!
//! Uses Cranelift JIT for in-process compilation and execution. The unified
//! `ruxen` driver invokes this crate as `ruxen repl` via [`run`].

mod capture;
mod commands;
mod completion;
mod display;
mod env;
mod eval;
mod highlight;
pub mod jit;
mod session;
mod slots;
mod validate;

use std::borrow::Cow;

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::{ValidationContext, ValidationResult, Validator};
use rustyline::{Config, Context, EditMode, Editor, Helper};

use completion::RuxenCompleter;
use highlight::RuxenHighlighter;
use session::ReplSession;
use validate::RuxenValidator;

/// Split a piped-stdin buffer into logical REPL chunks.
///
/// Accumulates lines until the parser accepts the buffer as one REPL input
/// (or rejects it with every block and bracket closed), then emits it. Used
/// when stdin is not a TTY so multi-line items like `class Foo ... end` are
/// evaluated as one input. The parser decides, so the split agrees with what
/// `eval_input` will parse.
fn split_repl_chunks(input: &str) -> Vec<String> {
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
        match chunk_state(&current) {
            // Only comments: nothing to evaluate.
            ChunkState::Empty => current.clear(),
            ChunkState::Open => {}
            ChunkState::Done => chunks.push(std::mem::take(&mut current)),
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Where an accumulated piped-input chunk stands.
enum ChunkState {
    /// Comments and blank lines only.
    Empty,
    /// Needs more lines: an open block, bracket, string or block comment.
    Open,
    /// One complete input, or one the parser rejects with nothing left open.
    Done,
}

fn chunk_state(src: &str) -> ChunkState {
    use ruxen_core::lexer::token::TokenKind;
    use ruxen_core::lexer::Lexer;
    use ruxen_core::parser::ast::ReplParseResult;
    use ruxen_core::parser::Parser;

    let tokens = match Lexer::new(src).tokenize() {
        Ok(tokens) => tokens,
        // E0001 unterminated block comment, E0002 unterminated string: the
        // next line may close it. Any other lex error is final.
        Err(diags) => {
            let unterminated = diags
                .iter()
                .any(|d| matches!(d.code.as_deref(), Some("E0001" | "E0002")));
            return if unterminated {
                ChunkState::Open
            } else {
                ChunkState::Done
            };
        }
    };
    if tokens.iter().all(|t| {
        matches!(
            t.kind,
            TokenKind::Newline | TokenKind::DocComment(_) | TokenKind::Eof
        )
    }) {
        return ChunkState::Empty;
    }
    match Parser::new(tokens.clone()).parse_repl_input() {
        ReplParseResult::Complete(_) => ChunkState::Done,
        ReplParseResult::Incomplete => ChunkState::Open,
        // A syntax error inside a block that is still open: keep reading to
        // its `end` so the error is reported once, for the whole block.
        ReplParseResult::Error(_) if Parser::has_unclosed_delimiters(&tokens) => ChunkState::Open,
        ReplParseResult::Error(_) => ChunkState::Done,
    }
}

/// Combined rustyline helper implementing all traits.
struct RuxenHelper {
    completer: RuxenCompleter,
    highlighter: RuxenHighlighter,
    validator: RuxenValidator,
}

impl Completer for RuxenHelper {
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

impl Highlighter for RuxenHelper {
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

impl Hinter for RuxenHelper {
    type Hint = String;

    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        None
    }
}

impl Validator for RuxenHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> rustyline::Result<ValidationResult> {
        self.validator.validate(ctx)
    }
}

impl Helper for RuxenHelper {}

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PRIMARY_PROMPT: &str = "ruxen> ";

/// Run the interactive REPL until the user quits (`:quit` or EOF).
///
/// `--version` / `--help` for `ruxen repl` are handled by clap at the
/// `ruxen` driver layer and never reach this function.
pub fn run() -> Result<(), String> {
    println!("Ruxen {} REPL — Type :help for commands", VERSION);

    let mut session =
        ReplSession::new().map_err(|e| format!("Failed to initialize REPL: {}", e))?;

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
                        return Ok(());
                    }
                    eval::EvalResult::Incomplete => {
                        eprintln!("Error: Incomplete input: {}", trimmed);
                    }
                    eval::EvalResult::Error(msg) => eprintln!("{}", msg),
                }
            }
        }
        println!("Goodbye!");
        return Ok(());
    }

    let config = Config::builder()
        .edit_mode(EditMode::Emacs)
        .auto_add_history(true)
        .build();

    let helper = RuxenHelper {
        completer: RuxenCompleter,
        highlighter: RuxenHighlighter,
        validator: RuxenValidator,
    };

    let mut editor: Editor<RuxenHelper, _> =
        Editor::with_config(config).map_err(|e| format!("Failed to create editor: {}", e))?;
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
    Ok(())
}
#[cfg(test)]
mod tests;
