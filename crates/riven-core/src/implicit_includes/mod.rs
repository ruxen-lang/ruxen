use crate::diagnostics::Diagnostic;
use crate::hir::nodes::{
    HirClassDef, HirEnumDef, HirItem, HirProgram, HirStructDef, HirVariantKind,
};
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::resolve::symbols::{ty_has_derive_trait, ty_is_effectively_copy, SymbolTable};

const SUPPORTED_DERIVES: [&str; 10] = [
    "Debug",
    "Clone",
    "Copy",
    "PartialEq",
    "Eq",
    "Hash",
    "Hashable",
    "Default",
    "Ord",
    "PartialOrd",
];

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

/// Merge the legacy `derive_traits` field with the new in-body
/// `include` directives. Post ruby-naming.spec.md §10a the loud form
/// for opting into a structural mixin is `include <Mixin>` (one per
/// line) inside the type body; the parser routes those into
/// `impl_blocks`, leaving `derive_traits` empty. The validator must
/// see both — under transition some types still carry legacy
/// `derive_traits` entries from the (also retired) attribute-prefix
/// path.
fn collected_derives(
    legacy: &[String],
    impl_blocks: &[crate::hir::nodes::HirImplBlock],
) -> Vec<String> {
    let mut out: Vec<String> = legacy.to_vec();
    for block in impl_blocks {
        if block.negative_trait {
            continue;
        }
        if let Some(ref tref) = block.trait_ref {
            // Only structural mixins go through the auto-synthesis
            // validator. A user-defined mixin (`include Greetable`)
            // gets satisfied by the user's own method definitions and
            // must not be flagged "unknown mixin" by E0608. We filter
            // here so the downstream `validate_common_traits` pass
            // sees only structural names from SUPPORTED_DERIVES.
            if SUPPORTED_DERIVES.contains(&tref.name.as_str()) {
                out.push(tref.name.clone());
            }
            let _ = block.items.is_empty(); // touch fields to avoid warning
            let _ = block.generic_params.len();
            let _ = &block.target_ty;
            let _ = block.is_unsafe;
            let _ = &block.span;
        }
    }
    out
}

fn validate_class(class: &HirClassDef, symbols: &SymbolTable, diags: &mut Vec<Diagnostic>) {
    let derives = collected_derives(&class.derive_traits, &class.impl_blocks);
    validate_common_traits("class", &class.name, &derives, &class.span, diags);

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
    let derives = collected_derives(&strukt.derive_traits, &strukt.impl_blocks);
    validate_common_traits("struct", &strukt.name, &derives, &strukt.span, diags);
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
    let derives = collected_derives(&enm.derive_traits, &enm.impl_blocks);
    validate_common_traits("enum", &enm.name, &derives, &enm.span, diags);

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
    diags: &mut Vec<Diagnostic>,
) {
    for trait_name in derive_traits {
        if !SUPPORTED_DERIVES.contains(&trait_name.as_str()) {
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
///   * `String` / `Str` (the runtime ships `riven_string_from` as the
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
    if matches!(
        ty,
        Ty::Bool | Ty::Char | Ty::Unit | Ty::String | Ty::Str | Ty::Never
    ) {
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
///   * `Hash` / `Hashable` → E0615 if a field is not Hashable.
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
    let want_hash = has_derive(derive_traits, "Hash") || has_derive(derive_traits, "Hashable");
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
        if want_hash && !ty_satisfies_named_trait(field_ty, "Hash", symbols) {
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
/// `trait_name` is one of `"PartialEq" | "Eq" | "Hash" | "Hashable" |
/// "Ord" | "PartialOrd"`. `Eq` collapses to `PartialEq` (Eq is a
/// marker), and `Hash` is treated identically to `Hashable` to keep
/// the TEC-13 rename working through the transition release.
fn ty_satisfies_named_trait(ty: &Ty, trait_name: &str, symbols: &SymbolTable) -> bool {
    let canonical = match trait_name {
        "Eq" => "PartialEq",
        "Hashable" => "Hash",
        other => other,
    };

    if ty.is_integer() || ty.is_float() {
        // Float is technically *not* total-Ord, but the v1 lowering
        // uses identical Compare opcodes for both, so we accept it for
        // the purposes of derive validation. The runtime simply does
        // an IEEE-754 compare.
        return true;
    }
    if matches!(
        ty,
        Ty::Bool | Ty::Char | Ty::Unit | Ty::String | Ty::Str | Ty::Never
    ) {
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
            // Hash accepts either the new name or the legacy spelling;
            // every other trait must match exactly.
            if canonical == "Hash" {
                ty_has_derive_trait(ty, symbols, "Hash")
                    || ty_has_derive_trait(ty, symbols, "Hashable")
            } else {
                ty_has_derive_trait(ty, symbols, canonical)
            }
        }
        _ => false,
    }
}
