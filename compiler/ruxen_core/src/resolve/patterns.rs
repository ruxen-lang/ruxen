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
    /// Resolves `pattern`, defining every name it binds in the current
    /// scope. This is the only place pattern names are bound, so an inner
    /// binding always shadows an outer one of the same name.
    pub(super) fn resolve_pattern(&mut self, pattern: &ast::Pattern) -> HirPattern {
        self.bind_pattern(pattern, None, false)
    }

    /// Resolves a `let`/`var` pattern: a top-level name takes the declared
    /// `ty`, and `var` (`mutable`) makes every name it binds mutable.
    pub(super) fn resolve_let_pattern(
        &mut self,
        pattern: &ast::Pattern,
        ty: &Ty,
        mutable: bool,
    ) -> HirPattern {
        self.bind_pattern(pattern, Some(ty), mutable)
    }

    fn bind_pattern(
        &mut self,
        pattern: &ast::Pattern,
        ty: Option<&Ty>,
        var_mutable: bool,
    ) -> HirPattern {
        match pattern {
            ast::Pattern::Wildcard { span } => HirPattern::Wildcard { span: span.clone() },
            ast::Pattern::Identifier {
                mutable,
                name,
                span,
            } => {
                let mutable = *mutable || var_mutable;
                let ty = ty
                    .cloned()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                let def_id = self.symbols.define(
                    name.clone(),
                    DefKind::Variable { mutable, ty },
                    Visibility::Private,
                    span.clone(),
                );
                // Register the binding in the current scope so that body
                // expressions (e.g. match arm bodies) resolve to this def_id.
                self.scopes.insert(name.clone(), def_id);
                HirPattern::Binding {
                    def_id,
                    name: name.clone(),
                    mutable,
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
                let elems: Vec<HirPattern> = elements
                    .iter()
                    .map(|e| self.bind_pattern(e, None, var_mutable))
                    .collect();
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
                    self.unresolved_names.push(composite.clone());
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
                let fields_hir: Vec<HirPattern> = fields
                    .iter()
                    .map(|f| self.bind_pattern(f, None, var_mutable))
                    .collect();

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
                        let pat = self.bind_pattern(&f.pattern, None, var_mutable);
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
                let pats: Vec<HirPattern> = patterns
                    .iter()
                    .map(|p| self.bind_pattern(p, None, var_mutable))
                    .collect();
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
                let ty = ty
                    .cloned()
                    .unwrap_or_else(|| self.type_context.fresh_type_var());
                let def_id = self.symbols.define(
                    name.clone(),
                    DefKind::Variable {
                        mutable: *mutable || var_mutable,
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

    // ─── Type Expression Resolution ─────────────────────────────────
}
