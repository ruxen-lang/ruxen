//! Symbol table — stores all definitions in the program.
//!
//! Every named entity (variable, function, class, field, etc.) gets a unique
//! DefId and a corresponding Definition entry in the symbol table.

use crate::hir::nodes::{DefId, HirSelfMode};
use crate::hir::types::{MixinRef, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;

/// The signature of a function or method.
#[derive(Debug, Clone)]
pub struct FnSignature {
    pub self_mode: Option<HirSelfMode>,
    pub is_class_method: bool,
    pub is_async: bool,
    pub generic_params: Vec<GenericParamInfo>,
    pub params: Vec<ParamInfo>,
    pub return_ty: Ty,
    /// #06.8 Phase 2: the C-symbol alias declared via
    /// `lib "X" def name as "<c-symbol>"(...) end`. `Some(c)` means the
    /// linker will resolve this function to symbol `c`; `None` means the
    /// Riven name *is* the linked C symbol (the historical default for
    /// `lib` blocks). Non-FFI functions always carry `None`.
    pub c_symbol: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GenericParamInfo {
    pub name: String,
    pub bounds: Vec<MixinRef>,
    /// Tier-2 const generics (T2.02 S5): tracks whether this slot
    /// holds a type parameter or a const parameter.  Default `Type`
    /// keeps every pre-S5 construction site backwards-compatible.
    pub kind: GenericParamKind,
}

/// Tier-2 const generics: kind of a declared generic parameter.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum GenericParamKind {
    #[default]
    Type,
    Const {
        ty: Ty,
    },
}

impl GenericParamInfo {
    /// Backwards-compatible constructor for type generic params.
    pub fn type_param(name: String, bounds: Vec<MixinRef>) -> Self {
        Self {
            name,
            bounds,
            kind: GenericParamKind::Type,
        }
    }

    /// Stage 5: constructor for a const generic parameter.
    pub fn const_param(name: String, ty: Ty) -> Self {
        Self {
            name,
            bounds: vec![],
            kind: GenericParamKind::Const { ty },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: String,
    pub ty: Ty,
    pub auto_assign: bool,
}

/// Information about a class definition.
#[derive(Debug, Clone)]
pub struct ClassInfo {
    pub generic_params: Vec<GenericParamInfo>,
    pub parent: Option<DefId>,
    pub fields: Vec<DefId>,
    pub methods: Vec<DefId>,
    pub derive_traits: Vec<String>,
    pub opt_out_send: bool,
    pub opt_out_sync: bool,
    pub manual_send: bool,
    pub manual_sync: bool,
    /// T2.02 S9: where-clause const predicates (`where N > 0`,
    /// `where N + M == 8`).  Evaluated at instantiation time
    /// against the binding map; failing predicates emit E0706.
    /// Default empty for backwards compatibility.
    pub const_predicates: Vec<HirConstPredicate>,
    /// #06.8 T0c: set when the class body carried a
    /// `layout flat_heap_struct` directive. Marker only in Wave 1 —
    /// the link-time layout-mismatch check (E0724) is reserved but
    /// not yet emitted. Will be consumed by the runtime-ABI pin
    /// check once a real stdlib class is migrated to user-source
    /// `.rvn` and gates a `riven_<class>_layout_check` symbol.
    pub flat_heap_struct: bool,
}

/// Information about a struct definition.
#[derive(Debug, Clone)]
pub struct StructInfo {
    pub generic_params: Vec<GenericParamInfo>,
    pub fields: Vec<DefId>,
    pub derive_traits: Vec<String>,
    pub layout: Vec<String>,
    pub opt_out_send: bool,
    pub opt_out_sync: bool,
    pub manual_send: bool,
    pub manual_sync: bool,
    /// T2.02 S9: see `ClassInfo::const_predicates`.
    pub const_predicates: Vec<HirConstPredicate>,
}

/// HIR-level lowered where-clause const predicate.
///
/// The parser's `ast::ConstPredicate` holds a raw `Expr` tree
/// (full comparison + arithmetic + identifier surface).  At resolve
/// time we lower the recognised shape into this compact form so
/// evaluation at instantiation is a straight binary comparison over
/// two `ConstExpr` sub-trees.
///
/// Unsupported shapes lower to a sentinel that evaluates to false
/// at every instantiation; users see E0706 with the unsupported
/// form's span.
#[derive(Debug, Clone, PartialEq)]
pub struct HirConstPredicate {
    pub lhs: crate::hir::types::ConstExpr,
    pub op: ConstPredOp,
    pub rhs: crate::hir::types::ConstExpr,
    pub span: crate::lexer::token::Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstPredOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Information about an enum definition.
#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub generic_params: Vec<GenericParamInfo>,
    pub variants: Vec<DefId>,
    pub derive_traits: Vec<String>,
    pub opt_out_send: bool,
    pub opt_out_sync: bool,
    pub manual_send: bool,
    pub manual_sync: bool,
    /// T2.02 S9: see `ClassInfo::const_predicates`.
    pub const_predicates: Vec<HirConstPredicate>,
}

/// Information about a trait definition.
#[derive(Debug, Clone)]
pub struct MixinInfo {
    pub generic_params: Vec<GenericParamInfo>,
    pub super_traits: Vec<MixinRef>,
    pub required_methods: Vec<String>,
    pub default_methods: Vec<String>,
    pub assoc_types: Vec<String>,
}

/// The kind of definition — what this name refers to.
#[derive(Debug, Clone)]
pub enum DefKind {
    Variable {
        mutable: bool,
        ty: Ty,
    },
    Function {
        signature: FnSignature,
    },
    Class {
        info: ClassInfo,
    },
    Struct {
        info: StructInfo,
    },
    Enum {
        info: EnumInfo,
    },
    EnumVariant {
        parent: DefId,
        variant_idx: usize,
        kind: VariantDefKind,
    },
    Trait {
        info: MixinInfo,
    },
    TypeAlias {
        target: Ty,
    },
    Newtype {
        inner: Ty,
    },
    TypeParam {
        bounds: Vec<MixinRef>,
    },
    /// Tier-2 const generics (T2.02 stage 3).
    ///
    /// Registered when the resolver encounters
    /// `GenericParam::Const { name, ty, .. }`.  `ty` is the resolved
    /// `Ty` of the annotation (typically `Ty::USize` / `Ty::Int` /
    /// `Ty::Bool`).  Stage 5 will use this to validate const-arg
    /// types at use sites and emit E0701 when the wrong primitive
    /// is passed.
    ConstParam {
        ty: Ty,
    },
    Module {
        items: Vec<DefId>,
    },
    Field {
        parent: DefId,
        ty: Ty,
        index: usize,
    },
    Method {
        parent: DefId,
        signature: FnSignature,
    },
    Const {
        ty: Ty,
    },
    /// A parameter in a function or closure
    Param {
        ty: Ty,
        auto_assign: bool,
    },
    /// Self reference inside a class/impl
    SelfValue {
        ty: Ty,
    },
}

/// Kind of enum variant (for construction checking).
#[derive(Debug, Clone)]
pub enum VariantDefKind {
    Unit,
    Tuple(Vec<Ty>),
    Struct(Vec<(String, Ty)>),
}

/// A single definition in the symbol table.
#[derive(Debug, Clone)]
pub struct Definition {
    pub id: DefId,
    pub name: String,
    pub kind: DefKind,
    pub visibility: Visibility,
    pub span: Span,
}

/// The symbol table: stores all definitions indexed by DefId.
#[derive(Debug)]
pub struct SymbolTable {
    definitions: Vec<Definition>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            definitions: Vec::new(),
        }
    }

    /// Allocate a new definition and return its DefId.
    pub fn define(
        &mut self,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
    ) -> DefId {
        let id = self.definitions.len() as DefId;
        self.definitions.push(Definition {
            id,
            name,
            kind,
            visibility,
            span,
        });
        id
    }

    /// Look up a definition by DefId.
    pub fn get(&self, id: DefId) -> Option<&Definition> {
        self.definitions.get(id as usize)
    }

    /// Get a mutable reference to a definition.
    pub fn get_mut(&mut self, id: DefId) -> Option<&mut Definition> {
        self.definitions.get_mut(id as usize)
    }

    /// Get the total number of definitions.
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Check if the symbol table is empty.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Iterate over all definitions.
    pub fn iter(&self) -> impl Iterator<Item = &Definition> {
        self.definitions.iter()
    }

    /// Update the type of a variable/field/param definition.
    pub fn update_ty(&mut self, id: DefId, new_ty: Ty) {
        if let Some(def) = self.definitions.get_mut(id as usize) {
            match &mut def.kind {
                DefKind::Variable { ty, .. } => *ty = new_ty,
                DefKind::Field { ty, .. } => *ty = new_ty,
                DefKind::Param { ty, .. } => *ty = new_ty,
                DefKind::SelfValue { ty } => *ty = new_ty,
                DefKind::Const { ty } => *ty = new_ty,
                DefKind::ConstParam { ty } => *ty = new_ty,
                _ => {}
            }
        }
    }

    /// Get the type associated with a definition, if applicable.
    pub fn def_ty(&self, id: DefId) -> Option<Ty> {
        self.get(id).and_then(|def| match &def.kind {
            DefKind::Variable { ty, .. } => Some(ty.clone()),
            DefKind::Field { ty, .. } => Some(ty.clone()),
            DefKind::Param { ty, .. } => Some(ty.clone()),
            DefKind::SelfValue { ty } => Some(ty.clone()),
            DefKind::Const { ty } => Some(ty.clone()),
            DefKind::ConstParam { ty } => Some(ty.clone()),
            DefKind::Function { signature } => Some(Ty::Fn {
                params: signature.params.iter().map(|p| p.ty.clone()).collect(),
                ret: Box::new(if signature.is_async {
                    Ty::Class {
                        name: "Future".to_string(),
                        generic_args: vec![signature.return_ty.clone()],
                    }
                } else {
                    signature.return_ty.clone()
                }),
            }),
            DefKind::Method { signature, .. } => Some(Ty::Fn {
                params: signature.params.iter().map(|p| p.ty.clone()).collect(),
                ret: Box::new(if signature.is_async {
                    Ty::Class {
                        name: "Future".to_string(),
                        generic_args: vec![signature.return_ty.clone()],
                    }
                } else {
                    signature.return_ty.clone()
                }),
            }),
            _ => None,
        })
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` when the type should be treated as `Copy` after consulting
/// user-defined `derive Copy` metadata in the symbol table.
pub fn ty_is_effectively_copy(ty: &Ty, symbols: &SymbolTable) -> bool {
    ty.is_copy_with(symbols)
}

/// Walk through transparent wrappers and check whether the underlying
/// user-defined type carries the requested derive trait.
///
/// Post ruby-naming.spec.md §3.6: structural mixins (`Copy`, `Clone`,
/// `Debug`, `Eq`, `Hash`, `PartialEq`, `Default`, `Ord`, `PartialOrd`)
/// are *implicitly* included when the type's fields structurally
/// support them. This helper checks the explicit `derive_traits` first
/// for backwards compatibility, then falls back to the spec's
/// implicit-include rule when applicable.
pub fn ty_has_derive_trait(ty: &Ty, symbols: &SymbolTable, trait_name: &str) -> bool {
    let name = match ty {
        Ty::Struct { name, .. } | Ty::Class { name, .. } | Ty::Enum { name, .. } => name,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => return ty_has_derive_trait(inner, symbols, trait_name),
        Ty::Alias { target, .. } => return ty_has_derive_trait(target, symbols, trait_name),
        Ty::Newtype { inner, .. } => return ty_has_derive_trait(inner, symbols, trait_name),
        _ => return false,
    };

    symbols.iter().any(|def| {
        if def.name != *name {
            return false;
        }
        match &def.kind {
            DefKind::Class { info } => {
                if info.derive_traits.iter().any(|t| t == trait_name) {
                    return true;
                }
                // ruby-naming.spec.md §3.6: a `class` never implicitly
                // includes `Copy` (reference semantics). For all other
                // structural mixins, treat them as implicitly included
                // when every field structurally supports them.
                if trait_name == "Copy" {
                    return false;
                }
                struct_or_class_fields_all_support(symbols, &info.fields, trait_name)
            }
            DefKind::Struct { info } => {
                if info.derive_traits.iter().any(|t| t == trait_name) {
                    return true;
                }
                struct_or_class_fields_all_support(symbols, &info.fields, trait_name)
            }
            DefKind::Enum { info } => {
                if info.derive_traits.iter().any(|t| t == trait_name) {
                    return true;
                }
                // ruby-naming.spec.md §3.6: structural mixins apply to
                // enums too — Debug always; Clone/Eq/Hash/Ord when
                // every variant field structurally supports them.
                // `Copy` on enums follows the same field rule but is
                // conservative — enums in v1 stay non-Copy unless all
                // variants are unit-only with primitive payloads, so
                // we still gate on the field check.
                enum_variant_fields_all_support(symbols, &info.variants, trait_name)
            }
            _ => false,
        }
    })
}

/// Helper for `ty_has_derive_trait`: returns true when every field's
/// type structurally supports the requested mixin per the
/// spec §3.6 implicit-include rule.
fn enum_variant_fields_all_support(
    symbols: &SymbolTable,
    variant_def_ids: &[DefId],
    trait_name: &str,
) -> bool {
    variant_def_ids.iter().all(|vid| {
        let def = match symbols.get(*vid) {
            Some(d) => d,
            None => return false,
        };
        match &def.kind {
            DefKind::EnumVariant { kind, .. } => match kind {
                VariantDefKind::Unit => true,
                VariantDefKind::Tuple(types) => types
                    .iter()
                    .all(|ty| ty_supports_structural_mixin(ty, symbols, trait_name)),
                VariantDefKind::Struct(fields) => fields
                    .iter()
                    .all(|(_, ty)| ty_supports_structural_mixin(ty, symbols, trait_name)),
            },
            _ => false,
        }
    })
}

fn struct_or_class_fields_all_support(
    symbols: &SymbolTable,
    field_def_ids: &[DefId],
    trait_name: &str,
) -> bool {
    field_def_ids
        .iter()
        .all(|field_id| match symbols.get(*field_id).map(|d| &d.kind) {
            Some(DefKind::Field { ty, .. }) => {
                ty_supports_structural_mixin(ty, symbols, trait_name)
            }
            Some(DefKind::Variable { ty, .. }) => {
                ty_supports_structural_mixin(ty, symbols, trait_name)
            }
            _ => false,
        })
}

/// Returns true when the given type structurally supports the named
/// structural mixin. Primitives & references satisfy everything; nested
/// nominal types delegate back to `ty_has_derive_trait`.
fn ty_supports_structural_mixin(ty: &Ty, symbols: &SymbolTable, trait_name: &str) -> bool {
    if trait_name == "Copy" {
        return ty.is_copy_with(symbols);
    }
    // For every other structural mixin (Clone, Debug, Eq, Hash, …),
    // primitives and references satisfy unconditionally; user types
    // recurse through `ty_has_derive_trait`.
    match ty {
        Ty::Struct { .. } | Ty::Class { .. } | Ty::Enum { .. } => {
            ty_has_derive_trait(ty, symbols, trait_name)
        }
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => ty_supports_structural_mixin(inner, symbols, trait_name),
        Ty::Newtype { inner, .. } | Ty::Alias { target: inner, .. } => {
            ty_supports_structural_mixin(inner, symbols, trait_name)
        }
        Ty::Tuple(elems) => elems
            .iter()
            .all(|e| ty_supports_structural_mixin(e, symbols, trait_name)),
        Ty::FixedArray(elem, _) => ty_supports_structural_mixin(elem, symbols, trait_name),
        // Array, Map, Set, Option, Result are containers — they
        // forward the structural mixin to their element type.
        Ty::Array(elem) | Ty::Set(elem) | Ty::Option(elem) => {
            ty_supports_structural_mixin(elem, symbols, trait_name)
        }
        Ty::Map(k, v) | Ty::Result(k, v) => {
            ty_supports_structural_mixin(k, symbols, trait_name)
                && ty_supports_structural_mixin(v, symbols, trait_name)
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::token::Span;

    fn dummy_span() -> Span {
        Span::new(0, 0, 1, 1)
    }

    fn var_kind(ty: Ty) -> DefKind {
        DefKind::Variable { mutable: false, ty }
    }

    fn fn_kind(ret: Ty) -> DefKind {
        DefKind::Function {
            signature: FnSignature {
                self_mode: None,
                is_class_method: false,
                is_async: false,
                generic_params: Vec::new(),
                params: Vec::new(),
                return_ty: ret,
                c_symbol: None,
            },
        }
    }

    #[test]
    fn derive_copy_struct_is_effectively_copy() {
        let mut symbols = SymbolTable::new();
        symbols.define(
            "Point".to_string(),
            DefKind::Struct {
                info: StructInfo {
                    generic_params: Vec::new(),
                    fields: Vec::new(),
                    derive_traits: vec!["Copy".to_string(), "Clone".to_string()],
                    layout: Vec::new(),
                    opt_out_send: false,
                    opt_out_sync: false,
                    manual_send: false,
                    manual_sync: false,
                    const_predicates: vec![],
                },
            },
            Visibility::Public,
            dummy_span(),
        );

        let point = Ty::Struct {
            name: "Point".to_string(),
            generic_args: Vec::new(),
        };

        assert!(ty_is_effectively_copy(&point, &symbols));
        assert!(ty_is_effectively_copy(
            &Ty::Tuple(vec![point.clone(), Ty::Int]),
            &symbols
        ));
        assert!(!ty_is_effectively_copy(
            &Ty::Array(Box::new(point)),
            &symbols
        ));
    }

    fn class_kind() -> DefKind {
        DefKind::Class {
            info: ClassInfo {
                generic_params: Vec::new(),
                parent: None,
                fields: Vec::new(),
                methods: Vec::new(),
                derive_traits: Vec::new(),
                opt_out_send: false,
                opt_out_sync: false,
                manual_send: false,
                manual_sync: false,
                const_predicates: vec![],
                flat_heap_struct: false,
            },
        }
    }

    #[test]
    fn new_table_is_empty() {
        let table = SymbolTable::new();
        assert_eq!(table.len(), 0);
        assert!(table.is_empty());
    }

    #[test]
    fn default_matches_new() {
        let table = SymbolTable::default();
        assert!(table.is_empty());
    }

    #[test]
    fn define_returns_sequential_defids_starting_at_zero() {
        let mut table = SymbolTable::new();
        let a = table.define(
            "a".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            dummy_span(),
        );
        let b = table.define(
            "b".to_string(),
            var_kind(Ty::Bool),
            Visibility::Private,
            dummy_span(),
        );
        let c = table.define(
            "c".to_string(),
            var_kind(Ty::Unit),
            Visibility::Private,
            dummy_span(),
        );
        assert_eq!(a, 0);
        assert_eq!(b, 1);
        assert_eq!(c, 2);
        assert_eq!(table.len(), 3);
        assert!(!table.is_empty());
    }

    #[test]
    fn get_returns_definition_by_defid() {
        let mut table = SymbolTable::new();
        let id = table.define(
            "foo".to_string(),
            var_kind(Ty::Int),
            Visibility::Public,
            dummy_span(),
        );
        let def = table.get(id).expect("definition should be present");
        assert_eq!(def.id, id);
        assert_eq!(def.name, "foo");
        assert_eq!(def.visibility, Visibility::Public);
    }

    #[test]
    fn get_returns_none_for_unknown_defid() {
        let table = SymbolTable::new();
        assert!(table.get(0).is_none());
        assert!(table.get(999).is_none());
    }

    #[test]
    fn distinguishes_function_class_and_variable_kinds() {
        let mut table = SymbolTable::new();
        let v = table.define(
            "x".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            dummy_span(),
        );
        let f = table.define(
            "do_stuff".to_string(),
            fn_kind(Ty::Unit),
            Visibility::Public,
            dummy_span(),
        );
        let c = table.define(
            "Widget".to_string(),
            class_kind(),
            Visibility::Public,
            dummy_span(),
        );

        assert!(matches!(
            table.get(v).unwrap().kind,
            DefKind::Variable { .. }
        ));
        assert!(matches!(
            table.get(f).unwrap().kind,
            DefKind::Function { .. }
        ));
        assert!(matches!(table.get(c).unwrap().kind, DefKind::Class { .. }));
    }

    #[test]
    fn span_is_preserved_on_definitions() {
        let mut table = SymbolTable::new();
        let span = Span::new(10, 20, 4, 7);
        let id = table.define(
            "here".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            span.clone(),
        );
        let def = table.get(id).unwrap();
        assert_eq!(def.span, span);
        assert_eq!(def.span.line, 4);
        assert_eq!(def.span.column, 7);
    }

    #[test]
    fn duplicate_names_allocate_distinct_defids() {
        // The symbol table itself does not deduplicate — it's the scope
        // layer that handles shadowing. Two defines with the same name must
        // get distinct DefIds.
        let mut table = SymbolTable::new();
        let a = table.define(
            "same".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            dummy_span(),
        );
        let b = table.define(
            "same".to_string(),
            var_kind(Ty::Bool),
            Visibility::Private,
            dummy_span(),
        );
        assert_ne!(a, b);
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn iter_yields_definitions_in_insertion_order() {
        let mut table = SymbolTable::new();
        table.define(
            "a".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            dummy_span(),
        );
        table.define(
            "b".to_string(),
            var_kind(Ty::Bool),
            Visibility::Private,
            dummy_span(),
        );
        table.define(
            "c".to_string(),
            var_kind(Ty::Char),
            Visibility::Private,
            dummy_span(),
        );
        let names: Vec<_> = table.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn update_ty_changes_variable_type() {
        let mut table = SymbolTable::new();
        let id = table.define(
            "x".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            dummy_span(),
        );
        table.update_ty(id, Ty::Bool);
        let def = table.get(id).unwrap();
        if let DefKind::Variable { ty, .. } = &def.kind {
            assert_eq!(*ty, Ty::Bool);
        } else {
            panic!("expected DefKind::Variable");
        }
    }

    #[test]
    fn update_ty_is_noop_for_class_definitions() {
        let mut table = SymbolTable::new();
        let id = table.define(
            "C".to_string(),
            class_kind(),
            Visibility::Public,
            dummy_span(),
        );
        // Class is not one of the variants update_ty touches; it must be a no-op.
        table.update_ty(id, Ty::Int);
        assert!(matches!(table.get(id).unwrap().kind, DefKind::Class { .. }));
    }

    #[test]
    fn def_ty_returns_type_for_variable() {
        let mut table = SymbolTable::new();
        let id = table.define(
            "x".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            dummy_span(),
        );
        assert_eq!(table.def_ty(id), Some(Ty::Int));
    }

    #[test]
    fn def_ty_returns_fn_type_for_function() {
        let mut table = SymbolTable::new();
        let id = table.define(
            "f".to_string(),
            fn_kind(Ty::Bool),
            Visibility::Public,
            dummy_span(),
        );
        match table.def_ty(id) {
            Some(Ty::Fn { params, ret }) => {
                assert!(params.is_empty());
                assert_eq!(*ret, Ty::Bool);
            }
            other => panic!("expected Ty::Fn, got {:?}", other),
        }
    }

    #[test]
    fn def_ty_returns_none_for_class() {
        let mut table = SymbolTable::new();
        let id = table.define(
            "C".to_string(),
            class_kind(),
            Visibility::Public,
            dummy_span(),
        );
        assert_eq!(table.def_ty(id), None);
    }

    #[test]
    fn name_lookup_is_case_sensitive_via_iter() {
        // The symbol table itself has no name-keyed lookup (scopes own that).
        // Confirm that `iter` sees case-sensitive, distinct names.
        let mut table = SymbolTable::new();
        table.define(
            "Foo".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            dummy_span(),
        );
        table.define(
            "foo".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            dummy_span(),
        );
        let hits: Vec<_> = table.iter().map(|d| d.name.as_str()).collect();
        assert!(hits.contains(&"Foo"));
        assert!(hits.contains(&"foo"));
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn get_mut_allows_in_place_mutation() {
        let mut table = SymbolTable::new();
        let id = table.define(
            "x".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            dummy_span(),
        );
        {
            let def = table.get_mut(id).unwrap();
            def.name = "renamed".to_string();
        }
        assert_eq!(table.get(id).unwrap().name, "renamed");
    }

    #[test]
    fn visibility_is_preserved_per_definition() {
        let mut table = SymbolTable::new();
        let a = table.define(
            "a".to_string(),
            var_kind(Ty::Int),
            Visibility::Private,
            dummy_span(),
        );
        let b = table.define(
            "b".to_string(),
            var_kind(Ty::Int),
            Visibility::Public,
            dummy_span(),
        );
        let c = table.define(
            "c".to_string(),
            var_kind(Ty::Int),
            Visibility::Protected,
            dummy_span(),
        );
        assert_eq!(table.get(a).unwrap().visibility, Visibility::Private);
        assert_eq!(table.get(b).unwrap().visibility, Visibility::Public);
        assert_eq!(table.get(c).unwrap().visibility, Visibility::Protected);
    }

    // NOTE: The task spec mentions "Clear/reset behavior if exposed" — the
    // current `SymbolTable` does not expose `clear` or `reset`; definitions
    // are append-only for the lifetime of the table. Documented as ignored.
    #[test]
    #[ignore = "SymbolTable exposes no clear/reset API; table is append-only"]
    fn clear_reset_not_exposed() {}
}
