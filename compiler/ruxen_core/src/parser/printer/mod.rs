//! Debug pretty-printer for the Ruxen AST.
//!
//! Dumps any AST node into readable, indented text output. Expressions are
//! shown in abbreviated form to keep output manageable.

use super::ast::*;

mod exprs;
mod format;
mod items;

pub use format::{format_expr_short, format_pattern, format_type, format_type_path};

// ─── PrettyPrinter ──────────────────────────────────────────────────

pub struct PrettyPrinter {
    indent: usize,
    output: String,
}

impl Default for PrettyPrinter {
    fn default() -> Self {
        Self::new()
    }
}

impl PrettyPrinter {
    pub fn new() -> Self {
        Self {
            indent: 0,
            output: String::new(),
        }
    }

    pub fn print_program(mut self, program: &Program) -> String {
        self.line("Program");
        self.indent();
        for item in &program.items {
            self.print_top_level_item(item);
        }
        self.dedent();
        self.output
    }

    // ── helpers ──────────────────────────────────────────────────────

    pub(super) fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
        self.output.push_str(text);
        self.output.push('\n');
    }

    pub(super) fn indent(&mut self) {
        self.indent += 1;
    }

    pub(super) fn dedent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }
}
