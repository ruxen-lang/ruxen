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
    /// Tier-2 const generics S5: walk the AST `GenericParams` and
    /// produce a kind-aware `Vec<GenericParamInfo>` suitable for
    /// storage on `ClassInfo` / `StructInfo` / `EnumInfo` / `FnSignature`.
    /// Each entry preserves the source-order position (so use-site
    /// generic-arg validation can pair the i'th arg with the i'th
    /// declared param).  Lifetimes are skipped (not stored on info).
    pub(crate) fn collect_generic_param_infos(
        &mut self,
        gp: &Option<ast::GenericParams>,
    ) -> Vec<GenericParamInfo> {
        let Some(gps) = gp.as_ref() else {
            return vec![];
        };
        gps.params
            .iter()
            .filter_map(|p| match p {
                ast::GenericParam::Type { name, bounds, .. } => {
                    let trait_refs: Vec<MixinRef> = bounds
                        .iter()
                        .map(|b| MixinRef {
                            name: b.path.segments.join("."),
                            generic_args: vec![],
                        })
                        .collect();
                    Some(GenericParamInfo::type_param(name.clone(), trait_refs))
                }
                ast::GenericParam::Const { name, ty, .. } => {
                    let resolved_ty = self.resolve_type_expr(ty);
                    Some(GenericParamInfo::const_param(name.clone(), resolved_ty))
                }
                ast::GenericParam::Lifetime { .. } => None,
            })
            .collect()
    }

    pub(super) fn resolve_generic_params(
        &mut self,
        gp: &Option<ast::GenericParams>,
    ) -> Vec<HirGenericParam> {
        // Stage 3 of const generics: first pass registers every
        // `GenericParam::Const` as a `DefKind::ConstParam` in the
        // symbol table so future passes (S4 HIR ConstExpr, S5
        // typeck unification) can look the name up.  We do this in
        // a separate pre-pass because `filter_map` captures `&mut
        // self` and Rust's borrow checker dislikes the symbol-table
        // mutation happening inside the type-param iteration.
        if let Some(gps) = gp.as_ref() {
            for p in &gps.params {
                if let ast::GenericParam::Const { name, ty, span } = p {
                    let resolved_ty = self.resolve_type_expr(ty);
                    // T2.02 spec §B8 (E-CONST-BAD-TYPE → E0705):
                    // a const-generic parameter's declared type must be
                    // an integer family or `Bool`.  Float* is non-goal
                    // NG2 (NaN ≠ NaN breaks the Eq contract const
                    // generics share); String / class / Vec / tuple
                    // const generics are also non-goals (NG3).
                    if !const_helpers::is_valid_const_param_ty(&resolved_ty) {
                        self.diagnostics.push(Diagnostic::error_with_code(
                            format!(
                                "const-generic parameter `{}` must be an integer or `Bool`, found `{}`",
                                name, resolved_ty
                            ),
                            span.clone(),
                            "E0705",
                        ));
                    }
                    let _ = self.symbols.define(
                        name.clone(),
                        DefKind::ConstParam { ty: resolved_ty },
                        Visibility::Public,
                        span.clone(),
                    );
                }
            }
        }

        gp.as_ref()
            .map(|gps| {
                gps.params
                    .iter()
                    .filter_map(|p| {
                        match p {
                            ast::GenericParam::Type { name, bounds, span } => {
                                let trait_refs: Vec<MixinRef> = bounds
                                    .iter()
                                    .map(|b| MixinRef {
                                        name: b.path.segments.join("."),
                                        generic_args: b
                                            .path
                                            .generic_args
                                            .as_ref()
                                            .map(|args| {
                                                args.iter()
                                                    .map(|a| self.resolve_type_expr(a))
                                                    .collect()
                                            })
                                            .unwrap_or_default(),
                                    })
                                    .collect();
                                Some(HirGenericParam {
                                    name: name.clone(),
                                    bounds: trait_refs,
                                    span: span.clone(),
                                })
                            }
                            ast::GenericParam::Lifetime { .. } => {
                                // Lifetimes are tracked but not yet used in Phase 3
                                None
                            }
                            // Const params were registered in the
                            // pre-pass above and don't appear in the
                            // HirGenericParam list (which is for
                            // type params only).
                            ast::GenericParam::Const { .. } => None,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn resolve_params(&mut self, params: &[ast::Param]) -> Vec<HirParam> {
        params
            .iter()
            .map(|p| {
                let ty = self.resolve_type_expr(&p.type_expr);
                let def_id = self.symbols.define(
                    p.name.clone(),
                    DefKind::Param {
                        ty: ty.clone(),
                        auto_assign: p.auto_assign,
                    },
                    Visibility::Private,
                    p.span.clone(),
                );
                HirParam {
                    def_id,
                    name: p.name.clone(),
                    ty,
                    auto_assign: p.auto_assign,
                    default: p.default.as_deref().cloned(),
                    span: p.span.clone(),
                }
            })
            .collect()
    }

    pub(super) fn resolve_and_register_params(&mut self, params: &[ast::Param]) -> Vec<HirParam> {
        params
            .iter()
            .map(|p| {
                let ty = self.resolve_type_expr(&p.type_expr);
                let def_id = self.symbols.define(
                    p.name.clone(),
                    DefKind::Param {
                        ty: ty.clone(),
                        auto_assign: p.auto_assign,
                    },
                    Visibility::Private,
                    p.span.clone(),
                );
                self.scopes.insert(p.name.clone(), def_id);
                HirParam {
                    def_id,
                    name: p.name.clone(),
                    ty,
                    auto_assign: p.auto_assign,
                    default: p.default.as_deref().cloned(),
                    span: p.span.clone(),
                }
            })
            .collect()
    }

    pub(super) fn convert_self_mode(&self, mode: ast::SelfMode) -> HirSelfMode {
        match mode {
            ast::SelfMode::Immutable => HirSelfMode::Ref,
            ast::SelfMode::Mutable => HirSelfMode::RefMut,
            ast::SelfMode::Consuming => HirSelfMode::Consuming,
        }
    }

    pub(super) fn int_literal_type(
        &self,
        suffix: Option<crate::lexer::token::NumericSuffix>,
    ) -> Ty {
        use crate::lexer::token::NumericSuffix;
        match suffix {
            None => Ty::Int,
            Some(NumericSuffix::I8) => Ty::Int8,
            Some(NumericSuffix::I16) => Ty::Int16,
            Some(NumericSuffix::I32) => Ty::Int32,
            Some(NumericSuffix::I64) => Ty::Int64,
            Some(NumericSuffix::U) => Ty::UInt,
            Some(NumericSuffix::U8) => Ty::UInt8,
            Some(NumericSuffix::U16) => Ty::UInt16,
            Some(NumericSuffix::U32) => Ty::UInt32,
            Some(NumericSuffix::U64) => Ty::UInt64,
            Some(NumericSuffix::ISize) => Ty::ISize,
            Some(NumericSuffix::USize) => Ty::USize,
            Some(NumericSuffix::F32) => Ty::Float32,
            Some(NumericSuffix::F64) => Ty::Float64,
        }
    }

    pub(super) fn float_literal_type(
        &self,
        suffix: Option<crate::lexer::token::NumericSuffix>,
    ) -> Ty {
        use crate::lexer::token::NumericSuffix;
        match suffix {
            None => Ty::Float,
            Some(NumericSuffix::F32) => Ty::Float32,
            Some(NumericSuffix::F64) => Ty::Float64,
            _ => Ty::Float,
        }
    }

    pub(super) fn pattern_binding_name(&self, pattern: &ast::Pattern) -> String {
        match pattern {
            ast::Pattern::Identifier { name, .. } => name.clone(),
            ast::Pattern::Tuple { .. } => "_tuple".to_string(),
            ast::Pattern::Ref { name, .. } => name.clone(),
            _ => "_".to_string(),
        }
    }

    pub(super) fn resolve_interpolation_tokens(
        &mut self,
        tokens: &[crate::lexer::token::Token],
        span: &Span,
    ) -> HirExpr {
        // The lexer gives us pre-tokenized expression tokens from #{...}
        // We need to parse them as an expression.
        // Wrap in a function body so the parser can handle them.
        if tokens.is_empty() {
            return HirExpr {
                kind: HirExprKind::StringLiteral(String::new()),
                ty: Ty::String,
                span: span.clone(),
            };
        }

        // Build a synthetic token stream: def _interp_ \n <tokens> \n end
        use crate::lexer::token::{Token, TokenKind};
        let dummy_span = Span {
            start: 0,
            end: 0,
            line: 0,
            column: 0,
        };
        let mut wrapped_tokens = vec![
            Token {
                kind: TokenKind::Def,
                span: dummy_span.clone(),
            },
            Token {
                kind: TokenKind::Identifier("_interp_".to_string()),
                span: dummy_span.clone(),
            },
            Token {
                kind: TokenKind::Newline,
                span: dummy_span.clone(),
            },
        ];
        wrapped_tokens.extend(tokens.iter().cloned());
        wrapped_tokens.push(Token {
            kind: TokenKind::Newline,
            span: dummy_span.clone(),
        });
        wrapped_tokens.push(Token {
            kind: TokenKind::End,
            span: dummy_span.clone(),
        });
        wrapped_tokens.push(Token {
            kind: TokenKind::Newline,
            span: dummy_span.clone(),
        });
        wrapped_tokens.push(Token {
            kind: TokenKind::Eof,
            span: dummy_span.clone(),
        });

        let mut parser = crate::parser::Parser::new(wrapped_tokens);
        if let Ok(program) = parser.parse() {
            if let Some(ast::TopLevelItem::Function(f)) = program.items.first() {
                if let Some(ast::Statement::Expression(expr)) = f.body.statements.first() {
                    return self.resolve_expr(expr);
                }
            }
        }

        // Fallback: if we can't parse, try a simple identifier lookup
        // (handles the common `#{variable}` case)
        if tokens.len() == 1 {
            if let TokenKind::Identifier(ref name) = tokens[0].kind {
                if let Some(def_id) = self.scopes.lookup(name) {
                    let ty = self
                        .symbols
                        .def_ty(def_id)
                        .unwrap_or_else(|| self.type_context.fresh_type_var());
                    return HirExpr {
                        kind: HirExprKind::VarRef(def_id),
                        ty,
                        span: span.clone(),
                    };
                }
            }
        }

        HirExpr {
            kind: HirExprKind::Error,
            ty: Ty::String,
            span: span.clone(),
        }
    }

    pub(super) fn error(&mut self, message: String, span: &Span) {
        self.diagnostics
            .push(Diagnostic::error(message, span.clone()));
    }

    /// T2.02 S8.S4 follow-up: surface pure-literal overflow /
    /// div-zero in a `ConstExpr` as **E0703**.  Called immediately
    /// after the S8.S4 normal-form pass at every const-arg /
    /// array-size resolve site; the normalisation collapses
    /// successful pure-literal `Op` nodes to `Lit`, so any `Op`
    /// that survives with literal children is by definition an
    /// eval failure and is invariant across instantiations.
    ///
    /// Param-bearing trees (`N + 1`, `M * 2`) surface
    /// `Err(Unresolved(name))` from `eval` — those are *deferred*
    /// to the monomorphization-side check (the spec's per-
    /// instantiation eval surfacing pass that's still pending).
    /// `Err(Malformed)` (parser recovery) is also skipped — the
    /// parser already emitted its own diagnostic upstream.
    pub(super) fn check_const_expr_eval_errors(
        &mut self,
        expr: &crate::hir::types::ConstExpr,
        span: &Span,
    ) {
        use crate::hir::types::ConstEvalError;
        let bindings = std::collections::HashMap::new();
        match expr.eval(&bindings) {
            Ok(_) => {}
            Err(ConstEvalError::Unresolved(_)) | Err(ConstEvalError::Malformed) => {}
            Err(ConstEvalError::NotImplemented) => {}
            Err(ConstEvalError::Overflow) => {
                self.diagnostics.push(Diagnostic::error_with_code(
                    "const expression overflows during evaluation".to_string(),
                    span.clone(),
                    "E0703",
                ));
            }
            Err(ConstEvalError::DivisionByZero) => {
                self.diagnostics.push(Diagnostic::error_with_code(
                    "const expression divides by zero".to_string(),
                    span.clone(),
                    "E0703",
                ));
            }
        }
    }

    /// T2.02 §B8 (E-CONST-NONCONST → E0702): surface
    /// `ConstExpr::Error` nodes — the marker
    /// `lower_const_expr_from_expr` produces for AST shapes that
    /// aren't valid v1 const expressions (unsupported binary ops
    /// like `%` / `<` / `<<`, function calls, method calls, field
    /// access, runtime variable references, …).
    ///
    /// Walks the tree once.  At most one E0702 is emitted per
    /// resolve-site span — the first reachable `Error` triggers
    /// it; nested noise stays quiet so the user sees the source
    /// location, not a diagnostic for every leaf.
    pub(super) fn check_const_expr_for_non_const(
        &mut self,
        expr: &crate::hir::types::ConstExpr,
        span: &Span,
    ) {
        if const_helpers::contains_const_expr_error(expr) {
            self.diagnostics.push(Diagnostic::error_with_code(
                "expression is not a valid const expression \
                 (v1 supports integer literals, in-scope const-param references, \
                 and `+ - * /` arithmetic over those)"
                    .to_string(),
                span.clone(),
                "E0702",
            ));
        }
    }
}
