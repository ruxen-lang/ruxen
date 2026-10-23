//! The implicit-include rule (ruby-naming.spec.md §3.6): whether a type's
//! fields structurally support a derived mixin.

use super::*;

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
    ty_has_derive_trait_seen(
        ty,
        symbols,
        trait_name,
        &mut std::collections::HashSet::new(),
    )
}

/// Cycle-aware core of [`ty_has_derive_trait`]. `seen` holds the nominal
/// type names currently on the structural-support recursion stack. A
/// self-referential type (`class Node { children: Array[Node] }`) would
/// otherwise recurse forever — `ty_has_derive_trait(Node)` →
/// `Array[Node]` field → `ty_has_derive_trait(Node)` → … (Q8 stack
/// overflow). On revisiting a name we return `true` (coinductive): a
/// recursive type structurally supports a derive iff its NON-recursive
/// parts do, exactly like Rust deriving `Clone`/`Debug` on a recursive
/// `enum`. The non-recursive fields are still checked on the first visit,
/// so a genuinely unsupported field still fails (`.all()`/`&&` below).
/// How many nested instances [`ty_has_derive_trait_seen`] checks before it
/// assumes support (coinductively, like a revisited type).
const MAX_DERIVE_DEPTH: usize = 32;

fn ty_has_derive_trait_seen(
    ty: &Ty,
    symbols: &SymbolTable,
    trait_name: &str,
    seen: &mut std::collections::HashSet<String>,
) -> bool {
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
        | Ty::RefMutLifetime(_, inner) => {
            return ty_has_derive_trait_seen(inner, symbols, trait_name, seen)
        }
        Ty::Alias { target, .. } => {
            return ty_has_derive_trait_seen(target, symbols, trait_name, seen)
        }
        Ty::Newtype { inner, .. } => {
            return ty_has_derive_trait_seen(inner, symbols, trait_name, seen)
        }
        _ => return false,
    };

    // Cycle: this nominal type is already being evaluated higher up the
    // stack. Assume it supports the trait (coinductive); the real
    // determination happens at the first (outermost) visit.
    // An instance (`Pair[Int, Mutex[Int]]`) is keyed by its full type, so a
    // nested instance of the same class is checked with its own arguments;
    // the depth cap keeps an expanding instantiation finite.
    if seen.len() > MAX_DERIVE_DEPTH || !seen.insert(ty.to_string()) {
        return true;
    }

    symbols.iter().any(|def| {
        if def.name != *name {
            return false;
        }
        // The instance's arguments substituted into the declared field types.
        let subst: std::collections::HashMap<String, Ty> = def
            .kind
            .generic_params()
            .unwrap_or_default()
            .iter()
            .map(|g| g.name.clone())
            .zip(args.iter().cloned())
            .collect();
        let seen = &mut *seen;
        match &def.kind {
            DefKind::Class { info } => {
                // ruby-naming.spec.md §3.6: a `class` never implicitly
                // includes `Copy` (reference semantics). For all other
                // structural mixins, treat them as implicitly included
                // when every field structurally supports them.
                if trait_name == "Copy" {
                    return false;
                }
                // A fieldless class whose methods are C (`lib`) functions is
                // an opaque handle: a derived body would copy, compare or
                // print its empty box, not the C object behind it (a copied
                // `Mutex` locked uninitialized memory). It supports a mixin
                // only by declaring that mixin's methods itself
                // (`SharedSync#clone`).
                if info.fields.is_empty() && is_c_handle(symbols, name, info, trait_name) {
                    return false;
                }
                struct_or_class_fields_all_support(symbols, &info.fields, &subst, trait_name, seen)
            }
            DefKind::Struct { info } => {
                struct_or_class_fields_all_support(symbols, &info.fields, &subst, trait_name, seen)
            }
            DefKind::Enum { info } => {
                // ruby-naming.spec.md §3.6: structural mixins apply to
                // enums too — Debug always; Clone/Eq/Hash/Ord when
                // every variant field structurally supports them.
                // `Copy` on enums follows the same field rule but is
                // conservative — enums in v1 stay non-Copy unless all
                // variants are unit-only with primitive payloads, so
                // we still gate on the field check.
                enum_variant_fields_all_support(symbols, &info.variants, &subst, trait_name, seen)
            }
            _ => false,
        }
    })
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

/// Helper for `ty_has_derive_trait`: returns true when every field's
/// type structurally supports the requested mixin per the
/// spec §3.6 implicit-include rule.
fn enum_variant_fields_all_support(
    symbols: &SymbolTable,
    variant_def_ids: &[DefId],
    subst: &std::collections::HashMap<String, Ty>,
    trait_name: &str,
    seen: &mut std::collections::HashSet<String>,
) -> bool {
    variant_def_ids.iter().all(|vid| {
        let def = match symbols.get(*vid) {
            Some(d) => d,
            None => return false,
        };
        match &def.kind {
            DefKind::EnumVariant { kind, .. } => match kind {
                VariantDefKind::Unit => true,
                VariantDefKind::Tuple(types) => types.iter().all(|ty| {
                    ty_supports_structural_mixin(&ty.subst_params(subst), symbols, trait_name, seen)
                }),
                VariantDefKind::Struct(fields) => fields.iter().all(|(_, ty)| {
                    ty_supports_structural_mixin(&ty.subst_params(subst), symbols, trait_name, seen)
                }),
            },
            _ => false,
        }
    })
}

fn struct_or_class_fields_all_support(
    symbols: &SymbolTable,
    field_def_ids: &[DefId],
    subst: &std::collections::HashMap<String, Ty>,
    trait_name: &str,
    seen: &mut std::collections::HashSet<String>,
) -> bool {
    field_def_ids
        .iter()
        .all(|field_id| match symbols.get(*field_id).map(|d| &d.kind) {
            Some(DefKind::Field { ty, .. }) | Some(DefKind::Variable { ty, .. }) => {
                ty_supports_structural_mixin(&ty.subst_params(subst), symbols, trait_name, seen)
            }
            _ => false,
        })
}

/// Returns true when the given type structurally supports the named
/// structural mixin. Primitives & references satisfy everything; nested
/// nominal types delegate back to `ty_has_derive_trait`.
fn ty_supports_structural_mixin(
    ty: &Ty,
    symbols: &SymbolTable,
    trait_name: &str,
    seen: &mut std::collections::HashSet<String>,
) -> bool {
    if trait_name == "Copy" {
        return ty.is_copy_with(symbols);
    }
    // For every other structural mixin (Clone, Debug, Eq, Hash, …),
    // primitives and references satisfy unconditionally; user types
    // recurse through `ty_has_derive_trait_seen` (which carries the
    // cycle-detection `seen` set for self-referential types — Q8).
    match ty {
        Ty::Struct { .. } | Ty::Class { .. } | Ty::Enum { .. } => {
            ty_has_derive_trait_seen(ty, symbols, trait_name, seen)
        }
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => {
            ty_supports_structural_mixin(inner, symbols, trait_name, seen)
        }
        Ty::Newtype { inner, .. } | Ty::Alias { target: inner, .. } => {
            ty_supports_structural_mixin(inner, symbols, trait_name, seen)
        }
        Ty::Tuple(elems) => elems
            .iter()
            .all(|e| ty_supports_structural_mixin(e, symbols, trait_name, seen)),
        Ty::FixedArray(elem, _) => ty_supports_structural_mixin(elem, symbols, trait_name, seen),
        // Array, Map, Set, Option, Result are containers — they
        // forward the structural mixin to their element type.
        Ty::Array(elem) | Ty::Set(elem) | Ty::Option(elem) => {
            ty_supports_structural_mixin(elem, symbols, trait_name, seen)
        }
        Ty::Map(k, v) | Ty::Result(k, v) => {
            ty_supports_structural_mixin(k, symbols, trait_name, seen)
                && ty_supports_structural_mixin(v, symbols, trait_name, seen)
        }
        _ => true,
    }
}
