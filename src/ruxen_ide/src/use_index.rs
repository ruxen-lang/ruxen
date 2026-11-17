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
//! - **Subsequent entries:** every use-site span, in source order
//!   (sorted by span start after the `hir::visit` walk).
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
    DefId, HirExpr, HirExprKind, HirFuncDef, HirImplBlock, HirImplItem, HirItem, HirMixinItem,
    HirPattern, HirProgram, HirStatement, HirVariantKind, UNRESOLVED_DEF,
};
use ruxen_core::hir::types::Ty;
use ruxen_core::hir::visit::{walk_expr, walk_func, walk_item, walk_pattern, walk_stmt, Visit};
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

    // The walk records a node before or after its children, so put the
    // use-sites (everything after the def-site) back into source order.
    for spans in builder.index.uses.values_mut() {
        spans[1..].sort_by_key(|s| (s.start, s.end));
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

    // ─── Type-reference recording ──────────────────────────────────

    /// Record every named class/struct/enum/newtype/alias that `ty`
    /// mentions as a use at `host_span`.
    ///
    /// `Ty` itself carries no span post-resolve, so the host span (the
    /// surrounding `let`, param, return-type, or field decl) is the
    /// best approximation available. Wave 2 consumers that need a
    /// tighter range can intersect with the source text.
    fn record_ty(&mut self, ty: &Ty, host_span: &Span) {
        let mut named = Vec::new();
        ty.any(&mut |t| {
            if let Ty::Class { name, .. }
            | Ty::Struct { name, .. }
            | Ty::Enum { name, .. }
            | Ty::Newtype { name, .. }
            | Ty::Alias { name, .. } = t
            {
                named.extend(lookup_named_type(self.symbols, name));
            }
            false
        });
        for def_id in named {
            self.record(def_id, host_span.clone());
        }
    }

    /// The types an `extension` / inline `include` block names: its
    /// target and its associated types. `walk_item` visits its methods.
    fn record_impl_block_types(&mut self, b: &HirImplBlock) {
        self.record_ty(&b.target_ty, &b.span);
        for item in &b.items {
            if let HirImplItem::AssocType { ty, span, .. } = item {
                self.record_ty(ty, span);
            }
        }
    }
}

/// Item-level types (fields, variants, signatures) are recorded here;
/// `hir::visit` walks every body. A `Ty` has no span of its own, so
/// `visit_ty` stays the default no-op and each override records its
/// types against the host span.
impl Visit for Builder<'_> {
    fn visit_item(&mut self, item: &HirItem) {
        match item {
            HirItem::Class(c) => {
                if let Some(parent) = c.parent {
                    // The parent class is referenced by inheritance.
                    self.record(parent, c.span.clone());
                }
                for f in &c.fields {
                    self.record_ty(&f.ty, &f.span);
                }
                for b in &c.impl_blocks {
                    self.record_impl_block_types(b);
                }
            }
            HirItem::Struct(s) => {
                for f in &s.fields {
                    self.record_ty(&f.ty, &f.span);
                }
                for b in &s.impl_blocks {
                    self.record_impl_block_types(b);
                }
            }
            HirItem::Enum(e) => {
                for v in &e.variants {
                    match &v.kind {
                        HirVariantKind::Unit => {}
                        HirVariantKind::Tuple(fields) | HirVariantKind::Struct(fields) => {
                            for vf in fields {
                                self.record_ty(&vf.ty, &vf.span);
                            }
                        }
                    }
                }
                for b in &e.impl_blocks {
                    self.record_impl_block_types(b);
                }
            }
            HirItem::Mixin(m) => {
                for item in &m.items {
                    if let HirMixinItem::MethodSig {
                        params,
                        return_ty,
                        span,
                        ..
                    } = item
                    {
                        for p in params {
                            self.record_ty(&p.ty, &p.span);
                        }
                        self.record_ty(return_ty, span);
                    }
                }
            }
            HirItem::Impl(b) => self.record_impl_block_types(b),
            HirItem::TypeAlias(a) => self.record_ty(&a.ty, &a.span),
            HirItem::Newtype(n) => self.record_ty(&n.inner_ty, &n.span),
            HirItem::Const(c) => self.record_ty(&c.ty, &c.value.span),
            HirItem::Module(_) | HirItem::Function(_) => {}
        }
        walk_item(self, item);
    }

    fn visit_func(&mut self, f: &HirFuncDef) {
        for p in &f.params {
            self.record_ty(&p.ty, &p.span);
        }
        self.record_ty(&f.return_ty, &f.span);
        walk_func(self, f);
    }

    fn visit_stmt(&mut self, s: &HirStatement) {
        if let HirStatement::Let { ty, span, .. } = s {
            self.record_ty(ty, span);
        }
        walk_stmt(self, s);
    }

    fn visit_pattern(&mut self, p: &HirPattern) {
        if let HirPattern::Enum { type_def, span, .. } | HirPattern::Struct { type_def, span, .. } =
            p
        {
            self.record(*type_def, span.clone());
        }
        walk_pattern(self, p);
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        match &expr.kind {
            HirExprKind::VarRef(def_id) => self.record(*def_id, expr.span.clone()),
            HirExprKind::FieldAccess {
                object,
                field_name,
                field_idx,
                ..
            } => {
                if let Some(field_def) =
                    resolve_field_def(self.symbols, &object.ty, *field_idx, field_name)
                {
                    self.record(field_def, expr.span.clone());
                }
            }
            // Typeck records the selected method on a class, struct or enum
            // receiver. A builtin-head call (`s.ljust`) stays unrecorded: its
            // def lives in a stdlib file, not in this document.
            HirExprKind::MethodCall { method, .. } => self.record(*method, expr.span.clone()),
            HirExprKind::FnCall { callee, .. } => self.record(*callee, expr.span.clone()),
            HirExprKind::Construct { type_def, .. } => {
                self.record(*type_def, expr.span.clone());
            }
            HirExprKind::EnumVariant {
                type_def,
                variant_idx,
                ..
            } => {
                self.record(*type_def, expr.span.clone());
                if let Some(variant_def) =
                    resolve_variant_def(self.symbols, *type_def, *variant_idx)
                {
                    self.record(variant_def, expr.span.clone());
                }
            }
            HirExprKind::Cast {
                expr: inner,
                target,
            } => self.record_ty(target, &inner.span),
            // Every other kind only nests expressions, which the walk reaches.
            _ => {}
        }
        walk_expr(self, expr);
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
