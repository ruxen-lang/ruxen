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
    pub(super) fn resolve_pattern(&mut self, pattern: &ast::Pattern) -> HirPattern {
        self.resolve_pattern_with_type(pattern, &Ty::Error)
    }

    pub(super) fn resolve_pattern_with_type(
        &mut self,
        pattern: &ast::Pattern,
        _expected_ty: &Ty,
    ) -> HirPattern {
        match pattern {
            ast::Pattern::Wildcard { span } => HirPattern::Wildcard { span: span.clone() },
            ast::Pattern::Identifier {
                mutable,
                name,
                span,
            } => {
                let ty = self.type_context.fresh_type_var();
                let def_id = self.symbols.define(
                    name.clone(),
                    DefKind::Variable {
                        mutable: *mutable,
                        ty,
                    },
                    Visibility::Private,
                    span.clone(),
                );
                // Register the binding in the current scope so that body
                // expressions (e.g. match arm bodies) resolve to the same
                // def_id.  `register_pattern_bindings` guards against
                // duplicates with an `is_none()` check.
                self.scopes.insert(name.clone(), def_id);
                HirPattern::Binding {
                    def_id,
                    name: name.clone(),
                    mutable: *mutable,
                    span: span.clone(),
                }
            }
            ast::Pattern::Literal { expr, span } => {
                let hir_expr = self.resolve_expr(expr);
                HirPattern::Literal {
                    expr: Box::new(hir_expr),
                    span: span.clone(),
                }
            }
            ast::Pattern::Tuple { elements, span } => {
                let elems: Vec<HirPattern> =
                    elements.iter().map(|e| self.resolve_pattern(e)).collect();
                HirPattern::Tuple {
                    elements: elems,
                    span: span.clone(),
                }
            }
            ast::Pattern::Enum {
                path,
                variant,
                fields,
                span,
            } => {
                let type_name = path.join(".");
                let composite = format!("{}.{}", type_name, variant);
                let variant_def = self.scopes.lookup(&composite).unwrap_or_else(|| {
                    self.error(format!("undefined enum variant `{}`", composite), span);
                    UNRESOLVED_DEF
                });

                let variant_idx = if variant_def != UNRESOLVED_DEF {
                    if let Some(def) = self.symbols.get(variant_def) {
                        if let DefKind::EnumVariant { variant_idx, .. } = &def.kind {
                            *variant_idx
                        } else {
                            0
                        }
                    } else {
                        0
                    }
                } else {
                    0
                };

                let type_def = self
                    .type_registry
                    .get(&type_name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);
                let fields_hir: Vec<HirPattern> =
                    fields.iter().map(|f| self.resolve_pattern(f)).collect();

                HirPattern::Enum {
                    type_def,
                    variant_idx,
                    variant_name: variant.clone(),
                    fields: fields_hir,
                    span: span.clone(),
                }
            }
            ast::Pattern::Struct {
                path,
                fields,
                rest,
                span,
            } => {
                let type_name = path.join(".");
                let type_def = self
                    .type_registry
                    .get(&type_name)
                    .copied()
                    .unwrap_or(UNRESOLVED_DEF);
                let fields_hir: Vec<(String, HirPattern)> = fields
                    .iter()
                    .map(|f| {
                        let name = f.name.clone().unwrap_or_default();
                        let pat = self.resolve_pattern(&f.pattern);
                        (name, pat)
                    })
                    .collect();
                HirPattern::Struct {
                    type_def,
                    fields: fields_hir,
                    rest: *rest,
                    span: span.clone(),
                }
            }
            ast::Pattern::Or { patterns, span } => {
                let pats: Vec<HirPattern> =
                    patterns.iter().map(|p| self.resolve_pattern(p)).collect();
                HirPattern::Or {
                    patterns: pats,
                    span: span.clone(),
                }
            }
            ast::Pattern::Ref {
                mutable,
                name,
                span,
            } => {
                let ty = self.type_context.fresh_type_var();
                let def_id = self.symbols.define(
                    name.clone(),
                    DefKind::Variable {
                        mutable: *mutable,
                        ty,
                    },
                    Visibility::Private,
                    span.clone(),
                );
                // Insert into scope so that VarRef lookups in the arm
                // body resolve to the same def_id as the pattern binding.
                self.scopes.insert(name.clone(), def_id);
                HirPattern::Ref {
                    mutable: *mutable,
                    name: name.clone(),
                    def_id,
                    span: span.clone(),
                }
            }
            ast::Pattern::Rest { span } => HirPattern::Rest { span: span.clone() },
        }
    }

    pub(super) fn register_pattern_bindings(
        &mut self,
        pattern: &ast::Pattern,
        mutable: bool,
        span: &Span,
    ) {
        match pattern {
            ast::Pattern::Identifier { name, .. } => {
                // Already handled in resolve_pattern_with_type for let-bindings,
                // but for match/for patterns we need to register too
                if self.scopes.lookup(name).is_none() {
                    let ty = self.type_context.fresh_type_var();
                    let def_id = self.symbols.define(
                        name.clone(),
                        DefKind::Variable { mutable, ty },
                        Visibility::Private,
                        span.clone(),
                    );
                    self.scopes.insert(name.clone(), def_id);
                }
            }
            ast::Pattern::Tuple { elements, .. } => {
                for elem in elements {
                    self.register_pattern_bindings(elem, mutable, span);
                }
            }
            ast::Pattern::Enum { fields, .. } => {
                for field in fields {
                    self.register_pattern_bindings(field, mutable, span);
                }
            }
            ast::Pattern::Struct { fields, .. } => {
                for field in fields {
                    self.register_pattern_bindings(&field.pattern, mutable, span);
                }
            }
            ast::Pattern::Or { patterns, .. } => {
                // All alternatives must bind the same names
                if let Some(first) = patterns.first() {
                    self.register_pattern_bindings(first, mutable, span);
                }
            }
            ast::Pattern::Ref {
                name, mutable: m, ..
            } => {
                if self.scopes.lookup(name).is_none() {
                    let ty = self.type_context.fresh_type_var();
                    let def_id = self.symbols.define(
                        name.clone(),
                        DefKind::Variable { mutable: *m, ty },
                        Visibility::Private,
                        span.clone(),
                    );
                    self.scopes.insert(name.clone(), def_id);
                }
            }
            _ => {}
        }
    }

    // ─── Type Expression Resolution ─────────────────────────────────
}
