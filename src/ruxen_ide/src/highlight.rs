//! LSP `textDocument/documentHighlight` — Wave-2 capability per
//! `docs/requirements/tier3_01_lsp.md` §5.6.
//!
//! Resolves the cursor to a `DefId` via `node_finder::node_at_position`,
//! then returns one `DocumentHighlight` for every span recorded in
//! `AnalysisResult::use_index` against that def.
//!
//! ## Read / Write classification (v1 simplification)
//!
//! `UseIndex` records the def-site as the FIRST entry, followed by
//! every use-site. The index does NOT distinguish read vs write at
//! use-sites — an assignment target and a plain reference both appear
//! as bare spans.
//!
//! For v1 we therefore mark **only the def-site as `WRITE`**, and every
//! subsequent use-site as `READ`. This is honest (the declaration IS
//! the original write — `let x = …`, `def fn(…)`, `class Foo`) and
//! avoids a second HIR walk just to find `Assign { target = VarRef(_) }`
//! sites. Editors render both kinds with a subtle colour difference;
//! the much more important contract — "every occurrence is
//! highlighted" — is fully met.
//!
//! Cross-file is the LSP server's concern: this function returns
//! highlights inside whichever file `result` represents, and the
//! caller decides how to scope by URI.
//!
//! Returns an empty `Vec` whenever the cursor doesn't land on a
//! known def — synthetic spans (`start == 0 && end == 0 && line == 0`)
//! are filtered out.

use lsp_types::{DocumentHighlight, DocumentHighlightKind, Position};
use ruxen_core::hir::nodes::{DefId, HirProgram, UNRESOLVED_DEF};
use ruxen_core::hir::types::Ty;
use ruxen_core::lexer::token::Span;
use ruxen_core::resolve::symbols::{DefKind, SymbolTable};

use crate::analysis::AnalysisResult;
use crate::node_finder::{node_at_position, NodeAtPosition};

/// Compute highlights for every occurrence of the def the cursor is on.
///
/// Returns an empty vec when:
/// - The analysis lacks a program / symbols / use-index (early-exit
///   from a lex / parse failure).
/// - The cursor doesn't resolve to any HIR node.
/// - The resolved node references no real def (e.g. a literal, an
///   `UNRESOLVED_DEF` method call we can't recover by name).
pub fn document_highlights(result: &AnalysisResult, position: Position) -> Vec<DocumentHighlight> {
    let Some(program) = result.program.as_ref() else {
        return Vec::new();
    };
    let Some(symbols) = result.symbols.as_ref() else {
        return Vec::new();
    };
    let Some(use_index) = result.use_index.as_ref() else {
        return Vec::new();
    };

    let byte_offset = result.line_index.byte_offset_of(position);
    let Some(node) = node_at_position(program, byte_offset) else {
        return Vec::new();
    };

    let Some(def_id) = def_id_for_node(&node, program, symbols, byte_offset) else {
        return Vec::new();
    };

    let Some(spans) = use_index.uses.get(&def_id) else {
        return Vec::new();
    };

    let mut highlights = Vec::with_capacity(spans.len());
    for (idx, span) in spans.iter().enumerate() {
        if is_synthetic(span) {
            continue;
        }
        let range = result.line_index.span_to_range(span);
        let kind = if idx == 0 {
            DocumentHighlightKind::WRITE
        } else {
            DocumentHighlightKind::READ
        };
        highlights.push(DocumentHighlight {
            range,
            kind: Some(kind),
        });
    }
    highlights
}

// ─── DefId extraction ───────────────────────────────────────────────

/// Map a `NodeAtPosition` to the `DefId` whose use-list we should
/// surface. For nodes where the HIR doesn't carry a resolved def
/// directly (an `UNRESOLVED_DEF` method call, a field access carrying
/// only an `object_ty + field_name`, a type reference by name), we
/// fall back to the same lookup strategy `use_index` uses internally
/// so highlights line up exactly with what `references` would return.
fn def_id_for_node(
    node: &NodeAtPosition,
    program: &HirProgram,
    symbols: &SymbolTable,
    byte_offset: usize,
) -> Option<DefId> {
    match node {
        NodeAtPosition::VarRef(def_id, _) => filter_real(symbols, *def_id),
        NodeAtPosition::Definition(def_id, _) => filter_real(symbols, *def_id),
        NodeAtPosition::FnCall { callee, .. } => filter_real(symbols, *callee),
        NodeAtPosition::MethodCall { method, .. } => {
            if *method != UNRESOLVED_DEF {
                if let Some(id) = filter_real(symbols, *method) {
                    return Some(id);
                }
            }
            // Fallback: walk the HIR, find the innermost MethodCall
            // whose span contains the cursor, then resolve by
            // receiver-type + method-name. Mirrors `signature_help`
            // and `use_index`'s recovery path.
            let (receiver_ty, method_name) =
                crate::node_finder::method_call_at(program, byte_offset)?;
            let resolved = resolve_method_def(symbols, &receiver_ty, &method_name)?;
            filter_real(symbols, resolved)
        }
        NodeAtPosition::FieldAccess {
            object_ty,
            field_name,
            ..
        } => {
            let resolved = resolve_field_def(symbols, object_ty, field_name)?;
            filter_real(symbols, resolved)
        }
        NodeAtPosition::TypeRef { name, .. } => {
            let resolved = lookup_named_type(symbols, name)?;
            filter_real(symbols, resolved)
        }
    }
}

/// Reject `UNRESOLVED_DEF`, synthetic compiler internals (`__`-prefixed
/// names), and zero-span defs. These mirror the `is_real` predicate in
/// `use_index` so we never return a DefId the index wouldn't carry.
fn filter_real(symbols: &SymbolTable, def_id: DefId) -> Option<DefId> {
    if def_id == UNRESOLVED_DEF {
        return None;
    }
    let def = symbols.get(def_id)?;
    if def.name.starts_with("__") {
        return None;
    }
    if is_synthetic(&def.span) {
        return None;
    }
    Some(def_id)
}

fn is_synthetic(span: &Span) -> bool {
    span.start == 0 && span.end == 0 && span.line == 0
}

// ─── DefId resolution helpers (mirrors `use_index`) ─────────────────

/// Resolve a method DefId from the receiver type + method name —
/// mirrors `use_index::resolve_method_def`. Walks past refs / aliases
/// / newtypes to find a concrete class or struct, then scans its
/// method list by name.
fn resolve_method_def(symbols: &SymbolTable, receiver_ty: &Ty, method_name: &str) -> Option<DefId> {
    let ty = peel_ty(receiver_ty);
    let type_name = match ty {
        Ty::Class { name, .. } | Ty::Struct { name, .. } => name.as_str(),
        _ => return None,
    };
    let type_def_id = lookup_named_type(symbols, type_name)?;
    let methods = match &symbols.get(type_def_id)?.kind {
        DefKind::Class { info } => &info.methods,
        // Structs keep methods inside impl blocks — best-effort skip,
        // same as `use_index`. The caller will simply get an empty
        // highlight list, which is honest.
        _ => return None,
    };
    methods.iter().copied().find(|id| {
        symbols
            .get(*id)
            .map(|d| d.name == method_name)
            .unwrap_or(false)
    })
}

/// Resolve a field DefId from the receiver type + field name.
/// Mirrors `use_index::resolve_field_def` but doesn't need a
/// `field_idx` — the `NodeAtPosition::FieldAccess` carries only the
/// name, so we name-scan the class/struct fields directly.
fn resolve_field_def(symbols: &SymbolTable, object_ty: &Ty, field_name: &str) -> Option<DefId> {
    let ty = peel_ty(object_ty);
    let type_name = match ty {
        Ty::Class { name, .. } | Ty::Struct { name, .. } => name.as_str(),
        _ => return None,
    };
    let type_def_id = lookup_named_type(symbols, type_name)?;
    let fields = match &symbols.get(type_def_id)?.kind {
        DefKind::Class { info } => &info.fields,
        DefKind::Struct { info } => &info.fields,
        _ => return None,
    };
    fields.iter().copied().find(|id| {
        symbols
            .get(*id)
            .map(|d| d.name == field_name)
            .unwrap_or(false)
    })
}

/// Peel reference / alias / newtype wrappers to expose the underlying
/// nominal type. Same shape used in `use_index` and `signature_help`.
fn peel_ty(ty: &Ty) -> &Ty {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner)
            | Ty::RawPtr(inner)
            | Ty::RawPtrMut(inner)
            | Ty::Newtype { inner, .. }
            | Ty::Alias { target: inner, .. } => cur = inner,
            _ => return cur,
        }
    }
}

/// Look up a class / struct / enum / newtype / alias by name. Mirrors
/// `use_index::lookup_named_type`.
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
