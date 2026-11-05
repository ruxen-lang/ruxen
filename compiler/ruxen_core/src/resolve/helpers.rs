use crate::diagnostics::Diagnostic;
use crate::hir::nodes::*;
use crate::hir::types::{MixinRef, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::{self, Visibility};

use super::const_helpers;
use super::scope::ScopeKind;
use super::symbols::*;
use super::{PendingFieldHint, Resolver};

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
                        .map(|b| MixinRef::from_bound_name(&b.path))
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

    /// The target of `type Name[P…] = T` with the alias's own generic
    /// parameters in scope (as type parameters), and those parameters' names
    /// in order: what `DefKind::TypeAlias` stores. Shared by the type
    /// registration pass and the HIR pass.
    pub(super) fn resolve_alias_target(&mut self, ta: &ast::TypeAliasDef) -> (Ty, Vec<String>) {
        if ta.generic_params.is_none() {
            return (self.resolve_type_expr(&ta.type_expr), Vec::new());
        }
        let generic_params = self.resolve_generic_params(&ta.generic_params);
        self.scopes.push(ScopeKind::Class);
        for gp in &generic_params {
            let gp_def = self.symbols.define(
                gp.name.clone(),
                DefKind::TypeParam {
                    bounds: gp.bounds.clone(),
                },
                Visibility::Private,
                gp.span.clone(),
            );
            self.scopes.insert_type(gp.name.clone(), gp_def);
        }
        let target = self.resolve_type_expr(&ta.type_expr);
        self.scopes.pop();
        (
            target,
            generic_params.into_iter().map(|gp| gp.name).collect(),
        )
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
                                    .map(|b| {
                                        MixinRef::from_bound(&b.path, |a| self.resolve_type_expr(a))
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

    /// [`Self::resolve_params`], with each param also bound by name in the
    /// current value scope.
    pub(super) fn resolve_and_register_params(&mut self, params: &[ast::Param]) -> Vec<HirParam> {
        let hir_params = self.resolve_params(params);
        for p in &hir_params {
            self.scopes.insert(p.name.clone(), p.def_id);
        }
        hir_params
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

    pub(super) fn error(&mut self, message: String, span: &Span) {
        self.diagnostics
            .push(Diagnostic::error(message, span.clone()));
    }

    /// E1127: `super` only dispatches to the parent's `init`, so it is
    /// valid only inside the instance `init` of a class with a parent.
    /// Anywhere else it used to lower to a silent no-op.
    pub(super) fn check_super_context(&mut self, span: &Span) {
        if !self.in_subclass_init {
            self.error_with_code(
                "`super` is only allowed inside `init` of a class that has a parent class"
                    .to_string(),
                span,
                "E1127",
            );
        }
    }

    /// Same as [`Self::error`] but carries a registry code so
    /// `ruxen explain E####` has something to say. `code` must appear in
    /// `diagnostics::codes::REGISTRY` (a test enforces it).
    pub(super) fn error_with_code(&mut self, message: String, span: &Span, code: &'static str) {
        self.diagnostics
            .push(Diagnostic::error_with_code(message, span.clone(), code));
    }

    /// E0734 for a bare name that is no local, parameter or function. A bare
    /// name is never a field (Ruby's rule). Inside a class or struct method
    /// the E0734 is queued for [`Self::attach_field_hints`], which runs
    /// after pass 2, when a parent class declared later in the file has its
    /// fields too.
    pub(super) fn error_undefined_variable(&mut self, name: &str, span: &Span) {
        let type_def = match self.current_self_ty.as_ref() {
            Some(Ty::Class { name, .. } | Ty::Struct { name, .. }) => {
                self.type_registry.get(name).copied()
            }
            _ => None,
        };
        if let Some(type_def) = type_def {
            self.pending_field_hints.push(PendingFieldHint {
                diagnostic: self.diagnostics.len(),
                type_def,
                name: name.to_string(),
                has_self: self.scopes.lookup("self").is_some(),
            });
        }
        self.diagnostics.push(Diagnostic::error_with_code(
            format!("undefined variable `{name}`"),
            span.clone(),
            "E0734",
        ));
    }

    /// Give each queued E0734 whose name is a field of its type, or of an
    /// ancestor class, a help line saying how to read it: `@name` in an
    /// instance method, through an instance in a class method (no `self`).
    pub(super) fn attach_field_hints(&mut self) {
        for hint in std::mem::take(&mut self.pending_field_hints) {
            let Some(owner) = self.field_owner(hint.type_def, &hint.name) else {
                continue;
            };
            let name = &hint.name;
            let help = if hint.has_self {
                format!(
                    "`{name}` is a field of `{owner}`; read it as `@{name}`. \
                     A bare name is a local variable or a method call."
                )
            } else {
                format!(
                    "`{name}` is a field of `{owner}`, but a class method has no instance; \
                     read it from an instance, such as `{}.{name}`.",
                    instance_name(&owner)
                )
            };
            if let Some(diag) = self.diagnostics.get_mut(hint.diagnostic) {
                diag.help.push(help);
            }
        }
    }

    /// The name of the type that declares the field `name`: `type_def`
    /// itself, else its nearest ancestor class. A visited set stops a
    /// malformed parent cycle.
    fn field_owner(&self, type_def: DefId, name: &str) -> Option<String> {
        let mut current = Some(type_def);
        let mut visited = std::collections::HashSet::new();
        while let Some(def_id) = current.filter(|&id| visited.insert(id)) {
            let def = self.symbols.get(def_id)?;
            if self
                .symbols
                .children_of(def_id)
                .any(|c| c.name == name && matches!(c.kind, DefKind::Field { .. }))
            {
                return Some(def.name.clone());
            }
            current = match &def.kind {
                DefKind::Class { info } => info.parent,
                _ => None,
            };
        }
        None
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

/// A variable name for an instance of the type `type_name` in a hint:
/// the last path segment in snake case (`Geo.PointPair` -> `point_pair`).
fn instance_name(type_name: &str) -> String {
    let short = type_name.rsplit('.').next().unwrap_or(type_name);
    let mut out = String::with_capacity(short.len() + 2);
    for (i, ch) in short.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}
