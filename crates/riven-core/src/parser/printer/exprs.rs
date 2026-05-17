//! Pretty-printing for blocks, statements, and expressions.
//!
//! Expressions are shown in tree form for control flow (if/match/while/for/
//! loop/block/closure) and in abbreviated one-line form for leaves.

use super::super::ast::*;
use super::format::*;
use super::PrettyPrinter;

impl PrettyPrinter {
    // ── block & statements ──────────────────────────────────────────

    pub(super) fn print_block(&mut self, block: &Block) {
        for stmt in &block.statements {
            self.print_statement(stmt);
        }
    }

    fn print_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Let(binding) => {
                let mutability = if binding.mutable { "var " } else { "" };
                let pat = format_pattern(&binding.pattern);
                let ty = binding
                    .type_annotation
                    .as_ref()
                    .map(|t| format!(": {}", format_type(t)))
                    .unwrap_or_default();
                let val = binding
                    .value
                    .as_ref()
                    .map(|v| format!(" = {}", format_expr_short(v)))
                    .unwrap_or_default();
                self.line(&format!("let {}{}{}{}", mutability, pat, ty, val));
            }
            Statement::Expression(expr) => {
                self.print_expr(expr);
            }
        }
    }

    // ── expression (tree form for control flow, short for leaves) ──

    fn print_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::If(if_expr) => {
                self.line(&format!("if {}", format_expr_short(&if_expr.condition)));
                self.indent();
                self.print_block(&if_expr.then_body);
                self.dedent();
                for elsif in &if_expr.elsif_clauses {
                    self.line(&format!("elsif {}", format_expr_short(&elsif.condition)));
                    self.indent();
                    self.print_block(&elsif.body);
                    self.dedent();
                }
                if let Some(else_body) = &if_expr.else_body {
                    self.line("else");
                    self.indent();
                    self.print_block(else_body);
                    self.dedent();
                }
            }
            ExprKind::IfLet(if_let) => {
                self.line(&format!(
                    "if let {} = {}",
                    format_pattern(&if_let.pattern),
                    format_expr_short(&if_let.value)
                ));
                self.indent();
                self.print_block(&if_let.then_body);
                self.dedent();
                if let Some(else_body) = &if_let.else_body {
                    self.line("else");
                    self.indent();
                    self.print_block(else_body);
                    self.dedent();
                }
            }
            ExprKind::Match(match_expr) => {
                self.line(&format!("match {}", format_expr_short(&match_expr.subject)));
                self.indent();
                for arm in &match_expr.arms {
                    let guard = arm
                        .guard
                        .as_ref()
                        .map(|g| format!(" if {}", format_expr_short(g)))
                        .unwrap_or_default();
                    self.line(&format!("{}{} =>", format_pattern(&arm.pattern), guard));
                    self.indent();
                    match &arm.body {
                        MatchArmBody::Expr(e) => {
                            self.line(&format_expr_short(e));
                        }
                        MatchArmBody::Block(b) => {
                            self.print_block(b);
                        }
                    }
                    self.dedent();
                }
                self.dedent();
            }
            ExprKind::While(w) => {
                self.line(&format!("while {}", format_expr_short(&w.condition)));
                self.indent();
                self.print_block(&w.body);
                self.dedent();
            }
            ExprKind::WhileLet(wl) => {
                self.line(&format!(
                    "while let {} = {}",
                    format_pattern(&wl.pattern),
                    format_expr_short(&wl.value)
                ));
                self.indent();
                self.print_block(&wl.body);
                self.dedent();
            }
            ExprKind::For(f) => {
                self.line(&format!(
                    "for {} in {}",
                    format_pattern(&f.pattern),
                    format_expr_short(&f.iterable)
                ));
                self.indent();
                self.print_block(&f.body);
                self.dedent();
            }
            ExprKind::Loop(l) => {
                self.line("loop");
                self.indent();
                self.print_block(&l.body);
                self.dedent();
            }
            ExprKind::Block(block) => {
                self.line("block");
                self.indent();
                self.print_block(block);
                self.dedent();
            }
            ExprKind::Closure(closure) => {
                let async_kw = if closure.is_async { "async " } else { "" };
                let mv = if closure.is_move { "move " } else { "" };
                let params: Vec<String> = closure
                    .params
                    .iter()
                    .map(|p| {
                        p.type_expr
                            .as_ref()
                            .map(|t| format!("{}: {}", p.name, format_type(t)))
                            .unwrap_or_else(|| p.name.clone())
                    })
                    .collect();
                self.line(&format!("{}{}|{}|", async_kw, mv, params.join(", ")));
                self.indent();
                match &closure.body {
                    ClosureBody::Expr(e) => {
                        self.line(&format_expr_short(e));
                    }
                    ClosureBody::Block(b) => {
                        self.print_block(b);
                    }
                }
                self.dedent();
            }
            // All other expressions: show abbreviated form on one line
            _ => {
                self.line(&format_expr_short(expr));
            }
        }
    }
}
