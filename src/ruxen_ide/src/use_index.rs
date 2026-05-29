//! Reverse index from `DefId` to every span where that def is mentioned.
//!
//! Spec: `docs/requirements/tier3_01_lsp.md` §5.2.
//!
//! Built once per analysis run (after typeck succeeds) by a single walk
//! over the HIR. Wave 2 capabilities — `textDocument/references`,
//! `textDocument/documentHighlight`, `textDocument/prepareRename`,
//! `textDocument/rename` — all read this index instead of re-walking
//! the HIR themselves.
//!
//! ## Contract
//!
//! For every "real" `DefId` (a def whose `Definition.span` is non-zero
//! and whose name does NOT start with `__`), `uses[def_id]` contains:
//!
//! - **First entry:** the definition's own span (the name span recorded
//!   in the `SymbolTable`). This lets references-style consumers return
//!   "decl + N uses" with a single contiguous slice.
//! - **Subsequent entries:** every use-site span, appended in HIR
//!   traversal order.
//!
//! Synthetic compiler internals (`__drop`, `__poll`, etc.) are skipped
//! — they're not user-visible and Wave 2 must never offer rename on
//! them.
//!
//! ## Coverage
//!
//! The walker records use-sites for:
//! - `HirExprKind::VarRef(def_id)` — local / param / function-name ref.
//! - `HirExprKind::FnCall { callee, .. }` — function-call callee.
//! - `HirExprKind::MethodCall { method, .. }` — resolved method def.
//! - `HirExprKind::FieldAccess { object, field_name, field_idx }` —
//!   field def, looked up via the object's resolved type → class
//!   `fields[field_idx]`.
//! - `HirExprKind::Construct { type_def, .. }` — class / struct ctor.
//! - `HirExprKind::EnumVariant { type_def, variant_idx, .. }` — both
//!   the enum and the specific variant.
//! - Type references in `let` annotations, params, return types, and
//!   generic args — resolved by name to class/struct/enum/newtype/alias
//!   defs. The use-span is approximated by the enclosing param / let /
//!   return-type span (HIR `Ty` itself carries no span post-resolve).

use std::collections::HashMap;

use ruxen_core::hir::nodes::{
    DefId, HirClassDef, HirConst, HirEnumDef, HirExpr, HirExprKind, HirFieldDef, HirFuncDef,
    HirImplBlock, HirImplItem, HirItem, HirMatchArm, HirMixinDef, HirMixinItem, HirModule,
    HirParam, HirPattern, HirProgram, HirStatement, HirStructDef, HirVariant, UNRESOLVED_DEF,
};
use ruxen_core::hir::types::Ty;
use ruxen_core::lexer::token::Span;
use ruxen_core::resolve::symbols::{DefKind, SymbolTable};

/// Reverse-index of every place each definition is mentioned.
///
/// See module docs for the contract on entry ordering and skipped
/// internals.
#[derive(Debug, Default, Clone)]
pub struct UseIndex {
    pub uses: HashMap<DefId, Vec<Span>>,
}

impl UseIndex {
    /// All recorded spans for `def_id` (def-site + uses), or `&[]` if
    /// the def is unknown / synthetic / skipped.
    pub fn spans_for(&self, def_id: DefId) -> &[Span] {
        self.uses.get(&def_id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Number of recorded spans for `def_id` (decl + uses).
    pub fn count(&self, def_id: DefId) -> usize {
        self.uses.get(&def_id).map(|v| v.len()).unwrap_or(0)
    }
}

/// Build a `UseIndex` from a fully-resolved HIR program and its symbol
/// table. Idempotent — calling this twice on the same inputs produces
/// the same map.
pub fn build_use_index(program: &HirProgram, symbols: &SymbolTable) -> UseIndex {
    let mut builder = Builder {
        symbols,
        index: UseIndex::default(),
    };

    // Pass 1 — seed every "real" def with its own span as the first
    // entry. Walking only the HIR would miss defs that are never used
    // (e.g. a class declared but never instantiated), and tests rely on
    // the def-site being present.
    for def in symbols.iter() {
        if !is_real(&def.name, &def.span) {
            continue;
        }
        builder
            .index
            .uses
            .entry(def.id)
            .or_insert_with(|| vec![def.span.clone()]);
    }

    // Pass 2 — walk the HIR and append every use-site.
    for item in &program.items {
        builder.visit_item(item);
    }

    builder.index
}

// ─── Builder ────────────────────────────────────────────────────────

struct Builder<'a> {
    symbols: &'a SymbolTable,
    index: UseIndex,
}

impl<'a> Builder<'a> {
    /// Append `span` as a use-site of `def_id`, skipping synthetic /
    /// unresolved / zero-span defs. The def-site itself is pre-seeded
    /// in pass 1, so this purely appends.
    fn record(&mut self, def_id: DefId, span: Span) {
        if def_id == UNRESOLVED_DEF {
            return;
        }
        let def = match self.symbols.get(def_id) {
            Some(d) => d,
            None => return,
        };
        if !is_real(&def.name, &def.span) {
            return;
        }
        self.index
            .uses
            .entry(def_id)
            .or_insert_with(|| vec![def.span.clone()])
            .push(span);
    }

    // ─── Items ──────────────────────────────────────────────────────

    fn visit_item(&mut self, item: &HirItem) {
        match item {
            HirItem::Module(m) => self.visit_module(m),
            HirItem::Class(c) => self.visit_class(c),
            HirItem::Struct(s) => self.visit_struct(s),
            HirItem::Enum(e) => self.visit_enum(e),
            HirItem::Mixin(m) => self.visit_mixin(m),
            HirItem::Impl(b) => self.visit_impl_block(b),
            HirItem::Function(f) => self.visit_func(f),
            HirItem::TypeAlias(a) => self.visit_ty(&a.ty, &a.span),
            HirItem::Newtype(n) => self.visit_ty(&n.inner_ty, &n.span),
            HirItem::Const(c) => self.visit_const(c),
        }
    }

    fn visit_module(&mut self, m: &HirModule) {
        for item in &m.items {
            self.visit_item(item);
        }
    }

    fn visit_class(&mut self, c: &HirClassDef) {
        if let Some(parent) = c.parent {
            // The parent class is referenced by inheritance.
            self.record(parent, c.span.clone());
        }
        for f in &c.fields {
            self.visit_field(f);
        }
        for m in &c.methods {
            self.visit_func(m);
        }
        for b in &c.impl_blocks {
            self.visit_impl_block(b);
        }
    }

    fn visit_struct(&mut self, s: &HirStructDef) {
        for f in &s.fields {
            self.visit_field(f);
        }
        for m in &s.methods {
            self.visit_func(m);
        }
        for b in &s.impl_blocks {
            self.visit_impl_block(b);
        }
    }

    fn visit_enum(&mut self, e: &HirEnumDef) {
        for v in &e.variants {
            self.visit_variant(v);
        }
        for m in &e.methods {
            self.visit_func(m);
        }
        for b in &e.impl_blocks {
            self.visit_impl_block(b);
        }
    }

    fn visit_mixin(&mut self, m: &HirMixinDef) {
        for item in &m.items {
            match item {
                HirMixinItem::AssocType { .. } => {}
                HirMixinItem::MethodSig {
                    params,
                    return_ty,
                    span,
                    ..
                } => {
                    for p in params {
                        self.visit_ty(&p.ty, &p.span);
                    }
                    self.visit_ty(return_ty, span);
                }
                HirMixinItem::DefaultMethod(f) => self.visit_func(f),
            }
        }
    }

    fn visit_impl_block(&mut self, b: &HirImplBlock) {
        self.visit_ty(&b.target_ty, &b.span);
        for item in &b.items {
            match item {
                HirImplItem::AssocType { ty, span, .. } => self.visit_ty(ty, span),
                HirImplItem::Method(f) => self.visit_func(f),
                HirImplItem::Include { .. } => {}
            }
        }
    }

    fn visit_func(&mut self, f: &HirFuncDef) {
        for p in &f.params {
            self.visit_param(p);
        }
        self.visit_ty(&f.return_ty, &f.span);
        self.visit_expr(&f.body);
    }

    fn visit_field(&mut self, f: &HirFieldDef) {
        self.visit_ty(&f.ty, &f.span);
    }

    fn visit_variant(&mut self, v: &HirVariant) {
        use ruxen_core::hir::nodes::HirVariantKind;
        match &v.kind {
            HirVariantKind::Unit => {}
            HirVariantKind::Tuple(fields) | HirVariantKind::Struct(fields) => {
                for vf in fields {
                    self.visit_ty(&vf.ty, &vf.span);
                }
            }
        }
    }

    fn visit_param(&mut self, p: &HirParam) {
        self.visit_ty(&p.ty, &p.span);
    }

    fn visit_const(&mut self, c: &HirConst) {
        self.visit_ty(&c.ty, &c.value.span);
        self.visit_expr(&c.value);
    }

    // ─── Statements / Expressions ──────────────────────────────────

    fn visit_stmt(&mut self, s: &HirStatement) {
        match s {
            HirStatement::Let {
                ty,
                value,
                span,
                pattern,
                ..
            } => {
                self.visit_ty(ty, span);
                self.visit_pattern(pattern);
                if let Some(v) = value {
                    self.visit_expr(v);
                }
            }
            HirStatement::Expr(e) => self.visit_expr(e),
        }
    }

    fn visit_pattern(&mut self, p: &HirPattern) {
        match p {
            HirPattern::Binding { .. } | HirPattern::Wildcard { .. } | HirPattern::Rest { .. } => {}
            HirPattern::Literal { expr, .. } => self.visit_expr(expr),
            HirPattern::Tuple { elements, .. }
            | HirPattern::Or {
                patterns: elements, ..
            } => {
                for el in elements {
                    self.visit_pattern(el);
                }
            }
            HirPattern::Enum {
                type_def,
                fields,
                span,
                ..
            } => {
                self.record(*type_def, span.clone());
                for f in fields {
                    self.visit_pattern(f);
                }
            }
            HirPattern::Struct {
                type_def,
                fields,
                span,
                ..
            } => {
                self.record(*type_def, span.clone());
                for (_, sub) in fields {
                    self.visit_pattern(sub);
                }
            }
            HirPattern::Ref { .. } => {}
        }
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        match &expr.kind {
            HirExprKind::IntLiteral(_)
            | HirExprKind::FloatLiteral(_)
            | HirExprKind::StringLiteral(_)
            | HirExprKind::BoolLiteral(_)
            | HirExprKind::CharLiteral(_)
            | HirExprKind::UnitLiteral
            | HirExprKind::NullLiteral
            | HirExprKind::Continue
            | HirExprKind::RegexLiteral { .. }
            | HirExprKind::Error => {}

            HirExprKind::VarRef(def_id) => self.record(*def_id, expr.span.clone()),

            HirExprKind::FieldAccess {
                object,
                field_name,
                field_idx,
            } => {
                self.visit_expr(object);
                if let Some(field_def) =
                    resolve_field_def(self.symbols, &object.ty, *field_idx, field_name)
                {
                    self.record(field_def, expr.span.clone());
                }
            }

            HirExprKind::MethodCall {
                object,
                method,
                method_name,
                args,
                block,
                ..
            } => {
                self.visit_expr(object);
                // Method-call resolution is partially deferred to MIR
                // (see project_ruxen_mir_two_dispatch_paths.md), so
                // `method` is frequently `UNRESOLVED_DEF` at HIR
                // time. Fall back to a name-based lookup against the
                // object's class / struct method list.
                let resolved = if *method == UNRESOLVED_DEF {
                    resolve_method_def(self.symbols, &object.ty, method_name)
                } else {
                    Some(*method)
                };
                if let Some(m) = resolved {
                    self.record(m, expr.span.clone());
                }
                for a in args {
                    self.visit_expr(a);
                }
                if let Some(b) = block {
                    self.visit_expr(b);
                }
            }

            HirExprKind::FnCall { callee, args, .. } => {
                self.record(*callee, expr.span.clone());
                for a in args {
                    self.visit_expr(a);
                }
            }

            HirExprKind::BinaryOp { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            HirExprKind::UnaryOp { operand, .. } => self.visit_expr(operand),
            HirExprKind::Borrow { expr, .. } => self.visit_expr(expr),

            HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
                for s in stmts {
                    self.visit_stmt(s);
                }
                if let Some(t) = tail {
                    self.visit_expr(t);
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
                    self.visit_match_arm(arm);
                }
            }

            HirExprKind::Loop { body } => self.visit_expr(body),
            HirExprKind::While { condition, body } => {
                self.visit_expr(condition);
                self.visit_expr(body);
            }
            HirExprKind::For { iterable, body, .. } => {
                // The For binding's def-site is recorded via the
                // pre-pass over `symbols`. The For node itself
                // doesn't re-use the binding, so we only descend.
                self.visit_expr(iterable);
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

            HirExprKind::Return(opt) | HirExprKind::Break(opt) => {
                if let Some(v) = opt {
                    self.visit_expr(v);
                }
            }

            HirExprKind::Closure { body, .. } => self.visit_expr(body),

            HirExprKind::Construct {
                type_def, fields, ..
            } => {
                self.record(*type_def, expr.span.clone());
                for (_, fv) in fields {
                    self.visit_expr(fv);
                }
            }

            HirExprKind::EnumVariant {
                type_def,
                variant_idx,
                fields,
                ..
            } => {
                self.record(*type_def, expr.span.clone());
                if let Some(variant_def) =
                    resolve_variant_def(self.symbols, *type_def, *variant_idx)
                {
                    self.record(variant_def, expr.span.clone());
                }
                for (_, fv) in fields {
                    self.visit_expr(fv);
                }
            }

            HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
                for e in elems {
                    self.visit_expr(e);
                }
            }

            HirExprKind::MapLiteral(pairs) => {
                for (k, v) in pairs {
                    self.visit_expr(k);
                    self.visit_expr(v);
                }
            }

            HirExprKind::ArrayFill { value, .. } => self.visit_expr(value),

            HirExprKind::Index { object, index } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }

            HirExprKind::Cast { expr, target } => {
                self.visit_expr(expr);
                self.visit_ty(target, &expr.span);
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
                use ruxen_core::hir::nodes::HirInterpolationPart;
                for p in parts {
                    if let HirInterpolationPart::Expr { expr, .. } = p {
                        self.visit_expr(expr);
                    }
                }
            }

            HirExprKind::MacroCall { args, .. } => {
                for a in args {
                    self.visit_expr(a);
                }
            }
        }
    }

    fn visit_match_arm(&mut self, arm: &HirMatchArm) {
        self.visit_pattern(&arm.pattern);
        if let Some(g) = &arm.guard {
            self.visit_expr(g);
        }
        self.visit_expr(&arm.body);
    }

    // ─── Type-reference walk ───────────────────────────────────────

    /// Walk a `Ty` and, for every named class/struct/enum/newtype/alias
    /// reference, find the def in `symbols` and record the enclosing
    /// `host_span` as a use.
    ///
    /// `Ty` itself carries no span post-resolve, so the host span (the
    /// surrounding `let`, param, return-type, or field decl) is the
    /// best approximation available. Wave 2 consumers that need a
    /// tighter range can intersect with the source text.
    fn visit_ty(&mut self, ty: &Ty, host_span: &Span) {
        match ty {
            Ty::Class { name, generic_args }
            | Ty::Struct { name, generic_args }
            | Ty::Enum { name, generic_args } => {
                if let Some(def_id) = lookup_named_type(self.symbols, name) {
                    self.record(def_id, host_span.clone());
                }
                for ga in generic_args {
                    self.visit_ty(ga, host_span);
                }
            }
            Ty::Newtype { name, inner }
            | Ty::Alias {
                name,
                target: inner,
            } => {
                if let Some(def_id) = lookup_named_type(self.symbols, name) {
                    self.record(def_id, host_span.clone());
                }
                self.visit_ty(inner, host_span);
            }
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner)
            | Ty::RawPtr(inner)
            | Ty::RawPtrMut(inner)
            | Ty::Option(inner)
            | Ty::Array(inner)
            | Ty::Set(inner)
            | Ty::FixedArray(inner, _) => self.visit_ty(inner, host_span),
            Ty::Result(a, b) | Ty::Map(a, b) => {
                self.visit_ty(a, host_span);
                self.visit_ty(b, host_span);
            }
            Ty::Tuple(elems) => {
                for e in elems {
                    self.visit_ty(e, host_span);
                }
            }
            Ty::Fn { params, ret } | Ty::FnMut { params, ret } | Ty::FnOnce { params, ret } => {
                for p in params {
                    self.visit_ty(p, host_span);
                }
                self.visit_ty(ret, host_span);
            }
            // Mixin refs are name-only; v1 doesn't chase them — they
            // aren't simple by-name SymbolTable lookups and Wave 2
            // doesn't yet need them.
            Ty::SomeMixin(_) | Ty::AnyMixin(_) => {}
            // No-name primitives / inference / params.
            _ => {}
        }
    }
}

// ─── Helpers ────────────────────────────────────────────────────────

/// A "real" def is one with a non-synthetic span and a non-`__`-prefixed
/// name. Synthetic builtins (`puts` in pre-#06.8 worlds, compiler
/// shims) have `Span { 0, 0, 0, 0 }` and are not user-visible. Names
/// starting with `__` are compiler-internal helpers (`__drop`,
/// `__poll`) — Wave 2 must never offer rename on them.
fn is_real(name: &str, span: &Span) -> bool {
    if name.starts_with("__") {
        return false;
    }
    !(span.start == 0 && span.end == 0 && span.line == 0)
}

/// Resolve a field DefId from the object's type + field index.
///
/// Walks past wrappers (`&T`, `&mut T`, raw pointers, alias / newtype)
/// to find a concrete class or struct, then indexes into its `fields`
/// list. The `field_name` is used as a sanity check — the symbol's
/// name must match.
fn resolve_field_def(
    symbols: &SymbolTable,
    object_ty: &Ty,
    field_idx: usize,
    field_name: &str,
) -> Option<DefId> {
    let mut ty = object_ty;
    while let Ty::Ref(inner)
    | Ty::RefMut(inner)
    | Ty::RefLifetime(_, inner)
    | Ty::RefMutLifetime(_, inner)
    | Ty::RawPtr(inner)
    | Ty::RawPtrMut(inner)
    | Ty::Newtype { inner, .. }
    | Ty::Alias { target: inner, .. } = ty
    {
        ty = inner;
    }
    let name = match ty {
        Ty::Class { name, .. } | Ty::Struct { name, .. } => name.as_str(),
        _ => return None,
    };
    let type_def_id = lookup_named_type(symbols, name)?;
    let fields = match &symbols.get(type_def_id)?.kind {
        DefKind::Class { info } => &info.fields,
        DefKind::Struct { info } => &info.fields,
        _ => return None,
    };
    let candidate = *fields.get(field_idx)?;
    let def = symbols.get(candidate)?;
    if def.name == field_name {
        Some(candidate)
    } else {
        // Fallback name scan within this class's fields, in case
        // field_idx drifted from the resolved type's layout.
        fields.iter().copied().find(|id| {
            symbols
                .get(*id)
                .map(|d| d.name == field_name)
                .unwrap_or(false)
        })
    }
}

/// Resolve a method DefId from the receiver type + method name.
///
/// Used when `HirExprKind::MethodCall::method` is `UNRESOLVED_DEF` —
/// method-call resolution is partially deferred to MIR
/// (`project_ruxen_mir_two_dispatch_paths.md`). For the use-index we
/// can recover the def by scanning the class / struct method list.
fn resolve_method_def(symbols: &SymbolTable, receiver_ty: &Ty, method_name: &str) -> Option<DefId> {
    let mut ty = receiver_ty;
    while let Ty::Ref(inner)
    | Ty::RefMut(inner)
    | Ty::RefLifetime(_, inner)
    | Ty::RefMutLifetime(_, inner)
    | Ty::RawPtr(inner)
    | Ty::RawPtrMut(inner)
    | Ty::Newtype { inner, .. }
    | Ty::Alias { target: inner, .. } = ty
    {
        ty = inner;
    }
    let type_name = match ty {
        Ty::Class { name, .. } | Ty::Struct { name, .. } => name.as_str(),
        _ => return None,
    };
    let type_def_id = lookup_named_type(symbols, type_name)?;
    let methods = match &symbols.get(type_def_id)?.kind {
        DefKind::Class { info } => &info.methods,
        DefKind::Struct { info } => {
            return resolve_struct_method(symbols, &info.fields, method_name)
        }
        _ => return None,
    };
    methods.iter().copied().find(|id| {
        symbols
            .get(*id)
            .map(|d| d.name == method_name)
            .unwrap_or(false)
    })
}

/// Structs don't carry a `methods: Vec<DefId>` list — they stash
/// methods in their `impl_blocks`. Resolving here is best-effort; the
/// LSP can still surface the def-site via SymbolTable.get if Wave 2
/// needs it.
fn resolve_struct_method(_symbols: &SymbolTable, _fields: &[DefId], _name: &str) -> Option<DefId> {
    None
}

/// Resolve an enum variant's DefId by parent enum + variant index.
fn resolve_variant_def(
    symbols: &SymbolTable,
    enum_def: DefId,
    variant_idx: usize,
) -> Option<DefId> {
    let info = match &symbols.get(enum_def)?.kind {
        DefKind::Enum { info } => info,
        _ => return None,
    };
    info.variants.get(variant_idx).copied()
}

/// Look up a class / struct / enum / newtype / alias / mixin by name.
///
/// Mirrors the `find_type_def` pattern in `type_def.rs` — name-based
/// scan with a preference for type-declaring kinds.
fn lookup_named_type(symbols: &SymbolTable, name: &str) -> Option<DefId> {
    let mut fallback: Option<DefId> = None;
    for def in symbols.iter() {
        if def.name != name {
            continue;
        }
        match def.kind {
            DefKind::Class { .. } | DefKind::Struct { .. } | DefKind::Enum { .. } => {
                return Some(def.id);
            }
            DefKind::Newtype { .. } | DefKind::TypeAlias { .. } | DefKind::Trait { .. } => {
                if fallback.is_none() {
                    fallback = Some(def.id);
                }
            }
            _ => {}
        }
    }
    fallback
}
