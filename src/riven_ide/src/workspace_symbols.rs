//! `workspace/symbol` — flat, case-insensitive substring search of
//! every named symbol across all open documents.
//!
//! Same HIR walk as `document_symbols`, but emits one
//! `SymbolInformation` per matching symbol with the URI attached. The
//! `container_name` is set so the editor can group `Foo::bar` under
//! its parent.
//!
//! Spec: `docs/requirements/tier3_01_lsp.md` §5.7.

use lsp_types::{Location, SymbolInformation, SymbolKind, Url};

use riven_core::hir::nodes::{
    HirClassDef, HirEnumDef, HirFuncDef, HirImplBlock, HirImplItem, HirItem, HirMixinDef,
    HirMixinItem, HirModule, HirStructDef, HirVariant,
};
use riven_core::lexer::token::Span;

use crate::analysis::AnalysisResult;
use crate::document_symbols::{
    is_synthetic_name, item_belongs_to_source, span_belongs_to_source,
};
use crate::line_index::LineIndex;

/// Search every document in `results` for symbols whose name contains
/// `query` (case-insensitive). An empty `query` matches every named
/// symbol.
pub fn workspace_symbols(
    results: &[(Url, &AnalysisResult)],
    query: &str,
) -> Vec<SymbolInformation> {
    let needle = query.to_lowercase();
    let mut out = Vec::new();
    for (uri, result) in results {
        let Some(program) = result.program.as_ref() else {
            continue;
        };
        let idx = &result.line_index;
        let source_len = result.source.len();
        let mut ctx = WalkCtx {
            out: &mut out,
            uri,
            idx,
            needle: needle.as_str(),
            container: None,
            source_len,
        };
        for item in &program.items {
            if !item_belongs_to_source(item, source_len) {
                continue;
            }
            walk_item(item, &mut ctx);
        }
    }
    out
}

struct WalkCtx<'a> {
    out: &'a mut Vec<SymbolInformation>,
    uri: &'a Url,
    idx: &'a LineIndex,
    needle: &'a str,
    container: Option<&'a str>,
    source_len: usize,
}

fn walk_item(item: &HirItem, ctx: &mut WalkCtx<'_>) {
    match item {
        HirItem::Module(m) => walk_module(m, ctx),
        HirItem::Class(c) => walk_class(c, ctx),
        HirItem::Struct(s) => walk_struct(s, ctx),
        HirItem::Enum(e) => walk_enum(e, ctx),
        HirItem::Mixin(m) => walk_mixin(m, ctx),
        HirItem::Impl(b) => walk_impl(b, ctx),
        HirItem::Function(f) => emit_fn(f, SymbolKind::FUNCTION, ctx),
        HirItem::TypeAlias(a) => emit_named(&a.name, &a.span, SymbolKind::INTERFACE, ctx),
        HirItem::Newtype(n) => emit_named(&n.name, &n.span, SymbolKind::STRUCT, ctx),
        HirItem::Const(c) => emit_named(&c.name, &c.span, SymbolKind::CONSTANT, ctx),
    }
}

fn walk_module(m: &HirModule, ctx: &mut WalkCtx<'_>) {
    emit_named(&m.name, &m.span, SymbolKind::MODULE, ctx);
    if is_synthetic_name(&m.name) || !span_belongs_to_source(&m.span, ctx.source_len) {
        return;
    }
    let mut inner = with_container(ctx, &m.name);
    for item in &m.items {
        walk_item(item, &mut inner);
    }
}

fn walk_class(c: &HirClassDef, ctx: &mut WalkCtx<'_>) {
    emit_named(&c.name, &c.span, SymbolKind::CLASS, ctx);
    let mut inner = with_container(ctx, &c.name);
    for f in &c.fields {
        emit_named(&f.name, &f.span, SymbolKind::FIELD, &mut inner);
    }
    for m in &c.methods {
        emit_fn(m, SymbolKind::METHOD, &mut inner);
    }
    for blk in &c.impl_blocks {
        walk_impl_in(blk, &mut inner);
    }
}

fn walk_struct(s: &HirStructDef, ctx: &mut WalkCtx<'_>) {
    emit_named(&s.name, &s.span, SymbolKind::STRUCT, ctx);
    let mut inner = with_container(ctx, &s.name);
    for f in &s.fields {
        emit_named(&f.name, &f.span, SymbolKind::FIELD, &mut inner);
    }
    for m in &s.methods {
        emit_fn(m, SymbolKind::METHOD, &mut inner);
    }
    for blk in &s.impl_blocks {
        walk_impl_in(blk, &mut inner);
    }
}

fn walk_enum(e: &HirEnumDef, ctx: &mut WalkCtx<'_>) {
    emit_named(&e.name, &e.span, SymbolKind::ENUM, ctx);
    let mut inner = with_container(ctx, &e.name);
    for v in &e.variants {
        walk_variant(v, &mut inner);
    }
    for m in &e.methods {
        emit_fn(m, SymbolKind::METHOD, &mut inner);
    }
    for blk in &e.impl_blocks {
        walk_impl_in(blk, &mut inner);
    }
}

fn walk_variant(v: &HirVariant, ctx: &mut WalkCtx<'_>) {
    emit_named(&v.name, &v.span, SymbolKind::ENUM_MEMBER, ctx);
}

fn walk_mixin(m: &HirMixinDef, ctx: &mut WalkCtx<'_>) {
    emit_named(&m.name, &m.span, SymbolKind::INTERFACE, ctx);
    let mut inner = with_container(ctx, &m.name);
    for item in &m.items {
        match item {
            HirMixinItem::DefaultMethod(f) => emit_fn(f, SymbolKind::METHOD, &mut inner),
            HirMixinItem::MethodSig { name, span, .. } => {
                emit_named(name, span, SymbolKind::METHOD, &mut inner);
            }
            HirMixinItem::AssocType { name, span } => {
                emit_named(name, span, SymbolKind::INTERFACE, &mut inner);
            }
        }
    }
}

fn walk_impl(b: &HirImplBlock, ctx: &mut WalkCtx<'_>) {
    walk_impl_in(b, ctx);
}

fn walk_impl_in(b: &HirImplBlock, ctx: &mut WalkCtx<'_>) {
    for it in &b.items {
        if let HirImplItem::Method(f) = it {
            emit_fn(f, SymbolKind::METHOD, ctx);
        }
    }
}

fn with_container<'a, 'b: 'a>(ctx: &'a mut WalkCtx<'b>, container: &'a str) -> WalkCtx<'a> {
    WalkCtx {
        out: ctx.out,
        uri: ctx.uri,
        idx: ctx.idx,
        needle: ctx.needle,
        container: Some(container),
        source_len: ctx.source_len,
    }
}

fn emit_fn(f: &HirFuncDef, kind: SymbolKind, ctx: &mut WalkCtx<'_>) {
    emit_named(&f.name, &f.span, kind, ctx);
}

fn emit_named(name: &str, span: &Span, kind: SymbolKind, ctx: &mut WalkCtx<'_>) {
    if is_synthetic_name(name) || !span_belongs_to_source(span, ctx.source_len) {
        return;
    }
    if !matches_query(name, ctx.needle) {
        return;
    }
    #[allow(deprecated)] // `deprecated` field is itself #[deprecated]
    ctx.out.push(SymbolInformation {
        name: name.to_string(),
        kind,
        tags: None,
        deprecated: None,
        location: Location {
            uri: ctx.uri.clone(),
            range: ctx.idx.span_to_range(span),
        },
        container_name: ctx.container.map(|s| s.to_string()),
    });
}

fn matches_query(name: &str, needle_lower: &str) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    name.to_lowercase().contains(needle_lower)
}
