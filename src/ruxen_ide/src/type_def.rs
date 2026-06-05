//! LSP `textDocument/typeDefinition` — jump to the declaration site of
//! the type of the expression under the cursor.
//!
//! Spec: `docs/requirements/tier3_01_lsp.md` §5.8.
//!
//! This is a variant of `goto_def`: instead of resolving the identifier
//! at the cursor to its defining `let` / `def` / `class` site, we
//! resolve it to the *type* of the value, then locate the class /
//! struct / enum / mixin that declares that type.
//!
//! Examples:
//! - `let c = Counter.new(...)` — cursor on `c` returns the span of
//!   `class Counter`.
//! - `def main; let r = build_counter(); r ...` — cursor on `r` or the
//!   call returns the span of `class Counter` (the return type of
//!   `build_counter`).
//! - Cursor on `x: Int` returns `None` — primitives have no source.
//! - Cursor on `let v: &Array[T] = ...` peels `&` and `[T]` and returns
//!   the span of `class Array`.

use ruxen_core::hir::types::Ty;
use ruxen_core::resolve::symbols::{DefKind, SymbolTable};

use crate::analysis::AnalysisResult;
use crate::node_finder::{node_at_position, NodeAtPosition};

/// Resolve the type of the expression at `position` to its declaration
/// site. Returns `None` when:
/// - There is no HIR (parse/lex error).
/// - The cursor is not on a typeable node.
/// - The type is a built-in primitive (no source span).
/// - The type's declaration carries a synthetic span (line 0, byte 0..0).
pub fn type_definition(
    result: &AnalysisResult,
    position: lsp_types::Position,
) -> Option<lsp_types::Location> {
    let program = result.program.as_ref()?;
    let symbols = result.symbols.as_ref()?;
    let byte_offset = result.line_index.byte_offset_of(position);
    let node = node_at_position(program, byte_offset)?;

    // Step 1: compute the type "at the cursor".
    let ty = match node {
        NodeAtPosition::VarRef(def_id, _) => symbols.def_ty(def_id)?,
        NodeAtPosition::FnCall { callee, .. } => return_ty_of(symbols, callee)?,
        NodeAtPosition::MethodCall { method, .. } => return_ty_of(symbols, method)?,
        NodeAtPosition::FieldAccess {
            object_ty,
            field_name,
            ..
        } => field_ty(symbols, &object_ty, &field_name)?,
        NodeAtPosition::TypeRef { name, .. } => Ty::Class {
            name,
            generic_args: vec![],
        },
        NodeAtPosition::Definition(def_id, _) => symbols.def_ty(def_id)?,
    };

    // Step 2: peel wrappers (Ref / RefMut / Newtype / Alias) down to a
    // named user-defined type.
    let core = peel_to_named(&ty)?;

    // Step 3: look up the named type in the symbol table.
    let def_id = find_type_def(symbols, &core)?;
    let definition = symbols.get(def_id)?;

    // Skip synthetic built-in spans.
    if is_synthetic_span(
        definition.span.line,
        definition.span.start,
        definition.span.end,
    ) {
        return None;
    }

    let range = result.line_index.span_to_range(&definition.span);
    Some(lsp_types::Location {
        uri: lsp_types::Url::parse("file:///placeholder").unwrap(),
        range,
    })
}

/// The named type the user-visible value carries — already peeled
/// through references, newtypes, and aliases.
struct NamedType {
    name: String,
    kind: NamedKind,
}

#[derive(Copy, Clone)]
enum NamedKind {
    Class,
    Struct,
    Enum,
}

fn return_ty_of(symbols: &SymbolTable, def_id: ruxen_core::hir::nodes::DefId) -> Option<Ty> {
    let def = symbols.get(def_id)?;
    match &def.kind {
        DefKind::Function { signature } => Some(signature.return_ty.clone()),
        DefKind::Method { signature, .. } => Some(signature.return_ty.clone()),
        _ => None,
    }
}

/// Look up a field's declared type given the object's type and the
/// field name. Walks the class/struct's `fields: Vec<DefId>` list.
fn field_ty(symbols: &SymbolTable, object_ty: &Ty, field_name: &str) -> Option<Ty> {
    let core = peel_to_named(object_ty)?;
    let parent_id = find_type_def(symbols, &core)?;
    let field_ids: Vec<_> = match &symbols.get(parent_id)?.kind {
        DefKind::Class { info } => info.fields.clone(),
        DefKind::Struct { info } => info.fields.clone(),
        _ => return None,
    };
    for fid in field_ids {
        let f = symbols.get(fid)?;
        if f.name == field_name {
            if let DefKind::Field { ty, .. } = &f.kind {
                return Some(ty.clone());
            }
        }
    }
    None
}

/// Peel reference, alias, and newtype wrappers off a type until we
/// hit a named (Class/Struct/Enum) carrier — or fail.
///
/// Container generics like `Array[T]`, `Option[T]`, `Result[T, E]`,
/// `Hash[K, V]`, `Set[T]`, `Tuple(...)`, `FixedArray(T, N)` are NOT
/// auto-unwrapped: the spec says `Array[T]` should land on `Array`'s
/// declaration, not on `T`'s. These primitive containers also have no
/// source span, so they return `None` at the lookup step.
fn peel_to_named(ty: &Ty) -> Option<NamedType> {
    match ty {
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner)
        | Ty::RawPtr(inner)
        | Ty::RawPtrMut(inner) => peel_to_named(inner),
        Ty::Alias { target, .. } => peel_to_named(target),
        Ty::Newtype { name, .. } => Some(NamedType {
            name: name.clone(),
            // Newtype is registered in symbols under DefKind::Newtype;
            // the lookup phase tries that too.
            kind: NamedKind::Struct, // hint only; lookup is name-based
        }),
        Ty::Class { name, .. } => Some(NamedType {
            name: name.clone(),
            kind: NamedKind::Class,
        }),
        Ty::Struct { name, .. } => Some(NamedType {
            name: name.clone(),
            kind: NamedKind::Struct,
        }),
        Ty::Enum { name, .. } => Some(NamedType {
            name: name.clone(),
            kind: NamedKind::Enum,
        }),
        // Primitives and built-in containers — no user-source decl.
        _ => None,
    }
}

/// Find a definition in the symbol table that declares the named type.
/// Prefers the `kind` carried on `NamedType` but falls back to any
/// type-declaring kind with the same name so that, e.g., a `Newtype`
/// stored as `Ty::Newtype { name: "Counter" }` resolves even when the
/// peel hint guessed wrong.
fn find_type_def(
    symbols: &SymbolTable,
    named: &NamedType,
) -> Option<ruxen_core::hir::nodes::DefId> {
    let mut fallback: Option<ruxen_core::hir::nodes::DefId> = None;
    for def in symbols.iter() {
        if def.name != named.name {
            continue;
        }
        let matches_hint = matches!(
            (&def.kind, named.kind),
            (DefKind::Class { .. }, NamedKind::Class)
                | (DefKind::Struct { .. }, NamedKind::Struct)
                | (DefKind::Enum { .. }, NamedKind::Enum)
        );
        if matches_hint {
            return Some(def.id);
        }
        if matches!(
            def.kind,
            DefKind::Class { .. }
                | DefKind::Struct { .. }
                | DefKind::Enum { .. }
                | DefKind::Newtype { .. }
                | DefKind::TypeAlias { .. }
                | DefKind::Trait { .. }
        ) && fallback.is_none()
        {
            fallback = Some(def.id);
        }
    }
    fallback
}

fn is_synthetic_span(line: u32, start: usize, end: usize) -> bool {
    line == 0 && start == 0 && end == 0
}
