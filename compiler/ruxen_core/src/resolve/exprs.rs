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
use super::{ClosureCaptureContext, PatternSpan, ResolveResult, Resolver};

impl Resolver {
    pub(super) fn resolve_expr(&mut self, expr: &ast::Expr) -> HirExpr {
        let span = expr.span.clone();
        match &expr.kind {
            ast::ExprKind::IntLiteral(val, suffix) => {
                let ty = self.int_literal_type(*suffix);
                HirExpr {
                    kind: HirExprKind::IntLiteral(*val),
                    ty,
                    span,
                }
            }
            ast::ExprKind::FloatLiteral(val, suffix) => {
                let ty = self.float_literal_type(*suffix);
                HirExpr {
                    kind: HirExprKind::FloatLiteral(*val),
                    ty,
                    span,
                }
            }
            ast::ExprKind::StringLiteral(s) => HirExpr {
                kind: HirExprKind::StringLiteral(s.clone()),
                ty: Ty::Str,
                span,
            },
            ast::ExprKind::InterpolatedString(parts) => {
                let hir_parts: Vec<HirInterpolationPart> = parts
                    .iter()
                    .map(|p| {
                        match p {
                            crate::lexer::token::StringPart::Literal(s) => {
                                HirInterpolationPart::Literal(s.clone())
                            }
                            crate::lexer::token::StringPart::Expr { tokens, spec } => {
                                // Parse the interpolation tokens as an expression.
                                // The format spec (Phase 2 #06.B) is threaded through
                                // unchanged to MIR — Phase C/D consume it.
                                let inner_expr = self.resolve_interpolation_tokens(tokens, &span);
                                HirInterpolationPart::Expr {
                                    expr: inner_expr,
                                    spec: spec.clone(),
                                }
                            }
                        }
                    })
                    .collect();
                HirExpr {
                    kind: HirExprKind::Interpolation { parts: hir_parts },
                    ty: Ty::String, // interpolated strings produce owned Strings
                    span,
                }
            }
            ast::ExprKind::CharLiteral(c) => HirExpr {
                kind: HirExprKind::CharLiteral(*c),
                ty: Ty::Char,
                span,
            },
            ast::ExprKind::RegexLiteral { pattern, flags } => HirExpr {
                kind: HirExprKind::RegexLiteral {
                    pattern: pattern.clone(),
                    flags: flags.clone(),
                },
                // Typed as `Regex` class — the std.regex bootstrap
                // declares `class Regex` so downstream method
                // resolution finds `.is_match`, `.find`, `.scan`,
                // `.replace`, `.replace_all`, `.split`, etc.
                ty: Ty::Class {
                    name: "Regex".to_string(),
                    generic_args: vec![],
                },
                span,
            },
            ast::ExprKind::BoolLiteral(b) => HirExpr {
                kind: HirExprKind::BoolLiteral(*b),
                ty: Ty::Bool,
                span,
            },
            ast::ExprKind::UnitLiteral => HirExpr {
                kind: HirExprKind::UnitLiteral,
                ty: Ty::Unit,
                span,
            },
            ast::ExprKind::Identifier(name) => {
                if let Some((def_id, def_scope_id)) = self.scopes.lookup_with_scope(name) {
                    // If the identifier resolves to an enum variant (e.g.
                    // bare `None`, `Color.Red`), lower it as an
                    // EnumVariant construction rather than a VarRef so
                    // codegen allocates and tags it correctly.
                    if let Some(def) = self.symbols.get(def_id) {
                        if let DefKind::EnumVariant {
                            parent,
                            variant_idx,
                            ..
                        } = def.kind
                        {
                            let parent_name = self
                                .symbols
                                .get(parent)
                                .map(|p| p.name.clone())
                                .unwrap_or_default();
                            return HirExpr {
                                kind: HirExprKind::EnumVariant {
                                    type_def: parent,
                                    type_name: parent_name.clone(),
                                    variant_name: name.clone(),
                                    variant_idx,
                                    fields: vec![],
                                },
                                ty: Ty::Enum {
                                    name: parent_name,
                                    generic_args: vec![],
                                },
                                span,
                            };
                        }
                    }
                    self.record_capture_if_needed(def_id, def_scope_id);
                    // Phase 2 stdlib (#06): Class/Struct identifiers
                    // imported via `use std.process.Command` (or
                    // otherwise reached through the value-scope
                    // `lookup` rather than `lookup_type`) must surface
                    // as their own `Ty::Class { name }` /
                    // `Ty::Struct { name }` so a subsequent
                    // `.new(...)` MethodCall sees a concrete receiver
                    // and dispatches through the collection-ctor fast
                    // path in mir/lower.rs. Without this branch,
                    // `def_ty` returns None for Class kinds
                    // (intentionally — see
                    // `def_ty_returns_none_for_class` in symbols.rs)
                    // and the receiver collapses to a fresh inference
                    // variable, which means `.arg/.status/etc.` would
                    // never resolve to the right `builtin_method_type`
                    // arm.
                    //
                    // Enum is intentionally NOT promoted here: enum
                    // identifiers reach this path only as the receiver
                    // for `EnumName.Variant(...)` constructor calls,
                    // which are parsed as their own AST shape
                    // (`ExprKind::EnumVariant`) and reach
                    // `resolve_expr` through a different arm. Limiting
                    // the promotion to Class/Struct also keeps the
                    // 226-fixture e2e baseline stable (only the pre-
                    // existing `95_error_into_conversion` typecheck
                    // failure remains; everything else passes).
                    let ty = match self.symbols.get(def_id).map(|d| &d.kind) {
                        Some(DefKind::Class { .. }) => Ty::Class {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        Some(DefKind::Struct { .. }) => Ty::Struct {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        _ => self
                            .symbols
                            .def_ty(def_id)
                            .unwrap_or_else(|| self.type_context.fresh_type_var()),
                    };
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span,
                    }
                } else if let Some(def_id) = self.scopes.lookup_type(name) {
                    // Type name used as a value — needed for constructor calls
                    // like Point.new(...), Color.Red, etc.
                    let ty = match self.symbols.get(def_id).map(|d| &d.kind) {
                        Some(DefKind::Class { .. }) => Ty::Class {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        Some(DefKind::Struct { .. }) => Ty::Struct {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        Some(DefKind::Enum { .. }) => Ty::Enum {
                            name: name.clone(),
                            generic_args: vec![],
                        },
                        _ => self.type_context.fresh_type_var(),
                    };
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span,
                    }
                } else if name.contains('.') {
                    // #06.93 Phase 3: dotted identifier produced by the
                    // parser for `Outer.Inner.method(args)` shape — the
                    // parser emits `Identifier("Outer.Inner")` and
                    // postfix builds a MethodCall on top. Look up the
                    // qualified name in `type_registry` (populated by
                    // Phase 1's nested-module registration). If found
                    // as a Class/Struct/Enum, emit a Class-typed
                    // expression so the trailing `.method(args)`
                    // dispatches as a regular class-method call.
                    if let Some(&def_id) = self.type_registry.get(name) {
                        let ty = match self.symbols.get(def_id).map(|d| &d.kind) {
                            Some(DefKind::Class { .. }) => Ty::Class {
                                name: name.clone(),
                                generic_args: vec![],
                            },
                            Some(DefKind::Struct { .. }) => Ty::Struct {
                                name: name.clone(),
                                generic_args: vec![],
                            },
                            Some(DefKind::Enum { .. }) => Ty::Enum {
                                name: name.clone(),
                                generic_args: vec![],
                            },
                            _ => self.type_context.fresh_type_var(),
                        };
                        HirExpr {
                            kind: HirExprKind::VarRef(def_id),
                            ty,
                            span,
                        }
                    } else {
                        self.error(format!("undefined qualified type `{}`", name), &span);
                        HirExpr {
                            kind: HirExprKind::Error,
                            ty: Ty::Error,
                            span,
                        }
                    }
                } else {
                    self.error(format!("undefined variable `{}`", name), &span);
                    HirExpr {
                        kind: HirExprKind::Error,
                        ty: Ty::Error,
                        span,
                    }
                }
            }
            ast::ExprKind::SelfRef => {
                if let Some(def_id) = self.scopes.lookup("self") {
                    let ty = self.current_self_ty.clone().unwrap_or(Ty::Error);
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span,
                    }
                } else {
                    self.error("`self` used outside of method context".to_string(), &span);
                    HirExpr {
                        kind: HirExprKind::Error,
                        ty: Ty::Error,
                        span,
                    }
                }
            }
            ast::ExprKind::SelfType => {
                if let Some(ref ty) = self.current_self_ty {
                    let def_id = self.scopes.lookup_type("Self").unwrap_or(UNRESOLVED_DEF);
                    HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty: ty.clone(),
                        span,
                    }
                } else {
                    self.error("`Self` used outside of type context".to_string(), &span);
                    HirExpr {
                        kind: HirExprKind::Error,
                        ty: Ty::Error,
                        span,
                    }
                }
            }
            ast::ExprKind::BinaryOp { left, op, right } => {
                let left_hir = self.resolve_expr(left);
                let right_hir = self.resolve_expr(right);
                let result_ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::BinaryOp {
                        op: *op,
                        left: Box::new(left_hir),
                        right: Box::new(right_hir),
                    },
                    ty: result_ty,
                    span,
                }
            }
            ast::ExprKind::UnaryOp { op, operand } => {
                let operand_hir = self.resolve_expr(operand);
                let result_ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::UnaryOp {
                        op: *op,
                        operand: Box::new(operand_hir),
                    },
                    ty: result_ty,
                    span,
                }
            }
            ast::ExprKind::Borrow(inner) => {
                let inner_hir = self.resolve_expr(inner);
                let ty = Ty::Ref(Box::new(inner_hir.ty.clone()));
                HirExpr {
                    kind: HirExprKind::Borrow {
                        mutable: false,
                        expr: Box::new(inner_hir),
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::BorrowMut(inner) => {
                let inner_hir = self.resolve_expr(inner);
                let ty = Ty::RefMut(Box::new(inner_hir.ty.clone()));
                HirExpr {
                    kind: HirExprKind::Borrow {
                        mutable: true,
                        expr: Box::new(inner_hir),
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::FieldAccess { object, field } => {
                let obj_hir = self.resolve_expr(object);
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::FieldAccess {
                        object: Box::new(obj_hir),
                        field_name: field.clone(),
                        field_idx: 0, // resolved during type checking
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::MethodCall {
                object,
                method,
                generic_args,
                args,
                block,
            } => {
                let obj_hir = self.resolve_expr(object);
                let mut args_hir: Vec<HirExpr> =
                    args.iter().map(|a| self.resolve_expr(a)).collect();
                let block_hir = block.as_ref().map(|b| Box::new(self.resolve_expr(b)));
                if block_hir.is_none() {
                    if let HirExprKind::VarRef(module_id) = &obj_hir.kind {
                        if let Some(module_def) = self.symbols.get(*module_id) {
                            let module_name = module_def.name.clone();
                            if let DefKind::Module { items } = &module_def.kind {
                                let items = items.clone();
                                let candidates: Vec<DefId> = items
                                    .iter()
                                    .copied()
                                    .filter(|item_id| {
                                        self.symbols
                                            .get(*item_id)
                                            .map(|def| {
                                                def.name == *method
                                                    || def.name.starts_with(&format!(
                                                        "{}__overload",
                                                        method
                                                    ))
                                            })
                                            .unwrap_or(false)
                                    })
                                    .collect();
                                if let Some(def_id) =
                                    self.select_overload_candidate_by_args(&candidates, &args_hir)
                                {
                                    self.append_default_args(def_id, &mut args_hir);
                                    let child_name = self
                                        .symbols
                                        .get(def_id)
                                        .map(|d| d.name.clone())
                                        .unwrap_or_else(|| method.clone());
                                    let callee_name =
                                        format!("{}_{}", module_name.replace('.', "_"), child_name);
                                    return HirExpr {
                                        kind: HirExprKind::FnCall {
                                            callee: def_id,
                                            callee_name,
                                            args: args_hir,
                                        },
                                        ty: self.type_context.fresh_type_var(),
                                        span,
                                    };
                                }
                            }
                        }
                    }
                }
                let generic_args_hir = generic_args
                    .iter()
                    .map(|a| self.resolve_type_expr(a))
                    .collect();
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(obj_hir),
                        method: UNRESOLVED_DEF, // resolved during type checking
                        method_name: method.clone(),
                        generic_args: generic_args_hir,
                        args: args_hir,
                        block: block_hir,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Call {
                callee,
                args,
                block,
            } => {
                let mut args_hir: Vec<HirExpr> =
                    args.iter().map(|a| self.resolve_expr(a)).collect();
                let mut block_hir = block.as_ref().map(|b| Box::new(self.resolve_expr(b)));

                // E1112 (docs/specs/stdlib/executor.spec.md B6) is
                // detected in `async_lowering::collect_block_on_in_async_diagnostics`,
                // which runs BEFORE the async-fn rewrite while the
                // original `block_on(...)` call is still attached to
                // its async parent. By the time the resolver runs,
                // the call has either been erased by the block_on
                // rewriter (sync scopes) or pushed inside a
                // synthesised state-machine `poll` method (async
                // scopes), making async_scope_depth here unreliable.

                // Try to resolve the callee
                match &callee.kind {
                    ast::ExprKind::Identifier(name) => {
                        // If `name` names a function that takes an implicit
                        // block (i.e. its body contains `yield`), forward
                        // the trailing block as the last argument and emit
                        // a plain `FnCall`.  The callee's signature was
                        // given an extra trailing `__block` parameter.
                        let takes_implicit_block = self.yield_fns.contains_key(name);
                        if takes_implicit_block {
                            if let Some(blk) = block_hir.take() {
                                args_hir.push(*blk);
                            }
                            if let Some(def_id) = self.scopes.lookup(name) {
                                let def_id = self.select_overload_by_args(def_id, &args_hir);
                                self.append_default_args(def_id, &mut args_hir);
                                let callee_name = self
                                    .symbols
                                    .get(def_id)
                                    .map(|d| d.name.clone())
                                    .unwrap_or_else(|| name.clone());
                                let ty = self.type_context.fresh_type_var();
                                return HirExpr {
                                    kind: HirExprKind::FnCall {
                                        callee: def_id,
                                        callee_name,
                                        args: args_hir,
                                    },
                                    ty,
                                    span,
                                };
                            }
                        }
                        if let Some(def_id) = self.scopes.lookup(name) {
                            let def_id = self.select_overload_by_args(def_id, &args_hir);
                            self.append_default_args(def_id, &mut args_hir);
                            let resolved_callee_name = self
                                .symbols
                                .get(def_id)
                                .map(|d| d.name.clone())
                                .unwrap_or_else(|| name.clone());
                            let ty = self.type_context.fresh_type_var();
                            // Check if this is a function or a closure call
                            let kind = match block_hir {
                                Some(blk) => HirExprKind::MethodCall {
                                    object: Box::new(HirExpr {
                                        kind: HirExprKind::VarRef(def_id),
                                        ty: self.symbols.def_ty(def_id).unwrap_or(Ty::Error),
                                        span: callee.span.clone(),
                                    }),
                                    method: UNRESOLVED_DEF,
                                    method_name: "call".to_string(),
                                    generic_args: vec![],
                                    args: args_hir,
                                    block: Some(blk),
                                },
                                None => HirExprKind::FnCall {
                                    callee: def_id,
                                    callee_name: resolved_callee_name,
                                    args: args_hir,
                                },
                            };
                            HirExpr { kind, ty, span }
                        } else if let Some(type_def_id) = self.scopes.lookup_type(name) {
                            // `Name(arg)` where `Name` is the name of a type.
                            // For a zero-cost `newtype Meters(Float)` wrapper
                            // this desugars to a single-field Construct that
                            // can later be read back via `.0`.
                            if let Some(def) = self.symbols.get(type_def_id) {
                                if let DefKind::Newtype { inner } = &def.kind {
                                    let inner_ty = inner.clone();
                                    if args_hir.len() != 1 {
                                        self.error(
                                            format!(
                                                "newtype `{}` expects exactly 1 argument, got {}",
                                                name,
                                                args_hir.len(),
                                            ),
                                            &span,
                                        );
                                        return HirExpr {
                                            kind: HirExprKind::Error,
                                            ty: Ty::Error,
                                            span,
                                        };
                                    }
                                    let arg = args_hir.into_iter().next().unwrap();
                                    let ty = Ty::Newtype {
                                        name: name.clone(),
                                        inner: Box::new(inner_ty),
                                    };
                                    return HirExpr {
                                        kind: HirExprKind::Construct {
                                            type_def: type_def_id,
                                            type_name: name.clone(),
                                            fields: vec![("0".to_string(), arg)],
                                        },
                                        ty,
                                        span,
                                    };
                                }
                            }
                            self.error(format!("undefined function `{}`", name), &span);
                            HirExpr {
                                kind: HirExprKind::Error,
                                ty: Ty::Error,
                                span,
                            }
                        } else {
                            // Could be a type constructor: Type.new(...)
                            self.error(format!("undefined function `{}`", name), &span);
                            HirExpr {
                                kind: HirExprKind::Error,
                                ty: Ty::Error,
                                span,
                            }
                        }
                    }
                    // FieldAccess could be a static method call: Type.method(...)
                    ast::ExprKind::FieldAccess { object, field } => {
                        let obj_hir = self.resolve_expr(object);
                        let ty = self.type_context.fresh_type_var();
                        HirExpr {
                            kind: HirExprKind::MethodCall {
                                object: Box::new(obj_hir),
                                method: UNRESOLVED_DEF,
                                method_name: field.clone(),
                                generic_args: vec![],
                                args: args_hir,
                                block: block_hir,
                            },
                            ty,
                            span,
                        }
                    }
                    _ => {
                        let callee_hir = self.resolve_expr(callee);
                        let ty = self.type_context.fresh_type_var();
                        HirExpr {
                            kind: HirExprKind::MethodCall {
                                object: Box::new(callee_hir),
                                method: UNRESOLVED_DEF,
                                method_name: "call".to_string(),
                                generic_args: vec![],
                                args: args_hir,
                                block: block_hir,
                            },
                            ty,
                            span,
                        }
                    }
                }
            }
            ast::ExprKind::Index { object, index } => {
                let obj_hir = self.resolve_expr(object);
                let idx_hir = self.resolve_expr(index);
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::Index {
                        object: Box::new(obj_hir),
                        index: Box::new(idx_hir),
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Assign { target, value } => {
                let target_hir = self.resolve_expr(target);
                let value_hir = self.resolve_expr(value);
                HirExpr {
                    kind: HirExprKind::Assign {
                        target: Box::new(target_hir),
                        value: Box::new(value_hir),
                        semantics: MoveSemantics::Move, // determined during type checking
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::CompoundAssign { target, op, value } => {
                let target_hir = self.resolve_expr(target);
                let value_hir = self.resolve_expr(value);
                HirExpr {
                    kind: HirExprKind::CompoundAssign {
                        target: Box::new(target_hir),
                        op: *op,
                        value: Box::new(value_hir),
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::If(if_expr) => self.resolve_if(if_expr),
            ast::ExprKind::IfLet(if_let) => self.resolve_if_let(if_let),
            ast::ExprKind::Match(match_expr) => self.resolve_match(match_expr),
            ast::ExprKind::While(while_expr) => {
                let cond = self.resolve_expr(&while_expr.condition);
                self.scopes.push(ScopeKind::Loop);
                let body = self.resolve_block_as_expr(&while_expr.body);
                self.scopes.pop();
                HirExpr {
                    kind: HirExprKind::While {
                        condition: Box::new(cond),
                        body: Box::new(body),
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::WhileLet(wl) => {
                // Desugar while-let to loop + match
                let value = self.resolve_expr(&wl.value);
                self.scopes.push(ScopeKind::Loop);
                let pattern = self.resolve_pattern(&wl.pattern);
                let body = self.resolve_block_as_expr(&wl.body);
                self.scopes.pop();
                let break_expr = HirExpr {
                    kind: HirExprKind::Break(None),
                    ty: Ty::Never,
                    span: span.clone(),
                };
                HirExpr {
                    kind: HirExprKind::Loop {
                        body: Box::new(HirExpr {
                            kind: HirExprKind::Match {
                                scrutinee: Box::new(value),
                                arms: vec![
                                    HirMatchArm {
                                        pattern,
                                        guard: None,
                                        body: Box::new(body),
                                        span: span.clone(),
                                    },
                                    HirMatchArm {
                                        pattern: HirPattern::Wildcard { span: span.clone() },
                                        guard: None,
                                        body: Box::new(break_expr),
                                        span: span.clone(),
                                    },
                                ],
                            },
                            ty: Ty::Unit,
                            span: span.clone(),
                        }),
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::For(for_expr) => {
                let iterable = self.resolve_expr(&for_expr.iterable);
                self.scopes.push(ScopeKind::Loop);
                let binding_name = self.pattern_binding_name(&for_expr.pattern);
                let binding_ty = self.type_context.fresh_type_var();
                let binding_def = self.symbols.define(
                    binding_name.clone(),
                    DefKind::Variable {
                        mutable: false,
                        ty: binding_ty.clone(),
                    },
                    Visibility::Private,
                    for_expr.pattern.span().clone(),
                );
                self.scopes.insert(binding_name.clone(), binding_def);
                // For tuple patterns like (i, result), also register each sub-binding
                // and collect their DefIds so the MIR lowerer can destructure.
                let mut tuple_bindings = Vec::new();
                if let ast::Pattern::Tuple { elements, .. } = &for_expr.pattern {
                    self.register_pattern_bindings(
                        &for_expr.pattern,
                        false,
                        for_expr.pattern.span(),
                    );
                    for elem in elements {
                        if let ast::Pattern::Identifier { name, .. } = elem {
                            if let Some(def_id) = self.scopes.lookup(name) {
                                tuple_bindings.push((def_id, name.clone()));
                            }
                        }
                    }
                }
                let body = self.resolve_block_as_expr(&for_expr.body);
                self.scopes.pop();
                HirExpr {
                    kind: HirExprKind::For {
                        binding: binding_def,
                        binding_name,
                        iterable: Box::new(iterable),
                        body: Box::new(body),
                        tuple_bindings,
                    },
                    ty: Ty::Unit,
                    span,
                }
            }
            ast::ExprKind::Loop(loop_expr) => {
                self.scopes.push(ScopeKind::Loop);
                let body = self.resolve_block_as_expr(&loop_expr.body);
                self.scopes.pop();
                HirExpr {
                    kind: HirExprKind::Loop {
                        body: Box::new(body),
                    },
                    ty: self.type_context.fresh_type_var(),
                    span,
                }
            }
            ast::ExprKind::Block(block) => self.resolve_block_as_expr(block),
            ast::ExprKind::Closure(closure) => self.resolve_closure(closure, &span),
            ast::ExprKind::Return(value) => {
                let value_hir = value.as_ref().map(|v| Box::new(self.resolve_expr(v)));
                HirExpr {
                    kind: HirExprKind::Return(value_hir),
                    ty: Ty::Never,
                    span,
                }
            }
            ast::ExprKind::Break(value) => {
                if !self.scopes.in_loop() {
                    self.error("`break` used outside of loop".to_string(), &span);
                }
                let value_hir = value.as_ref().map(|v| Box::new(self.resolve_expr(v)));
                HirExpr {
                    kind: HirExprKind::Break(value_hir),
                    ty: Ty::Never,
                    span,
                }
            }
            ast::ExprKind::Continue => {
                if !self.scopes.in_loop() {
                    self.error("`continue` used outside of loop".to_string(), &span);
                }
                HirExpr {
                    kind: HirExprKind::Continue,
                    ty: Ty::Never,
                    span,
                }
            }
            ast::ExprKind::Range {
                start,
                end,
                inclusive,
            } => {
                let start_hir = start.as_ref().map(|s| Box::new(self.resolve_expr(s)));
                let end_hir = end.as_ref().map(|e| Box::new(self.resolve_expr(e)));
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::Range {
                        start: start_hir,
                        end: end_hir,
                        inclusive: *inclusive,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::ArrayLiteral(elems) => {
                let elems_hir: Vec<HirExpr> = elems.iter().map(|e| self.resolve_expr(e)).collect();
                let elem_ty = if elems_hir.is_empty() {
                    self.type_context.fresh_type_var()
                } else {
                    elems_hir[0].ty.clone()
                };
                let ty = Ty::Array(Box::new(elem_ty));
                HirExpr {
                    kind: HirExprKind::ArrayLiteral(elems_hir),
                    ty,
                    span,
                }
            }
            ast::ExprKind::MapLiteral(entries) => {
                let entries_hir: Vec<(HirExpr, HirExpr)> = entries
                    .iter()
                    .map(|(k, v)| (self.resolve_expr(k), self.resolve_expr(v)))
                    .collect();
                let (k_ty, v_ty) = if let Some((k, v)) = entries_hir.first() {
                    (k.ty.clone(), v.ty.clone())
                } else {
                    (
                        self.type_context.fresh_type_var(),
                        self.type_context.fresh_type_var(),
                    )
                };
                let ty = Ty::Map(Box::new(k_ty), Box::new(v_ty));
                HirExpr {
                    kind: HirExprKind::MapLiteral(entries_hir),
                    ty,
                    span,
                }
            }
            ast::ExprKind::ArrayFill { value, count } => {
                let value_hir = self.resolve_expr(value);
                let count_hir = self.resolve_expr(count);
                let elem_ty = value_hir.ty.clone();
                // Try to extract count as a usize
                let count_val = match &count_hir.kind {
                    HirExprKind::IntLiteral(n) => *n as usize,
                    _ => 0, // will be validated during type checking
                };
                HirExpr {
                    kind: HirExprKind::ArrayFill {
                        value: Box::new(value_hir),
                        count: count_val,
                    },
                    ty: Ty::FixedArray(
                        Box::new(elem_ty),
                        crate::hir::types::ConstExpr::Lit(count_val as u64),
                    ),
                    span,
                }
            }
            ast::ExprKind::TupleLiteral(elems) => {
                let elems_hir: Vec<HirExpr> = elems.iter().map(|e| self.resolve_expr(e)).collect();
                let tys: Vec<Ty> = elems_hir.iter().map(|e| e.ty.clone()).collect();
                HirExpr {
                    kind: HirExprKind::Tuple(elems_hir),
                    ty: Ty::Tuple(tys),
                    span,
                }
            }
            ast::ExprKind::Cast {
                expr: inner,
                target_type,
            } => {
                let inner_hir = self.resolve_expr(inner);
                let target = self.resolve_type_expr(target_type);
                HirExpr {
                    kind: HirExprKind::Cast {
                        expr: Box::new(inner_hir),
                        target: target.clone(),
                    },
                    ty: target,
                    span,
                }
            }
            ast::ExprKind::Await(inner) => {
                if self.async_scope_depth == 0 {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        "`.await` is only valid inside `async def` or `async { }`",
                        span.clone(),
                        "E1110",
                    ));
                }
                let inner_hir = self.resolve_expr(inner);
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(inner_hir),
                        method: UNRESOLVED_DEF,
                        method_name: "await".to_string(),
                        generic_args: vec![],
                        args: vec![],
                        block: None,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Try(inner) => {
                // Desugar `expr?` to match + early return
                let inner_hir = self.resolve_expr(inner);
                let result_ty = self.type_context.fresh_type_var();
                // For now, represent as a method call to a special `try_unwrap` operation
                // The type checker will handle the actual desugaring
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(inner_hir),
                        method: UNRESOLVED_DEF,
                        method_name: "try_op".to_string(),
                        generic_args: vec![],
                        args: vec![],
                        block: None,
                    },
                    ty: result_ty,
                    span,
                }
            }
            ast::ExprKind::SafeNav { object, field } => {
                let obj_hir = self.resolve_expr(object);
                let ty = self.type_context.fresh_type_var();
                // Desugar `x?.field` to match on Option
                HirExpr {
                    kind: HirExprKind::FieldAccess {
                        object: Box::new(obj_hir),
                        field_name: field.clone(),
                        field_idx: 0,
                    },
                    ty: Ty::Option(Box::new(ty)),
                    span,
                }
            }
            ast::ExprKind::SafeNavCall {
                object,
                method,
                args,
            } => {
                let obj_hir = self.resolve_expr(object);
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(obj_hir),
                        method: UNRESOLVED_DEF,
                        method_name: method.clone(),
                        generic_args: vec![],
                        args: args_hir,
                        block: None,
                    },
                    ty: Ty::Option(Box::new(ty)),
                    span,
                }
            }
            ast::ExprKind::MacroCall { name, args, .. } => {
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                let ty = match name.as_str() {
                    // ruby-naming.spec.md §10a:
                    //   `vec![...]` → `array![...]`
                    //   `hash!{...}` → `map!{...}`
                    //   `set!{...}` (unchanged)
                    // Both old and new macro names produce identical HIR
                    // while sources transition.
                    "vec" | "array" => {
                        let elem_ty = if args_hir.is_empty() {
                            self.type_context.fresh_type_var()
                        } else {
                            args_hir[0].ty.clone()
                        };
                        Ty::Array(Box::new(elem_ty))
                    }
                    "hash" | "map" => {
                        let (k, v) = if args_hir.len() >= 2 {
                            (args_hir[0].ty.clone(), args_hir[1].ty.clone())
                        } else {
                            (
                                self.type_context.fresh_type_var(),
                                self.type_context.fresh_type_var(),
                            )
                        };
                        Ty::Map(Box::new(k), Box::new(v))
                    }
                    "set" => {
                        let elem = if args_hir.is_empty() {
                            self.type_context.fresh_type_var()
                        } else {
                            args_hir[0].ty.clone()
                        };
                        Ty::Set(Box::new(elem))
                    }
                    "panic" => Ty::Never,
                    _ => self.type_context.fresh_type_var(),
                };
                HirExpr {
                    kind: HirExprKind::MacroCall {
                        name: name.clone(),
                        args: args_hir,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::EnumVariant {
                type_path,
                variant,
                args,
            } => {
                let type_name = type_path.join(".");
                let composite = format!("{}.{}", type_name, variant);
                let variant_def = self.scopes.lookup(&composite).unwrap_or(UNRESOLVED_DEF);
                let mut type_def = self
                    .type_registry
                    .get(&type_name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);

                // For bare variants (Ok, Err, Some, None) where type_path is empty,
                // look up the parent enum from the variant definition
                let mut resolved_type_name = type_name.clone();
                if type_def == UNRESOLVED_DEF && variant_def != UNRESOLVED_DEF {
                    if let Some(def) = self.symbols.get(variant_def) {
                        if let DefKind::EnumVariant { parent, .. } = &def.kind {
                            type_def = *parent;
                            if let Some(parent_def) = self.symbols.get(*parent) {
                                resolved_type_name = parent_def.name.clone();
                            }
                        }
                    }
                }

                // Extract variant_idx first to avoid borrow conflicts
                let variant_idx = if variant_def != UNRESOLVED_DEF {
                    self.symbols
                        .get(variant_def)
                        .and_then(|def| {
                            if let DefKind::EnumVariant { variant_idx, .. } = &def.kind {
                                Some(*variant_idx)
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0)
                } else {
                    self.error(
                        format!("undefined enum variant `{}.{}`", type_name, variant),
                        &span,
                    );
                    0
                };

                let fields_hir: Vec<(String, HirExpr)> = args
                    .iter()
                    .map(|fa| {
                        (
                            fa.name.clone().unwrap_or_default(),
                            self.resolve_expr(&fa.value),
                        )
                    })
                    .collect();

                let ty = if type_def != UNRESOLVED_DEF {
                    Ty::Enum {
                        name: resolved_type_name.clone(),
                        generic_args: vec![],
                    }
                } else {
                    Ty::Error
                };

                HirExpr {
                    kind: HirExprKind::EnumVariant {
                        type_def,
                        type_name: resolved_type_name,
                        variant_name: variant.clone(),
                        variant_idx,
                        fields: fields_hir,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::ClosureCall { callee, args } => {
                let callee_hir = self.resolve_expr(callee);
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                let ty = self.type_context.fresh_type_var();
                HirExpr {
                    kind: HirExprKind::MethodCall {
                        object: Box::new(callee_hir),
                        method: UNRESOLVED_DEF,
                        method_name: "call".to_string(),
                        generic_args: vec![],
                        args: args_hir,
                        block: None,
                    },
                    ty,
                    span,
                }
            }
            ast::ExprKind::Yield(args) => {
                let args_hir: Vec<HirExpr> = args.iter().map(|a| self.resolve_expr(a)).collect();
                // `yield VALUE …` desugars to `BLOCK.(VALUE …)`, encoded as
                // a MethodCall with method_name == "call" on the enclosing
                // function's block parameter.  Prefer the synthetic
                // `__block` inserted for implicit-block functions; fall
                // back to the explicit `&block` parameter name that the
                // older `Block(…)` syntax produces.  If neither is in
                // scope (e.g. a `yield` sitting inside a nested closure
                // whose enclosing method has no block), we keep the old
                // unresolved-FnCall shape so downstream passes can report
                // a clearer error.
                let block_def = self
                    .scopes
                    .lookup("__block")
                    .or_else(|| self.scopes.lookup("&block"));
                if let Some(block_def) = block_def {
                    let block_ty = self.symbols.def_ty(block_def).unwrap_or(Ty::Error);
                    let callee = HirExpr {
                        kind: HirExprKind::VarRef(block_def),
                        ty: block_ty,
                        span: span.clone(),
                    };
                    let ty = self.type_context.fresh_type_var();
                    HirExpr {
                        kind: HirExprKind::MethodCall {
                            object: Box::new(callee),
                            method: UNRESOLVED_DEF,
                            method_name: "call".to_string(),
                            generic_args: vec![],
                            args: args_hir,
                            block: None,
                        },
                        ty,
                        span,
                    }
                } else {
                    let ty = self.type_context.fresh_type_var();
                    HirExpr {
                        kind: HirExprKind::FnCall {
                            callee: UNRESOLVED_DEF,
                            callee_name: "yield".to_string(),
                            args: args_hir,
                        },
                        ty,
                        span,
                    }
                }
            }
            ast::ExprKind::UnsafeBlock(block) => {
                // Resolve the unsafe block body just like a regular block.
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
                    kind: HirExprKind::UnsafeBlock(stmts, tail_expr),
                    ty,
                    span,
                }
            }
            ast::ExprKind::NullLiteral => {
                HirExpr {
                    kind: HirExprKind::NullLiteral,
                    ty: Ty::UInt64, // null is a zero-valued pointer; for now UInt64
                    span,
                }
            }
        }
    }

    // ─── Block Resolution ───────────────────────────────────────────
}
