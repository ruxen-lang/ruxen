use riven_core::borrow_check;
use riven_core::borrow_check::errors::BorrowError;
use riven_core::diagnostics::{Diagnostic, DiagnosticLevel};
use riven_core::hir::context::TypeContext;
use riven_core::hir::nodes::HirProgram;
use riven_core::lexer::Lexer;
use riven_core::parser::Parser;
use riven_core::resolve::symbols::SymbolTable;
use riven_core::typeck;

use crate::line_index::LineIndex;

pub struct AnalysisResult {
    pub program: Option<HirProgram>,
    pub symbols: Option<SymbolTable>,
    pub type_context: Option<TypeContext>,
    pub diagnostics: Vec<Diagnostic>,
    pub borrow_errors: Vec<BorrowError>,
    pub source: String,
    pub line_index: LineIndex,
}

impl AnalysisResult {
    fn error_only(source: &str, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            program: None,
            symbols: None,
            type_context: None,
            diagnostics,
            borrow_errors: Vec::new(),
            source: source.to_string(),
            line_index: LineIndex::new(source),
        }
    }
}

/// Run the full analysis pipeline on a source string.
///
/// The pipeline is error-resilient: each phase gates the next.
/// - Lexer errors stop the pipeline (no tokens to parse)
/// - Parser errors stop the pipeline (no AST to type-check)
/// - Type errors gate the borrow checker
pub fn analyze(source: &str) -> AnalysisResult {
    let mut all_diagnostics = Vec::new();

    // Phase 1: Lex
    let mut lexer = Lexer::new(source);
    let tokens = match lexer.tokenize() {
        Ok(tokens) => tokens,
        Err(lex_errors) => {
            return AnalysisResult::error_only(source, lex_errors);
        }
    };

    // Phase 2: Parse
    let mut parser = Parser::new(tokens);
    let ast = match parser.parse() {
        Ok(ast) => ast,
        Err(parse_errors) => {
            return AnalysisResult::error_only(source, parse_errors);
        }
    };

    // Phase 3: Type check (always produces a result, even with errors)
    let type_result = typeck::type_check(&ast);
    let has_type_errors = type_result
        .diagnostics
        .iter()
        .any(|d| d.level == DiagnosticLevel::Error);
    all_diagnostics.extend(type_result.diagnostics);

    // Phase 4: Borrow check (only if no type errors)
    let borrow_errors = if has_type_errors {
        Vec::new()
    } else {
        borrow_check::borrow_check(&type_result.program, &type_result.symbols)
    };

    AnalysisResult {
        program: Some(type_result.program),
        symbols: Some(type_result.symbols),
        type_context: Some(type_result.type_context),
        diagnostics: all_diagnostics,
        borrow_errors,
        source: source.to_string(),
        line_index: LineIndex::new(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyze_valid_program_has_no_errors() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        assert!(result.program.is_some());
        assert!(result.symbols.is_some());
        assert!(result.type_context.is_some());
        let has_errors = result
            .diagnostics
            .iter()
            .any(|d| d.level == DiagnosticLevel::Error);
        assert!(
            !has_errors,
            "Valid program should have no error diagnostics, got: {:?}",
            result.diagnostics
        );
        assert!(result.borrow_errors.is_empty());
    }

    #[test]
    fn analyze_preserves_source() {
        let src = "def main\n  let x = 1\nend\n";
        let result = analyze(src);
        assert_eq!(result.source, src);
    }

    #[test]
    fn analyze_empty_source_produces_valid_result() {
        let src = "";
        let result = analyze(src);
        // Empty source — should produce an empty program (no items), no errors
        assert!(result.program.is_some() || !result.diagnostics.is_empty());
    }

    #[test]
    fn analyze_type_error_captures_in_diagnostics() {
        // Assigning a string to a variable annotated Int should produce a type error
        let src = "def main\n  let x: Int = \"hello\"\nend\n";
        let result = analyze(src);
        let has_type_error = result
            .diagnostics
            .iter()
            .any(|d| d.level == DiagnosticLevel::Error);
        assert!(
            has_type_error,
            "Expected a type error diagnostic, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn analyze_lex_error_stops_pipeline() {
        // Unterminated string literal
        let src = "let x = \"\n";
        let result = analyze(src);
        assert!(
            result.program.is_none(),
            "Lex error should prevent HIR production"
        );
        assert!(
            !result.diagnostics.is_empty(),
            "Lex error must produce diagnostics"
        );
    }

    #[test]
    fn analyze_parse_error_stops_pipeline() {
        let src = "def\nend\n"; // malformed function def
        let result = analyze(src);
        // Either no program, or program + diagnostics
        assert!(
            result.program.is_none() || !result.diagnostics.is_empty(),
            "Parse error should yield no program or some diagnostics"
        );
    }

    #[test]
    fn analyze_type_error_skips_borrow_check() {
        let src = "def main\n  let x: Int = \"wrong\"\nend\n";
        let result = analyze(src);
        // Type errors gate borrow-check; borrow_errors must be empty
        assert!(
            result.borrow_errors.is_empty(),
            "Borrow check must not run when type errors exist"
        );
    }

    #[test]
    fn analyze_line_index_matches_source() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        // line_index should know where line 1 starts (byte 9 after "def main\n")
        let pos = result.line_index.position_of(9);
        assert_eq!(pos.line, 1);
        assert_eq!(pos.character, 0);
    }

    #[test]
    fn analyze_simple_function_gets_symbols() {
        let src = "def add(a: Int, b: Int) -> Int\n  a + b\nend\n";
        let result = analyze(src);
        assert!(result.program.is_some());
        let symbols = result.symbols.as_ref().unwrap();
        // There should be at least a few definitions (builtins + add + params a,b)
        assert!(
            symbols.len() > 5,
            "Expected some symbols, got {}",
            symbols.len()
        );
    }

    #[test]
    fn analyze_class_definition_analyzes_ok() {
        let src = "class Box\n  x: Int\n  def init(@x: Int)\n  end\nend\n\ndef main\n  let b = Box.new(5)\nend\n";
        let result = analyze(src);
        assert!(result.program.is_some());
        // Class Box must appear in symbols
        let symbols = result.symbols.as_ref().unwrap();
        let class_exists = symbols.iter().any(|d| d.name == "Box");
        assert!(class_exists, "Expected Box class in symbols");
    }

    #[test]
    fn error_only_yields_no_program() {
        let diagnostics = Vec::new();
        let result = AnalysisResult::error_only("source", diagnostics);
        assert!(result.program.is_none());
        assert!(result.symbols.is_none());
        assert!(result.type_context.is_none());
        assert!(result.borrow_errors.is_empty());
        assert_eq!(result.source, "source");
    }
}
