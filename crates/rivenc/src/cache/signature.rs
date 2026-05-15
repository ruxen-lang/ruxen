//! Public-signature extraction and comparison for dependency-aware invalidation.
//!
//! A file's `FileSignature` captures its **interface** — everything dependents
//! can observe — with function bodies and private items excluded. Two
//! signatures compare equal exactly when the public interface is unchanged.
//!
//! Types are encoded using the `Ty: Display` implementation from `riven-core`,
//! which produces canonical human-readable strings. This avoids adding
//! `serde::Serialize` to the entire HIR type hierarchy while still giving us a
//! structural equality check that's stable across compilations.

use riven_core::hir::nodes::{
    HirFuncDef, HirImplBlock, HirImplItem, HirItem, HirMixinItem, HirProgram,
};
use riven_core::parser::ast::Visibility;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigParam {
    pub name: String,
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigFn {
    pub name: String,
    pub generic_params: Vec<String>,
    pub self_mode: Option<String>,
    pub is_class_method: bool,
    pub params: Vec<SigParam>,
    pub return_ty: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SigField {
    pub name: String,
    pub ty: String,
    pub public: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublicItem {
    Function(SigFn),
    Class {
        name: String,
        generic_params: Vec<String>,
        fields: Vec<SigField>,
        methods: Vec<SigFn>,
    },
    Struct {
        name: String,
        generic_params: Vec<String>,
        fields: Vec<SigField>,
    },
    Enum {
        name: String,
        generic_params: Vec<String>,
        variants: Vec<EnumVariantSig>,
    },
    Trait {
        name: String,
        generic_params: Vec<String>,
        items: Vec<MixinItemSig>,
    },
    TraitImpl {
        trait_name: Option<String>,
        target_ty: String,
        methods: Vec<SigFn>,
    },
    TypeAlias {
        name: String,
        ty: String,
    },
    Newtype {
        name: String,
        inner_ty: String,
    },
    Const {
        name: String,
        ty: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumVariantSig {
    pub name: String,
    /// Canonical description of variant shape (unit/tuple(tys)/struct(fields)).
    pub shape: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MixinItemSig {
    AssocType { name: String },
    Method(SigFn),
}

/// A file's full public signature. Serialized to
/// `incremental/signatures/<source_hash>.sig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSignature {
    pub items: Vec<PublicItem>,
}

impl FileSignature {
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        postcard::to_allocvec(self).map_err(|e| format!("signature serialize: {}", e))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        postcard::from_bytes(bytes).map_err(|e| format!("signature deserialize: {}", e))
    }
}

/// Extract the public signature of a typed HIR program.
pub fn extract(hir: &HirProgram) -> FileSignature {
    let mut items = Vec::new();
    for item in &hir.items {
        extract_item(item, &mut items);
    }
    FileSignature { items }
}

fn extract_item(item: &HirItem, out: &mut Vec<PublicItem>) {
    match item {
        HirItem::Function(f) => {
            if matches!(f.visibility, Visibility::Public) {
                out.push(PublicItem::Function(fn_sig(f)));
            }
        }
        HirItem::Class(c) => {
            // Class field visibility lives on `HirFieldDef::visibility`.
            let fields = c
                .fields
                .iter()
                .map(|fd| SigField {
                    name: fd.name.clone(),
                    ty: fd.ty.to_string(),
                    public: matches!(fd.visibility, Visibility::Public),
                })
                .collect();
            let methods = c
                .methods
                .iter()
                .filter(|m| matches!(m.visibility, Visibility::Public))
                .map(fn_sig)
                .collect();
            out.push(PublicItem::Class {
                name: c.name.clone(),
                generic_params: generic_param_names(&c.generic_params),
                fields,
                methods,
            });
            for ib in &c.impl_blocks {
                out.push(impl_sig(ib));
            }
        }
        HirItem::Struct(s) => {
            let fields = s
                .fields
                .iter()
                .map(|fd| SigField {
                    name: fd.name.clone(),
                    ty: fd.ty.to_string(),
                    public: matches!(fd.visibility, Visibility::Public),
                })
                .collect();
            out.push(PublicItem::Struct {
                name: s.name.clone(),
                generic_params: generic_param_names(&s.generic_params),
                fields,
            });
        }
        HirItem::Enum(e) => {
            let variants = e
                .variants
                .iter()
                .map(|v| EnumVariantSig {
                    name: v.name.clone(),
                    shape: variant_shape(&v.kind),
                })
                .collect();
            out.push(PublicItem::Enum {
                name: e.name.clone(),
                generic_params: generic_param_names(&e.generic_params),
                variants,
            });
        }
        HirItem::Mixin(t) => {
            let tis = t
                .items
                .iter()
                .map(|ti| match ti {
                    HirMixinItem::AssocType { name, .. } => {
                        MixinItemSig::AssocType { name: name.clone() }
                    }
                    HirMixinItem::MethodSig {
                        name,
                        self_mode,
                        is_class_method,
                        params,
                        return_ty,
                        ..
                    } => MixinItemSig::Method(SigFn {
                        name: name.clone(),
                        generic_params: Vec::new(),
                        self_mode: self_mode.map(|m| format!("{:?}", m)),
                        is_class_method: *is_class_method,
                        params: params
                            .iter()
                            .map(|p| SigParam {
                                name: p.name.clone(),
                                ty: p.ty.to_string(),
                            })
                            .collect(),
                        return_ty: return_ty.to_string(),
                    }),
                    HirMixinItem::DefaultMethod(f) => MixinItemSig::Method(fn_sig(f)),
                })
                .collect();
            out.push(PublicItem::Trait {
                name: t.name.clone(),
                generic_params: generic_param_names(&t.generic_params),
                items: tis,
            });
        }
        HirItem::Impl(ib) => {
            out.push(impl_sig(ib));
        }
        HirItem::Module(m) => {
            for sub in &m.items {
                extract_item(sub, out);
            }
        }
        HirItem::TypeAlias(a) => {
            out.push(PublicItem::TypeAlias {
                name: a.name.clone(),
                ty: a.ty.to_string(),
            });
        }
        HirItem::Newtype(n) => {
            out.push(PublicItem::Newtype {
                name: n.name.clone(),
                inner_ty: n.inner_ty.to_string(),
            });
        }
        HirItem::Const(c) => {
            out.push(PublicItem::Const {
                name: c.name.clone(),
                ty: c.ty.to_string(),
            });
        }
    }
}

fn fn_sig(f: &HirFuncDef) -> SigFn {
    SigFn {
        name: f.name.clone(),
        generic_params: generic_param_names(&f.generic_params),
        self_mode: f.self_mode.map(|m| format!("{:?}", m)),
        is_class_method: f.is_class_method,
        params: f
            .params
            .iter()
            .map(|p| SigParam {
                name: p.name.clone(),
                ty: p.ty.to_string(),
            })
            .collect(),
        return_ty: f.return_ty.to_string(),
    }
}

fn impl_sig(ib: &HirImplBlock) -> PublicItem {
    let methods = ib
        .items
        .iter()
        .filter_map(|it| match it {
            HirImplItem::Method(m) => Some(fn_sig(m)),
            HirImplItem::AssocType { .. } => None,
            HirImplItem::Include { .. } => None,
        })
        .collect();
    PublicItem::TraitImpl {
        trait_name: ib.trait_ref.as_ref().map(|r| r.name.clone()),
        target_ty: ib.target_ty.to_string(),
        methods,
    }
}

fn generic_param_names(params: &[riven_core::hir::nodes::HirGenericParam]) -> Vec<String> {
    params
        .iter()
        .map(|g| {
            if g.bounds.is_empty() {
                g.name.clone()
            } else {
                let bounds: Vec<_> = g.bounds.iter().map(|b| b.to_string()).collect();
                format!("{}: {}", g.name, bounds.join(" + "))
            }
        })
        .collect()
}

fn variant_shape(kind: &riven_core::hir::nodes::HirVariantKind) -> String {
    use riven_core::hir::nodes::HirVariantKind;
    match kind {
        HirVariantKind::Unit => "unit".to_string(),
        HirVariantKind::Tuple(fields) => {
            let tys: Vec<_> = fields.iter().map(|f| f.ty.to_string()).collect();
            format!("tuple({})", tys.join(","))
        }
        HirVariantKind::Struct(fields) => {
            let fs: Vec<String> = fields
                .iter()
                .map(|f| {
                    let name = f.name.clone().unwrap_or_default();
                    format!("{}:{}", name, f.ty)
                })
                .collect();
            format!("struct({})", fs.join(","))
        }
    }
}

/// Compare two signatures for structural equality.
///
/// Returns `true` if the public interface has changed — dependents must be
/// recompiled.
pub fn interface_changed(old: &FileSignature, new: &FileSignature) -> bool {
    old != new
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig_with(items: Vec<PublicItem>) -> FileSignature {
        FileSignature { items }
    }

    fn sample_fn(name: &str, return_ty: &str) -> SigFn {
        SigFn {
            name: name.into(),
            generic_params: vec![],
            self_mode: None,
            is_class_method: false,
            params: vec![],
            return_ty: return_ty.into(),
        }
    }

    #[test]
    fn empty_sigs_are_equal() {
        assert!(!interface_changed(
            &FileSignature { items: vec![] },
            &FileSignature { items: vec![] },
        ));
    }

    #[test]
    fn equal_function_sigs_compare_equal() {
        let a = sig_with(vec![PublicItem::Function(sample_fn("foo", "Int"))]);
        let b = sig_with(vec![PublicItem::Function(sample_fn("foo", "Int"))]);
        assert!(!interface_changed(&a, &b));
    }

    #[test]
    fn return_type_change_is_an_interface_change() {
        let a = sig_with(vec![PublicItem::Function(sample_fn("foo", "Int"))]);
        let b = sig_with(vec![PublicItem::Function(sample_fn("foo", "String"))]);
        assert!(interface_changed(&a, &b));
    }

    #[test]
    fn adding_a_function_is_an_interface_change() {
        let a = sig_with(vec![]);
        let b = sig_with(vec![PublicItem::Function(sample_fn("foo", "Int"))]);
        assert!(interface_changed(&a, &b));
    }

    #[test]
    fn adding_a_trait_impl_is_an_interface_change() {
        let a = sig_with(vec![]);
        let b = sig_with(vec![PublicItem::TraitImpl {
            trait_name: Some("Display".into()),
            target_ty: "User".into(),
            methods: vec![sample_fn("fmt", "String")],
        }]);
        assert!(interface_changed(&a, &b));
    }

    #[test]
    fn signature_roundtrips_through_postcard() {
        let sig = sig_with(vec![
            PublicItem::Function(sample_fn("foo", "Int")),
            PublicItem::Struct {
                name: "User".into(),
                generic_params: vec![],
                fields: vec![SigField {
                    name: "id".into(),
                    ty: "Int".into(),
                    public: true,
                }],
            },
        ]);
        let bytes = sig.to_bytes().unwrap();
        let recovered = FileSignature::from_bytes(&bytes).unwrap();
        assert_eq!(sig, recovered);
    }

    #[test]
    fn struct_field_type_change_is_interface_change() {
        let field = |ty: &str| SigField {
            name: "x".into(),
            ty: ty.into(),
            public: true,
        };
        let a = sig_with(vec![PublicItem::Struct {
            name: "S".into(),
            generic_params: vec![],
            fields: vec![field("Int")],
        }]);
        let b = sig_with(vec![PublicItem::Struct {
            name: "S".into(),
            generic_params: vec![],
            fields: vec![field("String")],
        }]);
        assert!(interface_changed(&a, &b));
    }
}
