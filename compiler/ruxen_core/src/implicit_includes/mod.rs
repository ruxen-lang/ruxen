use crate::diagnostics::Diagnostic;
use crate::hir::nodes::{
    HirClassDef, HirEnumDef, HirItem, HirProgram, HirStructDef, HirVariantKind,
};
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::resolve::symbols::{ty_has_derive_trait, ty_is_effectively_copy, SymbolTable};

/// The hashing mixin, declared in `std/hash`. The one spelling every phase
/// uses; `Hash` names only the `Hash[K, V]` collection.
pub(crate) const HASHABLE: &str = "Hashable";

const SUPPORTED_DERIVES: [&str; 9] = [
    "Debug",
    "Clone",
    "Copy",
    "PartialEq",
    "Eq",
    HASHABLE,
    "Default",
    "Ord",
    "PartialOrd",
];

/// The derives whose body the compiler synthesises, each with the method it
/// defines. MIR emits the body as `{Type}_{method}`; typeck registers the
/// `.rx` mixin's own `Self`-typed declaration of `method` for the type
/// (`std/core`, `std/hash`). `Debug`'s `to_debug` is the interpolation hook,
/// not a method of `mixin Debug`, so typeck finds nothing to register for it.
pub(crate) const DERIVED_METHODS: [(&str, &str); 7] = [
    ("Debug", "to_debug"),
    ("PartialEq", "eq"),
    (HASHABLE, "hash_code"),
    ("Default", "default"),
    ("Ord", "cmp"),
    ("PartialOrd", "partial_cmp"),
    ("Clone", "clone"),
];

/// The method `trait_name`'s derive defines (see [`DERIVED_METHODS`]), or
/// `None` for a derive with no synthesised body (`Copy`, `Eq`).
pub(crate) fn derived_method(trait_name: &str) -> Option<&'static str> {
    DERIVED_METHODS
        .iter()
        .find(|(t, _)| *t == trait_name)
        .map(|(_, m)| *m)
}

/// Peel references, aliases and newtypes down to the nominal (struct,
/// class or enum) type, or `None` when there is none.
pub(crate) fn peel_to_nominal(ty: &Ty) -> Option<&Ty> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner)
            | Ty::Newtype { inner, .. }
            | Ty::Alias { target: inner, .. } => cur = inner,
            Ty::Struct { .. } | Ty::Class { .. } | Ty::Enum { .. } => return Some(cur),
            _ => return None,
        }
    }
}

/// Does a `{Type}_{method}` body exist for `trait_name`'s derive on the
/// nominal type behind `ty`, either synthesised or written by the user?
///
/// This is the one rule MIR dispatch, the derive synthesis and typeck's
/// derived signatures share:
///
/// * a **struct** gets every derive it qualifies for
///   ([`ty_has_derive_trait`], ruby-naming.spec.md §3.6);
/// * an **enum** gets `Debug` and `Clone` only (its bodies switch on the
///   variant; the other derives have no enum body);
/// * a **class** gets them only at module root (the bodies mangle from the
///   bare name, so a dotted name would name no symbol), and, except for
///   `Clone`, only when it has at least one field, inherited ones included.
///   `fields.all(supports)` is vacuously true for a fieldless class, and the
///   fieldless classes are the primitive method-homes and FFI handle shells
///   (`class Int`, `class Mutex[T]`): a structural body for them is wrong
///   (`Int {}` for a number, `true` for two different mutexes). `Clone`
///   keeps the ungated rule it always had, so an existing `.clone` call on a
///   fieldless class still links.
pub(crate) fn derive_applies(ty: &Ty, symbols: &SymbolTable, trait_name: &str) -> bool {
    let Some(nominal) = peel_to_nominal(ty) else {
        return false;
    };
    if !ty_has_derive_trait(nominal, symbols, trait_name) {
        return false;
    }
    match nominal {
        Ty::Enum { .. } => matches!(trait_name, "Debug" | "Clone"),
        Ty::Class { name, .. } => {
            !name.contains('.') && (trait_name == "Clone" || class_has_fields(name, symbols))
        }
        _ => true,
    }
}

/// Must the compiler synthesise `trait_name`'s derived body for `ty`? True
/// when [`derive_applies`] and the type does not define the method itself:
/// a user `def hash_code` is the body, and a second, synthesised
/// `{Type}_hash_code` would be a duplicate definition.
pub(crate) fn derive_is_synthesized(ty: &Ty, symbols: &SymbolTable, trait_name: &str) -> bool {
    derive_applies(ty, symbols, trait_name) && !declares_derived_method(ty, symbols, trait_name)
}

/// Does the nominal type behind `ty` declare `trait_name`'s derived method
/// (see [`DERIVED_METHODS`]) itself?
fn declares_derived_method(ty: &Ty, symbols: &SymbolTable, trait_name: &str) -> bool {
    let Some(method) = derived_method(trait_name) else {
        return false;
    };
    let Some(Ty::Struct { name, .. } | Ty::Class { name, .. } | Ty::Enum { name, .. }) =
        peel_to_nominal(ty)
    else {
        return false;
    };
    symbols
        .nominal(name)
        .is_some_and(|def| symbols.methods_of(def.id, method).next().is_some())
}

/// Does class `name` have a field of its own or an inherited one?
fn class_has_fields(name: &str, symbols: &SymbolTable) -> bool {
    use crate::resolve::symbols::DefKind;
    let mut cur = symbols.nominal(name);
    while let Some(def) = cur {
        let DefKind::Class { info } = &def.kind else {
            return false;
        };
        if !info.fields.is_empty() {
            return true;
        }
        cur = info.parent.and_then(|p| symbols.get(p));
    }
    false
}

/// Whether `name` is a compiler-synthesizable derive marker (the closed
/// `SUPPORTED_DERIVES` set). Used by `MixinResolver::check_satisfaction` to
/// decide that a zero-required-method bound (`K: Hashable`, `T: Ord`, …) is
/// satisfied STRUCTURALLY via auto-derive, vs a pure-capability marker
/// (`Add`) that must be opted into nominally.
pub(crate) fn is_supported_derive(name: &str) -> bool {
    SUPPORTED_DERIVES.contains(&name)
}

pub fn validate_program(program: &HirProgram, symbols: &SymbolTable) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for item in &program.items {
        validate_item(item, symbols, &mut diags);
    }
    diags
}

fn validate_item(item: &HirItem, symbols: &SymbolTable, diags: &mut Vec<Diagnostic>) {
    match item {
        HirItem::Class(class) => validate_class(class, symbols, diags),
        HirItem::Struct(strukt) => validate_struct(strukt, symbols, diags),
        HirItem::Enum(enm) => validate_enum(enm, symbols, diags),
        HirItem::Module(module) => {
            for sub_item in &module.items {
                validate_item(sub_item, symbols, diags);
            }
        }
        _ => {}
    }
}

/// The structural mixins a type opts into: its in-body `include <Mixin>`
/// directives (ruby-naming.spec.md §10a), which the parser routes into
/// `impl_blocks`.
fn collected_derives(impl_blocks: &[crate::hir::nodes::HirImplBlock]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for block in impl_blocks {
        if block.negative_trait {
            continue;
        }
        if let Some(ref tref) = block.trait_ref {
            out.push(tref.name.clone());
        }
    }
    out
}

/// True if `name` resolves to a user-visible type/trait in the symbol
/// table — i.e. an `include <Name>` directive that names a real,
/// user-defined (or stdlib) mixin/trait/class/struct/enum. Used by
/// E0608 to distinguish a typo (`include WeirdName`) from a legitimate
/// non-structural `include` (`include Display`, `include Greetable`).
fn name_resolves_to_type(symbols: &SymbolTable, name: &str) -> bool {
    symbols.iter().any(|def| {
        def.name == name
            && matches!(
                def.kind,
                crate::resolve::symbols::DefKind::Trait { .. }
                    | crate::resolve::symbols::DefKind::Class { .. }
                    | crate::resolve::symbols::DefKind::Struct { .. }
                    | crate::resolve::symbols::DefKind::Enum { .. }
                    | crate::resolve::symbols::DefKind::Module { .. }
                    | crate::resolve::symbols::DefKind::TypeAlias { .. }
                    | crate::resolve::symbols::DefKind::Newtype { .. }
            )
    })
}

fn validate_class(class: &HirClassDef, symbols: &SymbolTable, diags: &mut Vec<Diagnostic>) {
    let derives = collected_derives(&class.impl_blocks);
    validate_common_traits("class", &class.name, &derives, &class.span, symbols, diags);

    if has_derive(&derives, "Copy") {
        diags.push(Diagnostic::error_with_code(
            format!(
                "Copy cannot be auto-synthesized on class `{}`; use a struct",
                class.name
            ),
            class.span.clone(),
            "E0603",
        ));
    }

    if has_derive(&derives, "Clone") {
        validate_clone_requirements(
            "class",
            &class.name,
            class
                .fields
                .iter()
                .map(|field| (&field.name, &field.ty, &field.span)),
            symbols,
            diags,
        );
    }
    validate_per_field_traits(
        "class",
        &class.name,
        &derives,
        class
            .fields
            .iter()
            .map(|field| (&field.name, &field.ty, &field.span)),
        symbols,
        diags,
    );
}

fn validate_struct(strukt: &HirStructDef, symbols: &SymbolTable, diags: &mut Vec<Diagnostic>) {
    let derives = collected_derives(&strukt.impl_blocks);
    validate_common_traits(
        "struct",
        &strukt.name,
        &derives,
        &strukt.span,
        symbols,
        diags,
    );
    validate_copy_requirements(
        "struct",
        &strukt.name,
        &derives,
        &strukt.span,
        strukt
            .fields
            .iter()
            .map(|field| (&field.name, &field.ty, &field.span)),
        symbols,
        diags,
    );
    if has_derive(&derives, "Clone") {
        validate_clone_requirements(
            "struct",
            &strukt.name,
            strukt
                .fields
                .iter()
                .map(|field| (&field.name, &field.ty, &field.span)),
            symbols,
            diags,
        );
    }
    validate_per_field_traits(
        "struct",
        &strukt.name,
        &derives,
        strukt
            .fields
            .iter()
            .map(|field| (&field.name, &field.ty, &field.span)),
        symbols,
        diags,
    );
}

fn validate_enum(enm: &HirEnumDef, symbols: &SymbolTable, diags: &mut Vec<Diagnostic>) {
    let derives = collected_derives(&enm.impl_blocks);
    validate_common_traits("enum", &enm.name, &derives, &enm.span, symbols, diags);

    if has_derive(&derives, "Default") {
        if enm.variants.is_empty() {
            // Empty enum: no variant could ever be the default. Pin the
            // dedicated B1-reserved code so consumers can distinguish a
            // *missing* `default` directive (E0605) from a *vacuously
            // impossible* Default auto-include on an enum that has zero
            // variants.
            diags.push(Diagnostic::error_with_code(
                format!(
                    "cannot auto-synthesize Default on empty enum `{}` (no variants to pick as default)",
                    enm.name
                ),
                enm.span.clone(),
                "E0616",
            ));
        } else {
            diags.push(Diagnostic::error_with_code(
                format!(
                    "cannot auto-synthesize Default on enum `{}` without a `default` variant",
                    enm.name
                ),
                enm.span.clone(),
                "E0605",
            ));
        }
    }

    if has_derive(&derives, "Copy") && !has_derive(&derives, "Clone") {
        diags.push(Diagnostic::error_with_code(
            "Copy implies Clone — a Copy type must also include Clone",
            enm.span.clone(),
            "E0602",
        ));
    }

    if has_derive(&derives, "Copy") {
        for variant in &enm.variants {
            match &variant.kind {
                HirVariantKind::Unit => {}
                HirVariantKind::Tuple(fields) | HirVariantKind::Struct(fields) => {
                    for (index, field) in fields.iter().enumerate() {
                        if !ty_is_effectively_copy(&field.ty, symbols) {
                            let field_name =
                                field.name.clone().unwrap_or_else(|| index.to_string());
                            diags.push(Diagnostic::error_with_code(
                                format!(
                                    "cannot auto-synthesize Copy on enum `{}` because variant `{}.{}` has non-Copy type `{}`",
                                    enm.name, variant.name, field_name, field.ty
                                ),
                                field.span.clone(),
                                "E0601",
                            ));
                        }
                    }
                }
            }
        }
    }

    if has_derive(&derives, "Clone") {
        for variant in &enm.variants {
            match &variant.kind {
                HirVariantKind::Unit => {}
                HirVariantKind::Tuple(fields) | HirVariantKind::Struct(fields) => {
                    for (index, field) in fields.iter().enumerate() {
                        if !ty_is_effectively_clone(&field.ty, symbols) {
                            let field_name =
                                field.name.clone().unwrap_or_else(|| index.to_string());
                            diags.push(Diagnostic::error_with_code(
                                format!(
                                    "cannot auto-synthesize Clone on enum `{}` because variant `{}.{}` has type `{}` which is not Clone",
                                    enm.name, variant.name, field_name, field.ty
                                ),
                                field.span.clone(),
                                "E0610",
                            ));
                        }
                    }
                }
            }
        }
    }
}

fn validate_common_traits(
    kind: &str,
    name: &str,
    derive_traits: &[String],
    span: &Span,
    symbols: &SymbolTable,
    diags: &mut Vec<Diagnostic>,
) {
    for trait_name in derive_traits {
        if !SUPPORTED_DERIVES.contains(&trait_name.as_str())
            && !name_resolves_to_type(symbols, trait_name)
        {
            diags.push(Diagnostic::error_with_code(
                format!(
                    "unknown mixin `{}` requested for auto-synthesis on {} `{}`",
                    trait_name, kind, name
                ),
                span.clone(),
                "E0608",
            ));
        }
    }

    let mut seen = std::collections::BTreeSet::new();
    for trait_name in derive_traits {
        if !seen.insert(trait_name.as_str()) {
            diags.push(Diagnostic::error_with_code(
                format!(
                    "duplicate include of auto-synthesized mixin `{}` on {} `{}`",
                    trait_name, kind, name
                ),
                span.clone(),
                "E0609",
            ));
        }
    }

    if has_derive(derive_traits, "Eq") && !has_derive(derive_traits, "PartialEq") {
        diags.push(Diagnostic::error_with_code(
            "Eq implies PartialEq — including Eq also requires PartialEq",
            span.clone(),
            "E0604",
        ));
    }

    if has_derive(derive_traits, "Ord")
        && !(has_derive(derive_traits, "Eq") && has_derive(derive_traits, "PartialOrd"))
    {
        diags.push(Diagnostic::error_with_code(
            "Ord implies Eq and PartialOrd — including Ord also requires Eq and PartialOrd",
            span.clone(),
            "E0606",
        ));
    }
}

fn validate_copy_requirements<'a>(
    kind: &str,
    name: &str,
    derive_traits: &[String],
    span: &Span,
    fields: impl Iterator<
        Item = (
            &'a String,
            &'a crate::hir::types::Ty,
            &'a crate::lexer::token::Span,
        ),
    >,
    symbols: &SymbolTable,
    diags: &mut Vec<Diagnostic>,
) {
    if !has_derive(derive_traits, "Copy") {
        return;
    }

    if !has_derive(derive_traits, "Clone") {
        diags.push(Diagnostic::error_with_code(
            "Copy implies Clone — a Copy type must also include Clone",
            span.clone(),
            "E0602",
        ));
    }

    for (field_name, field_ty, field_span) in fields {
        if !ty_is_effectively_copy(field_ty, symbols) {
            diags.push(Diagnostic::error_with_code(
                format!(
                    "cannot auto-synthesize Copy on {} `{}` because field `{}` has non-Copy type `{}`",
                    kind, name, field_name, field_ty
                ),
                field_span.clone(),
                "E0601",
            ));
        }
    }
}

fn has_derive(derive_traits: &[String], trait_name: &str) -> bool {
    derive_traits.iter().any(|name| name == trait_name)
}

/// Emit `E0610` for every field of `kind` `name` whose type does not
/// satisfy `Clone`. Used by `validate_struct` / `validate_class` after
/// the common-trait checks have ruled out the syntactic cases.
fn validate_clone_requirements<'a>(
    kind: &str,
    name: &str,
    fields: impl Iterator<
        Item = (
            &'a String,
            &'a crate::hir::types::Ty,
            &'a crate::lexer::token::Span,
        ),
    >,
    symbols: &SymbolTable,
    diags: &mut Vec<Diagnostic>,
) {
    for (field_name, field_ty, field_span) in fields {
        if !ty_is_effectively_clone(field_ty, symbols) {
            diags.push(Diagnostic::error_with_code(
                format!(
                    "cannot auto-synthesize Clone on {} `{}` because field `{}` has type `{}` which is not Clone",
                    kind, name, field_name, field_ty
                ),
                field_span.clone(),
                "E0610",
            ));
        }
    }
}

/// Whether `ty` is treated as Clone for the purposes of derive
/// synthesis. A type is Clone when it is:
///
///   * a primitive (Int / UInt / Float / Bool / Char / Unit), or
///   * `String` / `Str` (the runtime ships `ruxen_string_from` as the
///     copy primitive), or
///   * a built-in container of Clone elements
///     (`Vec[_]` / `HashMap[_, _]` / `Set[_]` / `Option[_]` /
///     `Result[_, _]` / fixed-size `Array[_; n]`), or
///   * a user-defined struct / class / enum that itself derives Clone,
///     or
///   * a transparent wrapper (`&T`, `&mut T`, alias, newtype) over a
///     Clone type.
///
/// Type parameters are conservatively rejected here because the
/// generated MIR cannot dispatch through a generic Clone bound in v1;
/// the synthesiser emits a concrete `<FieldType>_clone` call. A future
/// patch will lift this when monomorphisation is in place.
fn ty_is_effectively_clone(ty: &Ty, symbols: &SymbolTable) -> bool {
    if ty.is_integer() || ty.is_float() {
        return true;
    }
    if matches!(ty, Ty::Bool | Ty::Char | Ty::Unit | Ty::String | Ty::Never) {
        return true;
    }
    if ty_is_effectively_copy(ty, symbols) {
        return true;
    }
    match ty {
        Ty::Array(inner) | Ty::Set(inner) | Ty::Option(inner) | Ty::FixedArray(inner, _) => {
            ty_is_effectively_clone(inner, symbols)
        }
        Ty::Map(k, v) | Ty::Result(k, v) => {
            ty_is_effectively_clone(k, symbols) && ty_is_effectively_clone(v, symbols)
        }
        Ty::Tuple(elems) => elems
            .iter()
            .all(|elem| ty_is_effectively_clone(elem, symbols)),
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => ty_is_effectively_clone(inner, symbols),
        Ty::Alias { target, .. } => ty_is_effectively_clone(target, symbols),
        Ty::Newtype { inner, .. } => ty_is_effectively_clone(inner, symbols),
        Ty::Struct { .. } | Ty::Class { .. } | Ty::Enum { .. } => {
            ty_has_derive_trait(ty, symbols, "Clone")
        }
        _ => false,
    }
}

/// Walk every field of an aggregate (struct or class) and emit the
/// per-trait field-bound diagnostic for each derived trait that
/// requires per-field bounds:
///
///   * `PartialEq`  → E0613 if a field is not PartialEq.
///   * `Hashable` → E0615 if a field is not Hashable.
///   * `Ord`        → E0617 if a field is not Ord.
///   * `PartialOrd` → E0618 if a field is not PartialOrd.
///
/// Copy and Clone keep their own validators (E0601, E0610) — they
/// predate B1 and have richer messages — so they are deliberately
/// excluded here.
fn validate_per_field_traits<'a>(
    kind: &str,
    name: &str,
    derive_traits: &[String],
    fields: impl Iterator<
        Item = (
            &'a String,
            &'a crate::hir::types::Ty,
            &'a crate::lexer::token::Span,
        ),
    >,
    symbols: &SymbolTable,
    diags: &mut Vec<Diagnostic>,
) {
    let want_partial_eq = has_derive(derive_traits, "PartialEq");
    let want_hash = has_derive(derive_traits, HASHABLE);
    let want_ord = has_derive(derive_traits, "Ord");
    let want_partial_ord = has_derive(derive_traits, "PartialOrd");
    if !(want_partial_eq || want_hash || want_ord || want_partial_ord) {
        return;
    }

    for (field_name, field_ty, field_span) in fields {
        if want_partial_eq && !ty_satisfies_named_trait(field_ty, "PartialEq", symbols) {
            diags.push(Diagnostic::error_with_code(
                format!(
                    "cannot auto-synthesize PartialEq on {} `{}` because field `{}` has type `{}` which is not PartialEq",
                    kind, name, field_name, field_ty
                ),
                field_span.clone(),
                "E0613",
            ));
        }
        if want_hash && !ty_satisfies_named_trait(field_ty, HASHABLE, symbols) {
            diags.push(Diagnostic::error_with_code(
                format!(
                    "cannot auto-synthesize Hash on {} `{}` because field `{}` has type `{}` which is not hashable",
                    kind, name, field_name, field_ty
                ),
                field_span.clone(),
                "E0615",
            ));
        }
        if want_ord && !ty_satisfies_named_trait(field_ty, "Ord", symbols) {
            diags.push(Diagnostic::error_with_code(
                format!(
                    "cannot auto-synthesize Ord on {} `{}` because field `{}` has type `{}` which is not Ord",
                    kind, name, field_name, field_ty
                ),
                field_span.clone(),
                "E0617",
            ));
        }
        if want_partial_ord && !ty_satisfies_named_trait(field_ty, "PartialOrd", symbols) {
            diags.push(Diagnostic::error_with_code(
                format!(
                    "cannot auto-synthesize PartialOrd on {} `{}` because field `{}` has type `{}` which is not PartialOrd",
                    kind, name, field_name, field_ty
                ),
                field_span.clone(),
                "E0618",
            ));
        }
    }
}

/// Generalised "does this type satisfy `<trait_name>` for the purposes
/// of derive synthesis?". Built-in primitives, strings, and the
/// standard containers are accepted; user-defined aggregates must
/// themselves carry the matching `derive`. References / aliases /
/// newtypes are looked through.
///
/// `trait_name` is one of `"PartialEq" | "Eq" | "Hashable" | "Ord" |
/// "PartialOrd"`. `Eq` collapses to `PartialEq` (Eq is a marker).
pub(crate) fn ty_satisfies_named_trait(ty: &Ty, trait_name: &str, symbols: &SymbolTable) -> bool {
    let canonical = match trait_name {
        "Eq" => "PartialEq",
        other => other,
    };

    if ty.is_integer() || ty.is_float() {
        // Float is technically *not* total-Ord, but the v1 lowering
        // uses identical Compare opcodes for both, so we accept it for
        // the purposes of derive validation. The runtime simply does
        // an IEEE-754 compare.
        return true;
    }
    if matches!(ty, Ty::Bool | Ty::Char | Ty::Unit | Ty::String | Ty::Never) {
        return true;
    }

    match ty {
        Ty::Array(inner) | Ty::Set(inner) | Ty::Option(inner) | Ty::FixedArray(inner, _) => {
            ty_satisfies_named_trait(inner, canonical, symbols)
        }
        Ty::Map(k, v) | Ty::Result(k, v) => {
            ty_satisfies_named_trait(k, canonical, symbols)
                && ty_satisfies_named_trait(v, canonical, symbols)
        }
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => ty_satisfies_named_trait(inner, canonical, symbols),
        Ty::Alias { target, .. } => ty_satisfies_named_trait(target, canonical, symbols),
        Ty::Newtype { inner, .. } => ty_satisfies_named_trait(inner, canonical, symbols),
        Ty::Struct { .. } | Ty::Class { .. } | Ty::Enum { .. } => {
            ty_has_derive_trait(ty, symbols, canonical)
        }
        _ => false,
    }
}
