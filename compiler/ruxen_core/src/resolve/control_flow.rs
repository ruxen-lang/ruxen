use crate::diagnostics::Diagnostic;
use crate::hir::nodes::*;
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::{self, Visibility};

use super::scope::{ScopeId, ScopeKind};
use super::symbols::*;
use super::{ClosureCaptureContext, Resolver};

impl Resolver {
    pub(super) fn resolve_block_as_expr(&mut self, block: &ast::Block) -> HirExpr {
        self.scopes.push(ScopeKind::Block);

        let mut stmts = Vec::new();
        let mut tail_expr = None;

        for (i, stmt) in block.statements.iter().enumerate() {
            let is_last = i == block.statements.len() - 1;
            match stmt {
                ast::Statement::Let(binding) => {
                    stmts.push(self.resolve_let(binding));
                }
                ast::Statement::Expression(expr) => {
                    if let Some(binding) = self.resolve_bare_assignment_binding(expr) {
                        stmts.push(binding);
                        continue;
                    }
                    // `x = a and b`: Ruby binds `=` tighter than the keyword
                    // operators, so the leading assignment declares `x` like
                    // a statement of its own, and the rest reads `x`.
                    let split = split_leading_assignment(expr);
                    let binding = split
                        .as_ref()
                        .and_then(|(assign, _)| self.resolve_bare_assignment_binding(assign));
                    let expr = match (binding, split) {
                        (Some(binding), Some((_, rest))) => {
                            stmts.push(binding);
                            std::borrow::Cow::Owned(rest)
                        }
                        _ => std::borrow::Cow::Borrowed(expr),
                    };
                    let hir_expr = self.resolve_expr(&expr);
                    if is_last {
                        // Last expression in block is the tail (implicit return)
                        tail_expr = Some(Box::new(hir_expr));
                    } else {
                        stmts.push(HirStatement::Expr(hir_expr));
                    }
                }
            }
        }

        self.scopes.pop();

        let ty = tail_expr.as_ref().map(|e| e.ty.clone()).unwrap_or(Ty::Unit);

        HirExpr {
            kind: HirExprKind::Block(stmts, tail_expr),
            ty,
            span: block.span.clone(),
        }
    }

    fn resolve_bare_assignment_binding(&mut self, expression: &ast::Expr) -> Option<HirStatement> {
        let ast::ExprKind::Assign { target, value } = &expression.kind else {
            return None;
        };
        let ast::ExprKind::Identifier(name) = &target.kind else {
            return None;
        };
        // Ruby: an assignment names an existing local (or parameter) when
        // one is in scope; otherwise it declares a new local, even when a
        // method of that name exists (`p = "x"` shadows `Kernel#p`).
        if let Some(existing) = self.scopes.lookup(name) {
            let names_method = self.symbols.get(existing).is_some_and(|def| {
                matches!(
                    def.kind,
                    DefKind::Function { .. } | DefKind::OverloadSet { .. } | DefKind::Method { .. }
                )
            });
            if !names_method {
                return None;
            }
        }

        let binding = ast::LetBinding {
            mutable: true,
            pattern: ast::Pattern::Identifier {
                name: name.clone(),
                mutable: true,
                span: target.span.clone(),
            },
            type_annotation: None,
            value: Some(value.clone()),
            span: expression.span.clone(),
        };
        Some(self.resolve_let(&binding))
    }

    pub(super) fn resolve_let(&mut self, binding: &ast::LetBinding) -> HirStatement {
        let ty = binding
            .type_annotation
            .as_ref()
            .map(|t| self.resolve_type_expr(t))
            .unwrap_or_else(|| self.type_context.fresh_type_var());

        let value = binding.value.as_ref().map(|v| self.resolve_expr(v));

        let pattern = self.resolve_let_pattern(&binding.pattern, &ty, binding.mutable);

        // A simple binding's def is the let's def. Any other pattern binds
        // its names above and gets an unscoped def for the whole value.
        let def_id = match &pattern {
            HirPattern::Binding { def_id, .. } | HirPattern::Ref { def_id, .. } => *def_id,
            _ => self.symbols.define(
                self.pattern_binding_name(&binding.pattern),
                DefKind::Variable {
                    mutable: binding.mutable,
                    ty: ty.clone(),
                },
                Visibility::Private,
                binding.span.clone(),
            ),
        };

        HirStatement::Let {
            def_id,
            pattern,
            ty,
            value,
            mutable: binding.mutable,
            span: binding.span.clone(),
        }
    }

    // ─── If Expression Resolution ───────────────────────────────────

    pub(super) fn resolve_if(&mut self, if_expr: &ast::IfExpr) -> HirExpr {
        let cond = self.resolve_expr(&if_expr.condition);
        let then_branch = self.resolve_block_as_expr(&if_expr.then_body);

        // Handle elsif + else chain by nesting
        let else_branch = if !if_expr.elsif_clauses.is_empty() {
            // Build nested if-else from elsif chain
            let mut else_expr = if_expr
                .else_body
                .as_ref()
                .map(|b| self.resolve_block_as_expr(b));

            for elsif in if_expr.elsif_clauses.iter().rev() {
                let elsif_cond = self.resolve_expr(&elsif.condition);
                let elsif_body = self.resolve_block_as_expr(&elsif.body);
                let ty = self.type_context.fresh_type_var();
                else_expr = Some(HirExpr {
                    kind: HirExprKind::If {
                        cond: Box::new(elsif_cond),
                        then_branch: Box::new(elsif_body),
                        else_branch: else_expr.map(Box::new),
                    },
                    ty,
                    span: elsif.span.clone(),
                });
            }
            else_expr
        } else {
            if_expr
                .else_body
                .as_ref()
                .map(|b| self.resolve_block_as_expr(b))
        };

        let ty = self.type_context.fresh_type_var();
        HirExpr {
            kind: HirExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch: else_branch.map(Box::new),
            },
            ty,
            span: if_expr.span.clone(),
        }
    }

    pub(super) fn resolve_if_let(&mut self, if_let: &ast::IfLetExpr) -> HirExpr {
        let value = self.resolve_expr(&if_let.value);

        self.scopes.push(ScopeKind::Block);
        let pattern = self.resolve_pattern(&if_let.pattern);
        let then_body = self.resolve_block_as_expr(&if_let.then_body);
        self.scopes.pop();

        let else_body = if_let
            .else_body
            .as_ref()
            .map(|b| self.resolve_block_as_expr(b));

        // Desugar to match
        let wildcard_arm = HirMatchArm {
            pattern: HirPattern::Wildcard {
                span: if_let.span.clone(),
            },
            guard: None,
            body: Box::new(else_body.unwrap_or(HirExpr {
                kind: HirExprKind::UnitLiteral,
                ty: Ty::Unit,
                span: if_let.span.clone(),
            })),
            span: if_let.span.clone(),
        };

        let ty = self.type_context.fresh_type_var();
        HirExpr {
            kind: HirExprKind::Match {
                scrutinee: Box::new(value),
                arms: vec![
                    HirMatchArm {
                        pattern,
                        guard: None,
                        body: Box::new(then_body),
                        span: if_let.span.clone(),
                    },
                    wildcard_arm,
                ],
            },
            ty,
            span: if_let.span.clone(),
        }
    }

    // ─── Match Expression Resolution ────────────────────────────────

    pub(super) fn resolve_match(&mut self, match_expr: &ast::MatchExpr) -> HirExpr {
        let scrutinee = self.resolve_expr(&match_expr.subject);

        let mut arms = Vec::new();
        for arm in &match_expr.arms {
            self.scopes.push(ScopeKind::Match);
            let pattern = self.resolve_pattern(&arm.pattern);
            let guard = arm.guard.as_ref().map(|g| Box::new(self.resolve_expr(g)));
            let body = match &arm.body {
                ast::MatchArmBody::Expr(e) => self.resolve_expr(e),
                ast::MatchArmBody::Block(b) => self.resolve_block_as_expr(b),
            };
            self.scopes.pop();
            arms.push(HirMatchArm {
                pattern,
                guard,
                body: Box::new(body),
                span: arm.span.clone(),
            });
        }

        let ty = self.type_context.fresh_type_var();
        HirExpr {
            kind: HirExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            ty,
            span: match_expr.span.clone(),
        }
    }

    // ─── Closure Resolution ─────────────────────────────────────────

    pub(super) fn resolve_closure(&mut self, closure: &ast::ClosureExpr, span: &Span) -> HirExpr {
        let closure_scope_id = self.scopes.push(ScopeKind::Closure);
        self.closure_stack.push(ClosureCaptureContext {
            scope_id: closure_scope_id,
            is_move: closure.is_move,
            captures: Vec::new(),
        });

        // Block Tier-2: a paramless closure that references `_1`..`_9` or the
        // bare `it` gets those implicit parameters synthesized here (surface
        // preserved for `ruxen fmt`; see `implicit_block_params`). An explicit
        // `|params|` header takes precedence and suppresses the scan.
        let implicit_names = if closure.params.is_empty() {
            super::implicit_block_params::implicit_param_names(&closure.body)
        } else {
            Vec::new()
        };

        let mut params = Vec::new();
        let synth_params: Vec<ast::ClosureParam> = implicit_names
            .into_iter()
            .map(|name| ast::ClosureParam {
                name,
                type_expr: None,
                span: span.clone(),
            })
            .collect();
        let param_iter = closure.params.iter().chain(synth_params.iter());
        for p in param_iter {
            let ty = p
                .type_expr
                .as_ref()
                .map(|t| self.resolve_type_expr(t))
                .unwrap_or_else(|| self.type_context.fresh_type_var());
            let def_id = self.symbols.define(
                p.name.clone(),
                DefKind::Param {
                    ty: ty.clone(),
                    auto_assign: false,
                },
                Visibility::Private,
                p.span.clone(),
            );
            self.scopes.insert(p.name.clone(), def_id);
            params.push(HirClosureParam {
                def_id,
                name: p.name.clone(),
                ty,
                span: p.span.clone(),
            });
        }

        let old_async_scope_depth = self.async_scope_depth;
        if closure.is_async {
            self.async_scope_depth += 1;
        }

        let body = match &closure.body {
            // A one-expression body that assigns a new name declares a
            // local, as the same statement in a `do ... end` body does.
            ast::ClosureBody::Expr(e) => match self.resolve_bare_assignment_binding(e) {
                Some(binding) => HirExpr {
                    kind: HirExprKind::Block(vec![binding], None),
                    ty: Ty::Unit,
                    span: e.span.clone(),
                },
                None => self.resolve_expr(e),
            },
            ast::ClosureBody::Block(b) => self.resolve_block_as_expr(b),
        };

        self.async_scope_depth = old_async_scope_depth;
        let captures = self
            .closure_stack
            .pop()
            .map(|ctx| ctx.captures)
            .unwrap_or_default();
        self.scopes.pop();

        let param_tys: Vec<Ty> = params.iter().map(|p| p.ty.clone()).collect();
        let ret_ty = if closure.is_async {
            Ty::Class {
                name: "Future".to_string(),
                generic_args: vec![body.ty.clone()],
            }
        } else {
            body.ty.clone()
        };
        let fn_ty = Ty::Fn {
            params: param_tys,
            ret: Box::new(ret_ty),
        };

        HirExpr {
            kind: HirExprKind::Closure {
                params,
                body: Box::new(body),
                captures,
                is_async: closure.is_async,
                is_move: closure.is_move,
                kind: closure.kind,
            },
            ty: fn_ty,
            span: span.clone(),
        }
    }

    /// Desugar a `&`-form block argument (Block Tier-2) into the HIR that fills
    /// the call's block slot.
    ///   - `&:sym`  → the closure `{ |__proc_arg| __proc_arg.sym }`.
    ///   - `&block` / `&` → a reference to the enclosing function's block slot.
    pub(super) fn resolve_block_pass(&mut self, bp: &ast::BlockPassKind, span: &Span) -> HirExpr {
        match bp {
            ast::BlockPassKind::ToProc(method_name) => {
                // Build `{ |__proc_arg| __proc_arg.<method_name> }` and resolve
                // it through the normal closure path so capture / typing behave
                // identically to a hand-written block.
                let arg_name = "__proc_arg".to_string();
                let receiver = ast::Expr {
                    kind: ast::ExprKind::Identifier(arg_name.clone()),
                    span: span.clone(),
                };
                let call = ast::Expr {
                    kind: ast::ExprKind::MethodCall {
                        parens: true,
                        object: Box::new(receiver),
                        method: method_name.clone(),
                        generic_args: Vec::new(),
                        args: Vec::new(),
                        block: None,
                    },
                    span: span.clone(),
                };
                let closure = ast::ClosureExpr {
                    is_async: false,
                    is_move: false,
                    params: vec![ast::ClosureParam {
                        name: arg_name,
                        type_expr: None,
                        span: span.clone(),
                    }],
                    body: ast::ClosureBody::Expr(Box::new(call)),
                    span: span.clone(),
                    pipes: true,
                    do_end: false,
                    kind: ast::ClosureKind::Block,
                    spelling: ast::ClosureSpelling::Bare,
                };
                self.resolve_closure(&closure, span)
            }
            ast::BlockPassKind::Forward => self.resolve_block_forward(span),
        }
    }

    /// `&block` / bare `&` — forward the enclosing function's received block.
    /// The block slot is bound under `__block` (implicit-yield fns) and
    /// `&block` (explicit `&block:` params); a reference to it is a plain
    /// `VarRef` carrying the block's `Ty::Fn`, which the call site then passes
    /// as its own trailing block argument. No enclosing block → E1120.
    fn resolve_block_forward(&mut self, span: &Span) -> HirExpr {
        let block_def = self
            .scopes
            .lookup("__block")
            .or_else(|| self.scopes.lookup("&block"));
        if let Some(def) = block_def {
            let ty = self.symbols.def_ty(def).unwrap_or(Ty::Error);
            HirExpr {
                kind: HirExprKind::VarRef(def),
                ty,
                span: span.clone(),
            }
        } else {
            self.diagnostics.push(Diagnostic::error_with_code(
                "`&block` / `&` forwards the enclosing function's block, but this \
                 function has no block parameter to forward",
                span.clone(),
                "E1124",
            ));
            HirExpr {
                kind: HirExprKind::Error,
                ty: Ty::Error,
                span: span.clone(),
            }
        }
    }

    /// Record `def_id` as a capture of every enclosing closure it is defined
    /// outside of, innermost first. A closure nested in another reads the
    /// outer local through the enclosing closure's captures, so the enclosing
    /// closure captures it too: an owning lambda around a block that reads `s`
    /// owns (moves) `s`, and the borrow checker sees that move.
    pub(super) fn record_capture_if_needed(&mut self, def_id: DefId, def_scope_id: ScopeId) {
        let Some(def) = self.symbols.get(def_id) else {
            return;
        };
        let should_capture = matches!(
            def.kind,
            DefKind::Variable { .. } | DefKind::Param { .. } | DefKind::SelfValue { .. }
        );
        // A global is no closure's capture: the builtin type constructors
        // (`Array`, `Hash`, ...) are value-scope `Variable`s of the global
        // scope, and `Array.new` in a lambda must not capture (or move) them.
        if !should_capture || self.scopes.is_global(def_scope_id) {
            return;
        }
        let name = def.name.clone();
        let ty = self.symbols.def_ty(def_id).unwrap_or(Ty::Error);
        for closure in self.closure_stack.iter_mut().rev() {
            if self.scopes.is_within_scope(def_scope_id, closure.scope_id) {
                // Defined inside this closure, so inside every enclosing one.
                return;
            }
            if !closure.captures.iter().any(|cap| cap.def_id == def_id) {
                closure.captures.push(Capture {
                    def_id,
                    name: name.clone(),
                    by_move: closure.is_move,
                    ty: ty.clone(),
                });
            }
        }
    }

    // ─── Pattern Resolution ─────────────────────────────────────────
}

/// For a statement `x = a and b` / `x = a or b` (the assignment is the
/// leftmost operand of a chain of keyword `and` / `or`, which bind looser
/// than `=`), the assignment `x = a` and the rest of the statement with
/// the assignment replaced by a read of `x`: `x and b`.
fn split_leading_assignment(expr: &ast::Expr) -> Option<(ast::Expr, ast::Expr)> {
    let ast::ExprKind::BinaryOp { left, op, right } = &expr.kind else {
        return None;
    };
    if !matches!(op, ast::BinOp::KeywordAnd | ast::BinOp::KeywordOr) {
        return None;
    }
    let (assign, rest_left) = match &left.kind {
        ast::ExprKind::Assign { target, .. }
            if matches!(target.kind, ast::ExprKind::Identifier(_)) =>
        {
            (left.as_ref().clone(), target.as_ref().clone())
        }
        _ => split_leading_assignment(left)?,
    };
    let rest = ast::Expr {
        kind: ast::ExprKind::BinaryOp {
            left: Box::new(rest_left),
            op: *op,
            right: right.clone(),
        },
        span: expr.span.clone(),
    };
    Some((assign, rest))
}
