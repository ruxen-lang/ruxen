//! `textDocument/documentSymbol` — outline view of a single file.
//!
//! Walks `program.items` and produces a hierarchical `DocumentSymbol`
//! tree. Top-level functions/consts/type aliases become leaves;
//! classes/structs/enums/mixins become parents whose children are
//! their methods, fields and variants.
//!
//! Bootstrap-merged stdlib items (Displayable, Error, String, etc.)
//! carry spans pointing into their own `.rvn` source files; filtering
//! by `span.end <= source.len()` drops them so the outline only shows
//! what the user actually typed.
//!
//! Spec: `docs/requirements/tier3_01_lsp.md` §5.7.

use lsp_types::{DocumentSymbol, Range, SymbolKind};

use riven_core::hir::nodes::{
    HirClassDef, HirEnumDef, HirFieldDef, HirFuncDef, HirImplBlock, HirImplItem, HirItem,
    HirMixinDef, HirMixinItem, HirModule, HirStructDef, HirVariant,
};
use riven_core::lexer::token::Span;

use crate::analysis::AnalysisResult;
use crate::line_index::LineIndex;

/// Build the document-outline tree for the analysed file.
///
/// Top-level `impl Foo` blocks have no parent type to nest under at
/// this level, so their methods are surfaced as flat `METHOD` siblings.
/// `impl` blocks nested inside class/struct/enum bodies are folded
/// into the parent type's children via `push_impl_methods`.
pub fn document_symbols(result: &AnalysisResult) -> Vec<DocumentSymbol> {
    let Some(program) = result.program.as_ref() else {
        return Vec::new();
    };
    let line_index = &result.line_index;
    let source_len = result.source.len();
    let mut out = Vec::new();
    for item in &program.items {
        if !item_in_source(item, source_len) {
            continue;
        }
        if let HirItem::Impl(blk) = item {
            push_impl_methods(blk, line_index, &mut out);
            continue;
        }
        if let Some(sym) = symbol_for_item(item, line_index) {
            out.push(sym);
        }
    }
    out
}

/// Crate-visible so `workspace_symbols.rs` reuses the same synth-name
/// rule (`__foo` is treated as compiler-internal).
pub(crate) fn is_synthetic_name(name: &str) -> bool {
    name.starts_with("__")
}

/// `(0, 0, 0, …)` is the conventional Span sentinel for items that
/// have no source location — e.g. compiler-injected helpers.
pub(crate) fn is_synthetic_span(span: &Span) -> bool {
    span.start == 0 && span.end == 0 && span.line == 0
}

/// A top-level item is "in this source" iff its span's end byte fits
/// inside the file. Bootstrap-merged stdlib items have spans that
/// reference their original `.rvn` file (often tens of KB long),
/// which trivially exceed a user's open document.
pub(crate) fn item_belongs_to_source(item: &HirItem, source_len: usize) -> bool {
    item_in_source(item, source_len)
}

/// Same filter for a bare `Span` — used by `workspace_symbols.rs`
/// when emitting children of an in-source parent.
pub(crate) fn span_belongs_to_source(span: &Span, source_len: usize) -> bool {
    !is_synthetic_span(span) && span_within(span, source_len)
}

fn item_in_source(item: &HirItem, source_len: usize) -> bool {
    let span = item_top_span(item);
    !is_synthetic_span(span) && span_within(span, source_len)
}

fn span_within(span: &Span, source_len: usize) -> bool {
    span.end <= source_len
}

fn item_top_span(item: &HirItem) -> &Span {
    match item {
        HirItem::Module(m) => &m.span,
        HirItem::Class(c) => &c.span,
        HirItem::Struct(s) => &s.span,
        HirItem::Enum(e) => &e.span,
        HirItem::Mixin(m) => &m.span,
        HirItem::Impl(b) => &b.span,
        HirItem::Function(f) => &f.span,
        HirItem::TypeAlias(a) => &a.span,
        HirItem::Newtype(n) => &n.span,
        HirItem::Const(c) => &c.span,
    }
}

fn symbol_for_item(item: &HirItem, idx: &LineIndex) -> Option<DocumentSymbol> {
    match item {
        HirItem::Module(m) => symbol_for_module(m, idx),
        HirItem::Class(c) => symbol_for_class(c, idx),
        HirItem::Struct(s) => symbol_for_struct(s, idx),
        HirItem::Enum(e) => symbol_for_enum(e, idx),
        HirItem::Mixin(m) => symbol_for_mixin(m, idx),
        HirItem::Impl(_) => None, // handled at top-level via push_impl_methods
        HirItem::Function(f) => symbol_for_function(f, idx, SymbolKind::FUNCTION),
        HirItem::TypeAlias(a) => {
            if is_synthetic_name(&a.name) || is_synthetic_span(&a.span) {
                return None;
            }
            let range = idx.span_to_range(&a.span);
            Some(leaf(&a.name, SymbolKind::INTERFACE, range))
        }
        HirItem::Newtype(n) => {
            if is_synthetic_name(&n.name) || is_synthetic_span(&n.span) {
                return None;
            }
            let range = idx.span_to_range(&n.span);
            Some(leaf(&n.name, SymbolKind::STRUCT, range))
        }
        HirItem::Const(c) => {
            if is_synthetic_name(&c.name) || is_synthetic_span(&c.span) {
                return None;
            }
            let range = idx.span_to_range(&c.span);
            Some(leaf(&c.name, SymbolKind::CONSTANT, range))
        }
    }
}

fn symbol_for_module(m: &HirModule, idx: &LineIndex) -> Option<DocumentSymbol> {
    if is_synthetic_name(&m.name) || is_synthetic_span(&m.span) {
        return None;
    }
    let mut children = Vec::new();
    for item in &m.items {
        if let Some(sym) = symbol_for_item(item, idx) {
            children.push(sym);
        }
    }
    let range = idx.span_to_range(&m.span);
    Some(parent(&m.name, SymbolKind::MODULE, range, children))
}

fn symbol_for_class(c: &HirClassDef, idx: &LineIndex) -> Option<DocumentSymbol> {
    if is_synthetic_name(&c.name) || is_synthetic_span(&c.span) {
        return None;
    }
    let mut children = Vec::new();
    for f in &c.fields {
        if let Some(sym) = symbol_for_field(f, idx) {
            children.push(sym);
        }
    }
    for m in &c.methods {
        if let Some(sym) = symbol_for_function(m, idx, SymbolKind::METHOD) {
            children.push(sym);
        }
    }
    for blk in &c.impl_blocks {
        push_impl_methods(blk, idx, &mut children);
    }
    let range = idx.span_to_range(&c.span);
    Some(parent(&c.name, SymbolKind::CLASS, range, children))
}

fn symbol_for_struct(s: &HirStructDef, idx: &LineIndex) -> Option<DocumentSymbol> {
    if is_synthetic_name(&s.name) || is_synthetic_span(&s.span) {
        return None;
    }
    let mut children = Vec::new();
    for f in &s.fields {
        if let Some(sym) = symbol_for_field(f, idx) {
            children.push(sym);
        }
    }
    for m in &s.methods {
        if let Some(sym) = symbol_for_function(m, idx, SymbolKind::METHOD) {
            children.push(sym);
        }
    }
    for blk in &s.impl_blocks {
        push_impl_methods(blk, idx, &mut children);
    }
    let range = idx.span_to_range(&s.span);
    Some(parent(&s.name, SymbolKind::STRUCT, range, children))
}

fn symbol_for_enum(e: &HirEnumDef, idx: &LineIndex) -> Option<DocumentSymbol> {
    if is_synthetic_name(&e.name) || is_synthetic_span(&e.span) {
        return None;
    }
    let mut children = Vec::new();
    for v in &e.variants {
        if let Some(sym) = symbol_for_variant(v, idx) {
            children.push(sym);
        }
    }
    for m in &e.methods {
        if let Some(sym) = symbol_for_function(m, idx, SymbolKind::METHOD) {
            children.push(sym);
        }
    }
    for blk in &e.impl_blocks {
        push_impl_methods(blk, idx, &mut children);
    }
    let range = idx.span_to_range(&e.span);
    Some(parent(&e.name, SymbolKind::ENUM, range, children))
}

fn symbol_for_mixin(m: &HirMixinDef, idx: &LineIndex) -> Option<DocumentSymbol> {
    if is_synthetic_name(&m.name) || is_synthetic_span(&m.span) {
        return None;
    }
    let mut children = Vec::new();
    for item in &m.items {
        match item {
            HirMixinItem::DefaultMethod(f) => {
                if let Some(sym) = symbol_for_function(f, idx, SymbolKind::METHOD) {
                    children.push(sym);
                }
            }
            HirMixinItem::MethodSig { name, span, .. } => {
                if is_synthetic_name(name) || is_synthetic_span(span) {
                    continue;
                }
                let range = idx.span_to_range(span);
                children.push(leaf(name, SymbolKind::METHOD, range));
            }
            HirMixinItem::AssocType { name, span } => {
                if is_synthetic_name(name) || is_synthetic_span(span) {
                    continue;
                }
                let range = idx.span_to_range(span);
                children.push(leaf(name, SymbolKind::INTERFACE, range));
            }
        }
    }
    let range = idx.span_to_range(&m.span);
    Some(parent(&m.name, SymbolKind::INTERFACE, range, children))
}

/// Top-level `impl` blocks have no parent type to nest under in the
/// outline. Their methods are exposed as flat `METHOD` entries
/// alongside top-level functions.
fn push_impl_methods(blk: &HirImplBlock, idx: &LineIndex, out: &mut Vec<DocumentSymbol>) {
    for it in &blk.items {
        if let HirImplItem::Method(f) = it {
            if let Some(sym) = symbol_for_function(f, idx, SymbolKind::METHOD) {
                out.push(sym);
            }
        }
    }
}

fn symbol_for_function(
    f: &HirFuncDef,
    idx: &LineIndex,
    kind: SymbolKind,
) -> Option<DocumentSymbol> {
    if is_synthetic_name(&f.name) || is_synthetic_span(&f.span) {
        return None;
    }
    let range = idx.span_to_range(&f.span);
    Some(leaf(&f.name, kind, range))
}

fn symbol_for_field(f: &HirFieldDef, idx: &LineIndex) -> Option<DocumentSymbol> {
    if is_synthetic_name(&f.name) || is_synthetic_span(&f.span) {
        return None;
    }
    let range = idx.span_to_range(&f.span);
    Some(leaf(&f.name, SymbolKind::FIELD, range))
}

fn symbol_for_variant(v: &HirVariant, idx: &LineIndex) -> Option<DocumentSymbol> {
    if is_synthetic_name(&v.name) || is_synthetic_span(&v.span) {
        return None;
    }
    let range = idx.span_to_range(&v.span);
    Some(leaf(&v.name, SymbolKind::ENUM_MEMBER, range))
}

#[allow(deprecated)] // `deprecated` field is itself #[deprecated] in lsp-types 0.94
fn leaf(name: &str, kind: SymbolKind, range: Range) -> DocumentSymbol {
    DocumentSymbol {
        name: name.to_string(),
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: None,
    }
}

#[allow(deprecated)]
fn parent(
    name: &str,
    kind: SymbolKind,
    range: Range,
    children: Vec<DocumentSymbol>,
) -> DocumentSymbol {
    DocumentSymbol {
        name: name.to_string(),
        detail: None,
        kind,
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: if children.is_empty() {
            None
        } else {
            Some(children)
        },
    }
}
