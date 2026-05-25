//! REPL command dispatch.
//!
//! Commands use the `:` prefix (GHCi convention).

/// A parsed REPL command.
#[derive(Debug)]
pub enum Command {
    Help,
    Quit,
    Reset,
    Type(String),
    Unknown(String),
}

/// Parse a REPL command from a `:` prefixed line.
pub fn parse_command(line: &str) -> Option<Command> {
    let trimmed = line.trim();
    if !trimmed.starts_with(':') {
        return None;
    }

    let rest = &trimmed[1..];
    let mut parts = rest.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("").trim();

    Some(match cmd {
        "help" | "h" => Command::Help,
        "quit" | "exit" | "q" => Command::Quit,
        "reset" => Command::Reset,
        "type" | "t" => Command::Type(arg.to_string()),
        _ => Command::Unknown(cmd.to_string()),
    })
}

/// Display help text for available commands.
pub fn help_text() -> &'static str {
    "\x1b[1mAvailable commands:\x1b[0m
:help, :h       Show this help message
:quit, :exit, :q  Exit the REPL
:reset          Clear all state (variables, definitions, types)
:type <expr>    Show the type of an expression without evaluating it"
}
