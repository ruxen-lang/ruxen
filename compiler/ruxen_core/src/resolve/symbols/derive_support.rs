//! The implicit-include rule (ruby-naming.spec.md §3.6): whether a type's
//! fields structurally support a derived mixin.

use super::*;
use crate::hir::types::MAX_TYPE_NODES;

/// Returns `true` when the type should be treated as `Copy` after consulting
/// user-defined `derive Copy` metadata in the symbol table.
pub fn ty_is_effectively_copy(ty: &Ty, symbols: &SymbolTable) -> bool {
    ty.is_copy_with(symbols)
}

/// Walk through transparent wrappers and check whether the underlying
/// user-defined type carries the requested derive trait.
///
/// Post ruby-naming.spec.md §3.6: structural mixins (`Copy`, `Clone`,
/// `Debug`, `Eq`, `Hashable`, `PartialEq`, `Default`, `Ord`, `PartialOrd`)
/// are *implicitly* included when the type's fields structurally
/// support them; this helper applies that implicit-include rule.
pub fn ty_has_derive_trait(ty: &Ty, symbols: &SymbolTable, trait_name: &str) -> bool {
    DeriveWalk::new(symbols, trait_name).nominal(ty)
}

/// One structural-support query: whether every field of a type, through
/// nested types, supports `trait_name`.
///
/// A self-referential type (`class Node { children: Array[Node] }`) would
/// recurse forever, so a nominal type already on the recursion stack
/// (`on_stack`, keyed by its full type so a nested instance of the same
/// class is checked with its own arguments) is assumed to support the
/// mixin (coinductive): a recursive type supports a derive iff its
/// non-recursive parts do, as Rust derives `Clone`/`Debug` on a recursive
/// `enum`.
///
/// Every answer is kept in `memo` for the rest of the query, so each type
/// is walked once however many paths reach it. That is sound because an
/// answer is a conjunction over fields all the way up to the queried type:
/// a "yes" that rests on an assumption about a type still on the stack can
/// only be wrong if that type fails, and its "no" then makes the queried
/// type "no" too. A type larger than [`MAX_TYPE_NODES`] is unsupported, so
/// a type that grows on every step (`class Boom[T]` holding a
/// `Boom[(T, T)]`, which the instance engine rejects with E0746) ends.
struct DeriveWalk<'s> {
    symbols: &'s SymbolTable,
    trait_name: &'s str,
    on_stack: std::collections::HashSet<String>,
    memo: std::collections::HashMap<String, bool>,
}

impl<'s> DeriveWalk<'s> {
    fn new(symbols: &'s SymbolTable, trait_name: &'s str) -> Self {
        DeriveWalk {
            symbols,
            trait_name,
            on_stack: std::collections::HashSet::new(),
            memo: std::collections::HashMap::new(),
        }
    }

    /// Whether the nominal type `ty` (through references, aliases and
    /// newtypes) supports the mixin; any other type does not.
    fn nominal(&mut self, ty: &Ty) -> bool {
        let (name, args) = match ty {
            Ty::Struct {
                name, generic_args, ..
            }
            | Ty::Class {
                name, generic_args, ..
            }
            | Ty::Enum {
                name, generic_args, ..
            } => (name, generic_args),
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner)
            | Ty::Alias { target: inner, .. }
            | Ty::Newtype { inner, .. } => return self.nominal(inner),
            _ => return false,
        };
        if ty.exceeds_nodes(MAX_TYPE_NODES) {
            return false;
        }
        let key = ty.to_string();
        if let Some(&known) = self.memo.get(&key) {
            return known;
        }
        if !self.on_stack.insert(key.clone()) {
            return true;
        }
        let supported = self.declared_fields_support(name, args);
        self.on_stack.remove(&key);
        self.memo.insert(key, supported);
        supported
    }

    /// Whether the type declaration `name` (the one `SymbolTable::type_def`
    /// resolves, so the answer stays a conjunction), instantiated with
    /// `args`, supports the mixin.
    fn declared_fields_support(&mut self, name: &str, args: &[Ty]) -> bool {
        let symbols = self.symbols;
        let trait_name = self.trait_name;
        let Some(def) = symbols.type_def(name) else {
            return false;
        };
        {
            // The instance's arguments substituted into the declared field types.
            let subst: std::collections::HashMap<String, Ty> = def
                .kind
                .generic_params()
                .unwrap_or_default()
                .iter()
                .map(|g| g.name.clone())
                .zip(args.iter().cloned())
                .collect();
            let field_types: Vec<Ty> = match &def.kind {
                DefKind::Class { info } => {
                    // ruby-naming.spec.md §3.6: a `class` never implicitly
                    // includes `Copy` (reference semantics). For all other
                    // structural mixins, treat them as implicitly included
                    // when every field structurally supports them.
                    if trait_name == "Copy" {
                        return false;
                    }
                    // A fieldless class whose methods are C (`lib`) functions
                    // is an opaque handle: a derived body would copy, compare
                    // or print its empty box, not the C object behind it (a
                    // copied `Mutex` locked uninitialized memory). It
                    // supports a mixin only by declaring that mixin's
                    // methods itself (`SharedSync#clone`).
                    if info.fields.is_empty() && is_c_handle(symbols, name, info, trait_name) {
                        return false;
                    }
                    match field_types(symbols, &info.fields) {
                        Some(types) => types,
                        None => return false,
                    }
                }
                DefKind::Struct { info } => match field_types(symbols, &info.fields) {
                    Some(types) => types,
                    None => return false,
                },
                // ruby-naming.spec.md §3.6: structural mixins apply to enums
                // too — Debug always; Clone/Eq/Hash/Ord when every variant
                // field structurally supports them. `Copy` on enums follows
                // the same field rule but is conservative — enums in v1 stay
                // non-Copy unless all variants are unit-only with primitive
                // payloads, so we still gate on the field check.
                DefKind::Enum { info } => match variant_field_types(symbols, &info.variants) {
                    Some(types) => types,
                    None => return false,
                },
                _ => return false,
            };
            field_types
                .iter()
                .all(|ty| self.supports(&ty.subst_params(&subst)))
        }
    }

    /// Whether `ty` structurally supports the mixin. Primitives and
    /// references satisfy everything but `Copy`; containers forward to their
    /// elements; nested nominal types recurse through [`Self::nominal`].
    fn supports(&mut self, ty: &Ty) -> bool {
        if self.trait_name == "Copy" {
            return ty.is_copy_with(self.symbols);
        }
        match ty {
            Ty::Struct { .. } | Ty::Class { .. } | Ty::Enum { .. } => self.nominal(ty),
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner)
            | Ty::Newtype { inner, .. }
            | Ty::Alias { target: inner, .. }
            | Ty::FixedArray(inner, _)
            // Array, Set and Option are containers: they forward the mixin
            // to their element type.
            | Ty::Array(inner)
            | Ty::Set(inner)
            | Ty::Option(inner) => self.supports(inner),
            Ty::Tuple(elems) => elems.iter().all(|e| self.supports(e)),
            Ty::Map(k, v) | Ty::Result(k, v) => self.supports(k) && self.supports(v),
            _ => true,
        }
    }
}

/// Whether a class's methods include a C (`lib`) function and it does not
/// define the method a derive of `trait_name` would itself
/// (`SymbolTable::declares_derived_method`).
fn is_c_handle(symbols: &SymbolTable, name: &str, info: &ClassInfo, trait_name: &str) -> bool {
    let has_c_method = info.methods.iter().any(|&m| {
        matches!(
            symbols.get(m).map(|def| &def.kind),
            Some(DefKind::Method { signature, .. }) if signature.c_symbol.is_some()
        )
    });
    has_c_method && !symbols.declares_derived_method(name, trait_name)
}

/// The declared types of a struct's or class's fields, `None` when a field
/// is not found.
fn field_types(symbols: &SymbolTable, field_def_ids: &[DefId]) -> Option<Vec<Ty>> {
    field_def_ids
        .iter()
        .map(|field_id| match symbols.get(*field_id).map(|d| &d.kind) {
            Some(DefKind::Field { ty, .. }) | Some(DefKind::Variable { ty, .. }) => {
                Some(ty.clone())
            }
            _ => None,
        })
        .collect()
}

/// The declared payload types of every variant of an enum, `None` when a
/// variant is not found.
fn variant_field_types(symbols: &SymbolTable, variant_def_ids: &[DefId]) -> Option<Vec<Ty>> {
    let mut types = Vec::new();
    for vid in variant_def_ids {
        match symbols.get(*vid).map(|d| &d.kind) {
            Some(DefKind::EnumVariant { kind, .. }) => match kind {
                VariantDefKind::Unit => {}
                VariantDefKind::Tuple(payload) => types.extend(payload.iter().cloned()),
                VariantDefKind::Struct(fields) => {
                    types.extend(fields.iter().map(|(_, ty)| ty.clone()))
                }
            },
            _ => return None,
        }
    }
    Some(types)
}
