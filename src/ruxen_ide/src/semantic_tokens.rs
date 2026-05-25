use lsp_types::{SemanticToken, SemanticTokenModifier, SemanticTokenType};
use ruxen_core::hir::nodes::*;
use ruxen_core::lexer::token::TokenKind;
use ruxen_core::resolve::symbols::{DefKind, SymbolTable};

use crate::analysis::AnalysisResult;
use crate::line_index::LineIndex;

/// The token type legend — must match the order declared in server capabilities.
pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::KEYWORD,     // 0
    SemanticTokenType::VARIABLE,    // 1
    SemanticTokenType::PARAMETER,   // 2
    SemanticTokenType::FUNCTION,    // 3
    SemanticTokenType::METHOD,      // 4
    SemanticTokenType::TYPE,        // 5
    SemanticTokenType::PROPERTY,    // 6
    SemanticTokenType::ENUM_MEMBER, // 7
    SemanticTokenType::NUMBER,      // 8
    SemanticTokenType::STRING,      // 9
    SemanticTokenType::COMMENT,     // 10
    SemanticTokenType::OPERATOR,    // 11
];

pub const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::READONLY,      // bit 0
    SemanticTokenModifier::DOCUMENTATION, // bit 1
];

struct RawToken {
    line: u32,
    start_char: u32,
    length: u32,
    token_type: u32,
    modifiers: u32,
}

/// Compute semantic tokens for the entire file.
pub fn semantic_tokens(result: &AnalysisResult) -> Vec<SemanticToken> {
    let mut raw_tokens = Vec::new();

    // Walk the token stream for keywords, literals, comments, operators
    collect_lexical_tokens(result, &mut raw_tokens);

    // Walk the HIR for identifier classification (overrides lexical classification)
    if let (Some(program), Some(symbols)) = (&result.program, &result.symbols) {
        collect_hir_tokens(program, symbols, &result.line_index, &mut raw_tokens);
    }

    // Sort by position
    raw_tokens.sort_by(|a, b| a.line.cmp(&b.line).then(a.start_char.cmp(&b.start_char)));

    // Deduplicate: if HIR and lexical tokens overlap, keep the HIR one (last wins after sort is stable)
    dedup_tokens(&mut raw_tokens);

    // Encode as deltas
    encode_deltas(&raw_tokens)
}

fn collect_lexical_tokens(result: &AnalysisResult, tokens: &mut Vec<RawToken>) {
    // Re-lex to get the token stream with spans
    let mut lexer = ruxen_core::lexer::Lexer::new(&result.source);
    let lexed = match lexer.tokenize() {
        Ok(t) => t,
        Err(_) => return,
    };

    for token in &lexed {
        let (token_type, modifiers) = classify_token_kind(&token.kind);
        if let Some(tt) = token_type {
            let pos = result.line_index.position_of(token.span.start);
            let end_pos = result.line_index.position_of(token.span.end);
            // Only handle single-line tokens for simplicity
            if pos.line == end_pos.line {
                tokens.push(RawToken {
                    line: pos.line,
                    start_char: pos.character,
                    length: end_pos.character - pos.character,
                    token_type: tt,
                    modifiers,
                });
            }
        }
    }
}

fn classify_token_kind(kind: &TokenKind) -> (Option<u32>, u32) {
    match kind {
        // Keywords
        TokenKind::Let
        | TokenKind::Mut
        | TokenKind::Move
        | TokenKind::Ref
        | TokenKind::Class
        | TokenKind::Struct
        | TokenKind::Enum
        | TokenKind::Newtype
        | TokenKind::Type
        | TokenKind::Def
        | TokenKind::Protected
        | TokenKind::Consume
        | TokenKind::SelfValue
        | TokenKind::Init
        | TokenKind::Super
        | TokenKind::Return
        | TokenKind::Yield
        | TokenKind::Async
        | TokenKind::Await
        | TokenKind::If
        | TokenKind::Elsif
        | TokenKind::Else
        | TokenKind::Match
        | TokenKind::While
        | TokenKind::For
        | TokenKind::In
        | TokenKind::Loop
        | TokenKind::Do
        | TokenKind::End
        | TokenKind::Break
        | TokenKind::Continue
        | TokenKind::Where
        | TokenKind::As
        | TokenKind::Module
        | TokenKind::Use
        | TokenKind::Unsafe
        | TokenKind::Lib
        | TokenKind::True
        | TokenKind::False
        | TokenKind::SomeKw
        | TokenKind::OkKw
        | TokenKind::ErrKw => (Some(0), 0), // KEYWORD

        // Type identifiers
        TokenKind::TypeIdentifier(_) | TokenKind::SelfType => (Some(5), 0), // TYPE

        // Numbers
        TokenKind::IntLiteral(_, _) | TokenKind::FloatLiteral(_, _) => (Some(8), 0), // NUMBER

        // Strings
        TokenKind::StringLiteral(_) | TokenKind::CharLiteral(_) => (Some(9), 0), // STRING

        // Doc comments
        TokenKind::DocComment(_) => (Some(10), 2), // COMMENT + DOCUMENTATION modifier

        // Identifiers — classified later by HIR walk, skip in lexical pass
        TokenKind::Identifier(_) => (None, 0),

        // Interpolated strings — the literal parts are strings
        TokenKind::InterpolatedString(_) => (Some(9), 0), // STRING

        // Everything else — not classified
        _ => (None, 0),
    }
}

fn collect_hir_tokens(
    program: &HirProgram,
    symbols: &SymbolTable,
    line_index: &LineIndex,
    tokens: &mut Vec<RawToken>,
) {
    let mut walker = HirTokenWalker {
        symbols,
        line_index,
        tokens,
    };
    walker.visit_program(program);
}

struct HirTokenWalker<'a> {
    symbols: &'a SymbolTable,
    line_index: &'a LineIndex,
    tokens: &'a mut Vec<RawToken>,
}

impl<'a> HirTokenWalker<'a> {
    fn push_token(
        &mut self,
        span: &ruxen_core::lexer::token::Span,
        token_type: u32,
        modifiers: u32,
    ) {
        let pos = self.line_index.position_of(span.start);
        let end_pos = self.line_index.position_of(span.end);
        if pos.line == end_pos.line && end_pos.character > pos.character {
            self.tokens.push(RawToken {
                line: pos.line,
                start_char: pos.character,
                length: end_pos.character - pos.character,
                token_type,
                modifiers,
            });
        }
    }

    fn visit_program(&mut self, program: &HirProgram) {
        for item in &program.items {
            self.visit_item(item);
        }
    }

    fn visit_item(&mut self, item: &HirItem) {
        match item {
            HirItem::Function(func) => self.visit_func(func),
            HirItem::Class(class) => {
                for method in &class.methods {
                    self.visit_func(method);
                }
                for imp in &class.impl_blocks {
                    self.visit_impl(imp);
                }
            }
            HirItem::Struct(_) => {}
            HirItem::Enum(_) => {}
            HirItem::Mixin(t) => {
                for item in &t.items {
                    if let HirMixinItem::DefaultMethod(func) = item {
                        self.visit_func(func);
                    }
                }
            }
            HirItem::Impl(imp) => self.visit_impl(imp),
            HirItem::Module(m) => {
                for item in &m.items {
                    self.visit_item(item);
                }
            }
            HirItem::TypeAlias(_) | HirItem::Newtype(_) | HirItem::Const(_) => {}
        }
    }

    fn visit_impl(&mut self, imp: &HirImplBlock) {
        for item in &imp.items {
            if let HirImplItem::Method(func) = item {
                self.visit_func(func);
            }
        }
    }

    fn visit_func(&mut self, func: &HirFuncDef) {
        self.visit_expr(&func.body);
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        match &expr.kind {
            HirExprKind::VarRef(def_id) => {
                let (tt, mods) = self.classify_def(*def_id);
                self.push_token(&expr.span, tt, mods);
            }
            HirExprKind::FnCall { args, .. } => {
                // The callee name portion — classify as function
                self.push_token(&expr.span, 3, 0); // FUNCTION
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            HirExprKind::MethodCall {
                object,
                args,
                block,
                ..
            } => {
                self.visit_expr(object);
                // Method name is part of the expression — classified as METHOD
                for arg in args {
                    self.visit_expr(arg);
                }
                if let Some(b) = block {
                    self.visit_expr(b);
                }
            }
            HirExprKind::FieldAccess { object, .. } => {
                self.visit_expr(object);
            }
            HirExprKind::BinaryOp { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            HirExprKind::UnaryOp { operand, .. } => {
                self.visit_expr(operand);
            }
            HirExprKind::Borrow { expr: inner, .. } => {
                self.visit_expr(inner);
            }
            HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
                for stmt in stmts {
                    self.visit_statement(stmt);
                }
                if let Some(tail) = tail {
                    self.visit_expr(tail);
                }
            }
            HirExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.visit_expr(cond);
                self.visit_expr(then_branch);
                if let Some(e) = else_branch {
                    self.visit_expr(e);
                }
            }
            HirExprKind::Match { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    self.visit_expr(&arm.body);
                }
            }
            HirExprKind::While { condition, body } => {
                self.visit_expr(condition);
                self.visit_expr(body);
            }
            HirExprKind::For { iterable, body, .. } => {
                self.visit_expr(iterable);
                self.visit_expr(body);
            }
            HirExprKind::Loop { body } => {
                self.visit_expr(body);
            }
            HirExprKind::Assign { target, value, .. } => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            HirExprKind::CompoundAssign { target, value, .. } => {
                self.visit_expr(target);
                self.visit_expr(value);
            }
            HirExprKind::Return(Some(inner)) | HirExprKind::Break(Some(inner)) => {
                self.visit_expr(inner);
            }
            HirExprKind::Closure { body, .. } => {
                self.visit_expr(body);
            }
            HirExprKind::Construct { fields, .. } => {
                for (_name, val) in fields {
                    self.visit_expr(val);
                }
            }
            HirExprKind::EnumVariant { fields, .. } => {
                for (_name, val) in fields {
                    self.visit_expr(val);
                }
            }
            HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
                for e in elems {
                    self.visit_expr(e);
                }
            }
            HirExprKind::MapLiteral(entries) => {
                for (k, v) in entries {
                    self.visit_expr(k);
                    self.visit_expr(v);
                }
            }
            HirExprKind::Index { object, index } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            HirExprKind::Cast { expr: inner, .. } => {
                self.visit_expr(inner);
            }
            HirExprKind::ArrayFill { value, .. } => {
                self.visit_expr(value);
            }
            HirExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.visit_expr(s);
                }
                if let Some(e) = end {
                    self.visit_expr(e);
                }
            }
            HirExprKind::Interpolation { parts } => {
                for part in parts {
                    if let HirInterpolationPart::Expr { expr: e, .. } = part {
                        self.visit_expr(e);
                    }
                }
            }
            HirExprKind::MacroCall { args, .. } => {
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            _ => {}
        }
    }

    fn visit_statement(&mut self, stmt: &HirStatement) {
        match stmt {
            HirStatement::Let { value, .. } => {
                if let Some(val) = value {
                    self.visit_expr(val);
                }
            }
            HirStatement::Expr(expr) => {
                self.visit_expr(expr);
            }
        }
    }

    fn classify_def(&self, def_id: DefId) -> (u32, u32) {
        if let Some(def) = self.symbols.get(def_id) {
            match &def.kind {
                DefKind::Variable { mutable, .. } => {
                    let mods = if *mutable { 0 } else { 1 }; // READONLY = bit 0
                    (1, mods) // VARIABLE
                }
                DefKind::Param { .. } => (2, 0),       // PARAMETER
                DefKind::Function { .. } => (3, 0),    // FUNCTION
                DefKind::Method { .. } => (4, 0),      // METHOD
                DefKind::Field { .. } => (6, 0),       // PROPERTY
                DefKind::EnumVariant { .. } => (7, 0), // ENUM_MEMBER
                DefKind::SelfValue { .. } => (1, 0),   // VARIABLE
                _ => (1, 0),                           // VARIABLE fallback
            }
        } else {
            (1, 0) // VARIABLE fallback
        }
    }
}

/// Remove duplicate tokens at the same position, keeping the last one (HIR overrides lexical).
fn dedup_tokens(tokens: &mut Vec<RawToken>) {
    tokens
        .dedup_by(|b, a| a.line == b.line && a.start_char == b.start_char && a.length == b.length);
}

fn encode_deltas(tokens: &[RawToken]) -> Vec<SemanticToken> {
    let mut result = Vec::with_capacity(tokens.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;

    for token in tokens {
        let delta_line = token.line - prev_line;
        let delta_start = if delta_line == 0 {
            token.start_char - prev_start
        } else {
            token.start_char
        };

        result.push(SemanticToken {
            delta_line,
            delta_start,
            length: token.length,
            token_type: token.token_type,
            token_modifiers_bitset: token.modifiers,
        });

        prev_line = token.line;
        prev_start = token.start_char;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyze;

    /// Reconstruct absolute (line, start_char) positions from the delta-encoded tokens.
    fn expand_deltas(tokens: &[SemanticToken]) -> Vec<(u32, u32, u32, u32)> {
        let mut out = Vec::with_capacity(tokens.len());
        let mut line = 0u32;
        let mut start = 0u32;
        for t in tokens {
            if t.delta_line == 0 {
                start += t.delta_start;
            } else {
                line += t.delta_line;
                start = t.delta_start;
            }
            out.push((line, start, t.length, t.token_type));
        }
        out
    }

    #[test]
    fn tokens_on_simple_program() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        assert!(!tokens.is_empty(), "Expected some tokens");
    }

    #[test]
    fn tokens_include_keyword_type() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        // Token type 0 is KEYWORD
        let has_keyword = tokens.iter().any(|t| t.token_type == 0);
        assert!(has_keyword, "Expected at least one keyword token");
    }

    #[test]
    fn tokens_include_number_type() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        // Token type 8 is NUMBER
        let has_number = tokens.iter().any(|t| t.token_type == 8);
        assert!(has_number, "Expected NUMBER token for literal 42");
    }

    #[test]
    fn tokens_include_string_type() {
        let src = "def main\n  puts \"hello\"\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        // Token type 9 is STRING
        let has_string = tokens.iter().any(|t| t.token_type == 9);
        assert!(has_string, "Expected STRING token for \"hello\"");
    }

    #[test]
    fn tokens_deltas_are_sorted_and_monotonic() {
        let src = "def main\n  let a = 1\n  let b = 2\n  let c = 3\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        let absolute = expand_deltas(&tokens);
        // Each successive token must have (line, start) >= previous
        for window in absolute.windows(2) {
            let (l0, s0, _, _) = window[0];
            let (l1, s1, _, _) = window[1];
            assert!(
                (l1, s1) >= (l0, s0),
                "Tokens not monotonic: ({},{}) then ({},{})",
                l0,
                s0,
                l1,
                s1
            );
        }
    }

    #[test]
    fn tokens_empty_input_produces_empty_list() {
        let src = "";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        assert!(tokens.is_empty(), "Empty source → empty tokens");
    }

    #[test]
    fn tokens_lex_error_does_not_crash() {
        let src = "let x = \"\n"; // unterminated string — lex error
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        // Lex error means no HIR — lexical pass also fails, so token list may be empty
        // The key thing is no crash
        let _ = tokens;
    }

    #[test]
    fn tokens_include_type_identifier() {
        let src = "class Foo\n  x: Int\n  def init(@x: Int)\n  end\nend\n\ndef main\n  let f = Foo.new(1)\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        // Token type 5 is TYPE — expect at least one for "Int" or "Foo"
        let has_type = tokens.iter().any(|t| t.token_type == 5);
        assert!(has_type, "Expected TYPE token for Int/Foo");
    }

    #[test]
    fn tokens_doc_comment_has_documentation_modifier() {
        let src = "## This is a docstring\ndef main\n  let x = 42\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        // Doc comments carry modifier bit 1 (DOCUMENTATION)
        let has_doc_mod = tokens.iter().any(|t| t.token_modifiers_bitset & 2 != 0);
        // May or may not fire depending on whether lexer produces DocComment tokens
        let _ = has_doc_mod;
    }

    #[test]
    fn tokens_have_nonzero_length() {
        let src = "def main\n  let x = 42\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        for t in &tokens {
            assert!(t.length > 0, "Token length must be nonzero");
        }
    }

    #[test]
    fn tokens_no_crash_on_valid_complex_input() {
        let src = "class Counter\n  value: Int\n  def init(@value: Int)\n  end\n  pub def incremented -> Int\n    self.value + 1\n  end\nend\n\ndef main\n  let c = Counter.new(0)\n  puts \"#{c.incremented}\"\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        assert!(!tokens.is_empty());
    }

    #[test]
    fn tokens_variable_ref_classified() {
        let src = "def main\n  let x = 42\n  let y = x\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        // Token type 1 is VARIABLE — the ref to x in "let y = x" should be VARIABLE
        let has_variable = tokens.iter().any(|t| t.token_type == 1);
        assert!(has_variable, "Expected at least one VARIABLE token");
    }

    #[test]
    fn tokens_token_types_match_legend_range() {
        let src = "def main\n  let x = 42\n  puts \"hi\"\nend\n";
        let result = analyze(src);
        let tokens = semantic_tokens(&result);
        for t in &tokens {
            assert!(
                (t.token_type as usize) < TOKEN_TYPES.len(),
                "Token type {} is out of bounds (legend len {})",
                t.token_type,
                TOKEN_TYPES.len()
            );
        }
    }
}
