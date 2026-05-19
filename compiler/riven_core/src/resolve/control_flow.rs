#![allow(unused_imports)]

use std::collections::HashMap;

use crate::diagnostics::Diagnostic;
use crate::hir::context::TypeContext;
use crate::hir::nodes::*;
use crate::hir::types::{MixinRef, MoveSemantics, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::{self, Visibility};

use super::const_helpers;
use super::scope::{ScopeId, ScopeKind, ScopeStack};
use super::symbols::*;
use super::{ClosureCaptureContext, ResolveResult, Resolver};

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
                    let hir_expr = self.resolve_expr(expr);
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

    pub(super) fn resolve_let(&mut self, binding: &ast::LetBinding) -> HirStatement {
        let ty = binding
            .type_annotation
            .as_ref()
            .map(|t| self.resolve_type_expr(t))
            .unwrap_or_else(|| self.type_context.fresh_type_var());

        let value = binding.value.as_ref().map(|v| self.resolve_expr(v));

        let pattern = self.resolve_pattern_with_type(&binding.pattern, &ty);

        // Register the binding
        let name = self.pattern_binding_name(&binding.pattern);
        let def_id = self.symbols.define(
            name,
            DefKind::Variable {
                mutable: binding.mutable,
                ty: ty.clone(),
            },
            Visibility::Private,
            binding.span.clone(),
        );

        // Insert into current scope
        if let ast::Pattern::Identifier { name, .. } = &binding.pattern {
            self.scopes.insert(name.clone(), def_id);
        } else if let ast::Pattern::Tuple { .. } = &binding.pattern {
            // For tuple destructuring, register each element
            self.register_pattern_bindings(&binding.pattern, binding.mutable, &binding.span);
        } else {
            self.register_pattern_bindings(&binding.pattern, binding.mutable, &binding.span);
        }

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
        self.register_pattern_bindings(&if_let.pattern, false, &if_let.span);
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
            self.register_pattern_bindings(&arm.pattern, false, &arm.span);
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

        let mut params = Vec::new();
        for p in &closure.params {
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
            ast::ClosureBody::Expr(e) => self.resolve_expr(e),
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
            },
            ty: fn_ty,
            span: span.clone(),
        }
    }

    pub(super) fn record_capture_if_needed(&mut self, def_id: DefId, def_scope_id: ScopeId) {
        let Some(closure) = self.closure_stack.last_mut() else {
            return;
        };
        if self.scopes.is_within_scope(def_scope_id, closure.scope_id) {
            return;
        }
        let Some(def) = self.symbols.get(def_id) else {
            return;
        };
        let should_capture = matches!(
            def.kind,
            DefKind::Variable { .. } | DefKind::Param { .. } | DefKind::SelfValue { .. }
        );
        if !should_capture || closure.captures.iter().any(|cap| cap.def_id == def_id) {
            return;
        }
        closure.captures.push(Capture {
            def_id,
            name: def.name.clone(),
            by_move: closure.is_move,
            ty: self.symbols.def_ty(def_id).unwrap_or(Ty::Error),
        });
    }

    // ─── Pattern Resolution ─────────────────────────────────────────

}
