//! The one front-end pipeline every tool runs: parse, type check, borrow
//! check and MIR lowering with its loan check.
//!
//! [`parse`] turns source into an AST. [`check`] runs the front end on it up
//! to [`Options::stop_after`] and returns a [`Frontend`] holding everything a
//! later stage or a tool needs: the typed HIR, the diagnostics, the borrow
//! errors and, at [`Stage::Mir`], the MIR program. [`Frontend::render`] is the
//! one text format for all of it.

use std::path::Path;

use crate::borrow_check::errors::BorrowError;
use crate::codegen::target::ResolvedTarget;
use crate::diagnostics::{Diagnostic, DiagnosticLevel};
use crate::hir::nodes::HirProgram;
use crate::lexer::Lexer;
use crate::mir::lower::Lowerer;
use crate::mir::nodes::MirProgram;
use crate::parser::ast::Program;
use crate::parser::Parser;
use crate::resolve::symbols::SymbolTable;
use crate::typeck::TypeCheckResult;

/// Which standard library is loaded in front of the user program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prelude {
    /// The full hosted stdlib.
    Hosted,
    /// The curated wasm subset (`run_wasm_bootstrap_with_package_names`).
    Wasm,
    /// No stdlib: a no_std unit.
    None,
}

/// The last stage [`check`] runs. Each stage runs only when the stages
/// before it reported no error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Resolve and type inference (plus E1400 for a no_std unit).
    Typeck,
    /// The HIR borrow check.
    Borrow,
    /// MIR lowering and the MIR loan check ([`Frontend::lower`]).
    Mir,
}

/// What [`check`] runs and against which stdlib.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub prelude: Prelude,
    /// Reject heap allocation (E1400) and lower without the hosted runtime
    /// helpers ([`Lowerer::new_no_std`]).
    pub no_std: bool,
    pub stop_after: Stage,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            prelude: Prelude::Hosted,
            no_std: false,
            stop_after: Stage::Mir,
        }
    }
}

impl Options {
    /// The options for a build for `target`: the one rule choosing the
    /// stdlib. A no_std unit loads none; a wasm target loads the curated
    /// wasm subset, because the full stdlib pulls in `dispatch runtime`
    /// classes the LLVM wasm backend cannot lower (tier 4.09); every other
    /// target loads the hosted stdlib.
    pub fn for_target(target: &ResolvedTarget, no_std: bool) -> Self {
        let prelude = if no_std {
            Prelude::None
        } else if target.is_wasm() {
            Prelude::Wasm
        } else {
            Prelude::Hosted
        };
        Options {
            prelude,
            no_std,
            ..Options::default()
        }
    }
}

/// Lex and parse `source`.
pub fn parse(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let tokens = Lexer::new(source).tokenize()?;
    Parser::new(tokens).parse()
}

/// `diagnostics` as text, one per line, each prefixed with `file` when
/// given: the format of [`Frontend::render`], for the errors [`parse`]
/// returns.
pub fn render_diagnostics(diagnostics: &[Diagnostic], file: Option<&Path>) -> String {
    let prefix = file_prefix(file);
    diagnostics
        .iter()
        .map(|d| format!("{prefix}{d}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn file_prefix(file: Option<&Path>) -> String {
    file.map(|f| format!("{}: ", f.display()))
        .unwrap_or_default()
}

/// The front end's results for one program.
pub struct Frontend {
    options: Options,
    typeck: TypeCheckResult,
    /// Diagnostics from loading the stdlib. Any one is fatal: the program
    /// cannot be checked against a broken stdlib.
    pub bootstrap: Vec<Diagnostic>,
    /// HIR borrow errors (moves, mutability, blocks, Send), then the MIR
    /// loan errors (E1002, E1009).
    pub borrow_errors: Vec<BorrowError>,
    /// Why MIR lowering failed. Not set when borrow errors exist: a program
    /// that does not borrow check may well not lower, and the borrow error is
    /// the one to fix.
    pub lowering_error: Option<String>,
    /// The lowered program, set by a [`Stage::Mir`] check that found no
    /// error.
    pub mir: Option<MirProgram>,
}

/// Run the front end on `program` up to `options.stop_after`.
///
/// Order: implicit `main` for top-level statements, the async pre-checks
/// (E1112/E1116/E1115, before async lowering rewrites the bodies they
/// inspect), async lowering, resolve and type inference, E1400 for a no_std
/// unit; then, when all of that is clean, the HIR borrow check and MIR
/// lowering with its loan check.
pub fn check(program: &Program, options: &Options) -> Frontend {
    let mut bootstrap = Vec::new();
    let packages = match options.prelude {
        Prelude::Hosted => {
            crate::resolve::bootstrap::run_bootstrap_with_package_names(&mut bootstrap)
        }
        Prelude::Wasm => {
            crate::resolve::bootstrap::run_wasm_bootstrap_with_package_names(&mut bootstrap)
        }
        Prelude::None => Vec::new(),
    };

    let mut lowered = program.clone();
    crate::parser::implicit_main::synthesize_implicit_main(&mut lowered);
    let mut async_diagnostics =
        crate::async_lowering::collect_block_on_in_async_diagnostics(&lowered);
    async_diagnostics
        .extend(crate::async_lowering::collect_task_spawn_outside_async_diagnostics(&lowered));
    async_diagnostics.extend(crate::async_lowering::collect_await_in_loop_diagnostics(
        &lowered,
    ));
    // The `.await` desugar classifies awaitees against the stdlib's Future
    // classes, so it sees the bootstrap programs too.
    let bootstrap_refs: Vec<&Program> = packages.iter().map(|(_, p)| p).collect();
    crate::async_lowering::lower_async_defs_with_bootstrap(&mut lowered, &bootstrap_refs);

    let mut typeck = crate::typeck::type_check_with_bootstrap_packages(&lowered, &packages);
    typeck.diagnostics.extend(async_diagnostics);
    if options.no_std {
        typeck
            .diagnostics
            .extend(crate::no_std::validate(&typeck.program));
    }

    let mut frontend = Frontend {
        options: *options,
        typeck,
        bootstrap,
        borrow_errors: Vec::new(),
        lowering_error: None,
        mir: None,
    };
    // The borrow check and the MIR loan check run on a program that
    // type-checks: the loan check also runs after HIR borrow errors, so
    // their errors join.
    if frontend.typeck_failed() {
        return frontend;
    }
    if options.stop_after >= Stage::Borrow {
        frontend.borrow_errors =
            crate::borrow_check::borrow_check(frontend.hir(), frontend.symbols());
    }
    if options.stop_after >= Stage::Mir {
        frontend.mir = frontend.lower();
    }
    frontend
}

impl Frontend {
    /// The typed HIR.
    pub fn hir(&self) -> &HirProgram {
        &self.typeck.program
    }

    pub fn symbols(&self) -> &SymbolTable {
        &self.typeck.symbols
    }

    /// Type-check diagnostics (errors and warnings), including the async
    /// pre-checks and E1400.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.typeck.diagnostics
    }

    /// Whether any stage reported an error. Warnings do not count; a stdlib
    /// bootstrap diagnostic always does.
    pub fn has_errors(&self) -> bool {
        self.typeck_failed() || !self.borrow_errors.is_empty() || self.lowering_error.is_some()
    }

    /// Whether loading the stdlib or type checking reported an error: the
    /// typed HIR is then not fit for the borrow checkers or lowering.
    fn typeck_failed(&self) -> bool {
        !self.bootstrap.is_empty()
            || self
                .typeck
                .diagnostics
                .iter()
                .any(|d| d.level == DiagnosticLevel::Error)
    }

    /// Lower the typed HIR to MIR and check its loans. [`check`] calls this
    /// at [`Stage::Mir`]; a caller that checked to [`Stage::Borrow`] calls it
    /// to go on. Returns `None` when there is nothing to codegen: a borrow
    /// error or a lowering error. The caller must not call it when type
    /// checking failed. The MIR loan errors join the HIR borrow errors.
    pub fn lower(&mut self) -> Option<MirProgram> {
        let symbols = &self.typeck.symbols;
        let mut lowerer = if self.options.no_std {
            Lowerer::new_no_std(symbols)
        } else {
            Lowerer::new(symbols)
        };
        match lowerer.lower_program_checked(&self.typeck.program) {
            Ok((mir, loan_errors)) => {
                self.borrow_errors.extend(loan_errors);
                self.borrow_errors.is_empty().then_some(mir)
            }
            Err(error) => {
                if self.borrow_errors.is_empty() {
                    self.lowering_error = Some(error);
                }
                None
            }
        }
    }

    /// Every diagnostic and error as text, one per line: the stdlib
    /// diagnostics under one `stdlib bootstrap failed:` header, then the
    /// type-check diagnostics (warnings too), the borrow errors and the
    /// lowering error. Each line after the header is prefixed with `file`
    /// when given.
    pub fn render(&self, file: Option<&Path>) -> String {
        let prefix = file_prefix(file);
        let mut lines = Vec::new();
        if !self.bootstrap.is_empty() {
            lines.push("stdlib bootstrap failed:".to_string());
            lines.extend(self.bootstrap.iter().map(|d| format!("  {d}")));
        }
        lines.extend(
            self.typeck
                .diagnostics
                .iter()
                .map(|d| format!("{prefix}{d}")),
        );
        lines.extend(
            self.borrow_errors
                .iter()
                .map(|e| format!("{prefix}{}", e.to_string().trim_end())),
        );
        if let Some(error) = &self.lowering_error {
            lines.push(format!("{prefix}MIR lowering error: {error}"));
        }
        lines.join("\n")
    }

    /// The type-check result, with the stdlib diagnostics and the lowering
    /// error (an error diagnostic at the start of the file: lowering errors
    /// carry no span) appended to its diagnostics: the shape
    /// `typeck::type_check` returns, holding every stage's diagnostic but
    /// the borrow errors.
    pub fn into_type_check_result(self) -> TypeCheckResult {
        let mut typeck = self.typeck;
        typeck.diagnostics.extend(self.bootstrap);
        if let Some(error) = self.lowering_error {
            typeck.diagnostics.push(Diagnostic::error(
                format!("MIR lowering error: {error}"),
                crate::lexer::token::Span::new(0, 0, 1, 1),
            ));
        }
        typeck
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(diagnostics: &[Diagnostic]) -> Vec<String> {
        diagnostics.iter().filter_map(|d| d.code.clone()).collect()
    }

    fn run(source: &str, options: &Options) -> Frontend {
        check(&parse(source).expect("parse"), options)
    }

    /// Top-level statements run as an implicit `main`, so the async
    /// pre-checks see them exactly as they see a `def main` body.
    #[test]
    fn script_mode_async_diagnostics_match_def_main() {
        let typeck_only = Options {
            stop_after: Stage::Typeck,
            ..Options::default()
        };
        let script = run("h = Task.spawn(0)\nputs h\n", &typeck_only);
        let in_main = run(
            "def main\n  h = Task.spawn(0)\n  puts h\nend\n",
            &typeck_only,
        );
        assert!(
            codes(script.diagnostics()).contains(&"E1116".to_string()),
            "{}",
            script.render(None)
        );
        assert_eq!(codes(script.diagnostics()), codes(in_main.diagnostics()));
    }

    /// A MIR loan error is a borrow error, reported once, and leaves
    /// nothing to codegen.
    #[test]
    fn loan_error_is_reported_once_and_stops_codegen() {
        let source = "class U\n  name: String\n  def init(@name: String)\n  end\n\n  \
                      def label -> &String\n    @name\n  end\nend\n\n\
                      u = U.new(\"old\")\nx = u.label\nu.name = \"n\"\nputs x\n";
        let frontend = run(source, &Options::default());
        let codes: Vec<&str> = frontend
            .borrow_errors
            .iter()
            .map(|e| e.code.code_str())
            .collect();
        assert_eq!(codes, ["E1009"], "{}", frontend.render(None));
        assert!(frontend.mir.is_none());
    }
}
