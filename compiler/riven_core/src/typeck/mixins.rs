//! Mixin resolution for the Riven type system.
//!
//! (Was "trait" resolution pre-Ruby-naming migration — see
//! docs/specs/syntax/ruby-naming.spec.md.)
//!
//! Two modes of mixin satisfaction:
//! 1. Structural: type has all required methods with matching signatures
//! 2. Nominal: explicit `include M` (or pre-Ruby-naming `impl Trait for Type`)
//!    block exists
//!
//! Static dispatch (`some M`) accepts structural satisfaction.
//! Dynamic dispatch (`any M`) requires nominal satisfaction.

use std::collections::HashMap;

use crate::hir::nodes::DefId;
use crate::hir::nodes::*;
use crate::hir::types::{MixinRef, Ty};
use crate::resolve::symbols::{DefKind, FnSignature, MixinInfo, SymbolTable};

/// Result of checking whether a type satisfies a trait.
#[derive(Debug, Clone)]
pub enum MixinSatisfaction {
    /// Type satisfies the trait via an explicit impl block.
    Nominal,
    /// Type satisfies the trait structurally (has all required methods).
    Structural,
    /// Type does not satisfy the trait.
    Unsatisfied { missing_methods: Vec<String> },
}

/// The trait resolver manages all known impl blocks and performs
/// structural and nominal trait satisfaction checks.
pub struct MixinResolver {
    /// All known impl blocks: (target_type_name, trait_name) → methods
    nominal_impls: HashMap<(String, String), Vec<ImplMethod>>,
    /// Methods defined on types (from class bodies and standalone impls)
    type_methods: HashMap<String, Vec<TypeMethod>>,
    /// trait_name → (method_name → signature) from the trait *declaration*
    /// (both required method signatures and default methods). Used to
    /// dispatch method calls on a generic `T: Trait` receiver.
    trait_method_sigs: HashMap<String, HashMap<String, FnSignature>>,
}

#[derive(Debug, Clone)]
struct ImplMethod {
    name: String,
    signature: FnSignature,
}

#[derive(Debug, Clone)]
struct TypeMethod {
    name: String,
    signature: FnSignature,
}

impl MixinResolver {
    pub fn new() -> Self {
        Self {
            nominal_impls: HashMap::new(),
            type_methods: HashMap::new(),
            trait_method_sigs: HashMap::new(),
        }
    }

    /// Register an impl block discovered during name resolution.
    pub fn register_impl(
        &mut self,
        target_type: &str,
        trait_name: Option<&str>,
        methods: Vec<(String, FnSignature)>,
    ) {
        let type_name = target_type.to_string();

        if let Some(tname) = trait_name {
            let key = (type_name.clone(), tname.to_string());
            let impl_methods: Vec<ImplMethod> = methods
                .iter()
                .map(|(name, sig)| ImplMethod {
                    name: name.clone(),
                    signature: sig.clone(),
                })
                .collect();
            self.nominal_impls.insert(key, impl_methods);
        }

        // Also record methods on the type itself
        let type_meths = self.type_methods.entry(type_name).or_default();
        for (name, sig) in methods {
            type_meths.push(TypeMethod {
                name,
                signature: sig,
            });
        }
    }

    /// Check if a type satisfies a trait.
    ///
    /// For `impl Trait` (static dispatch): structural satisfaction is accepted.
    /// For `dyn Trait` (dynamic dispatch): only nominal satisfaction is accepted.
    pub fn check_satisfaction(
        &self,
        ty: &Ty,
        trait_ref: &MixinRef,
        symbols: &SymbolTable,
        require_nominal: bool,
    ) -> MixinSatisfaction {
        if matches!(trait_ref.name.as_str(), "Send" | "Sync") {
            let satisfied = match trait_ref.name.as_str() {
                "Send" => ty.is_send_with(symbols),
                "Sync" => ty.is_sync_with(symbols),
                _ => unreachable!(),
            };
            return if satisfied {
                MixinSatisfaction::Structural
            } else {
                MixinSatisfaction::Unsatisfied {
                    missing_methods: vec![format!(
                        "type `{}` does not satisfy `{}`",
                        ty, trait_ref.name
                    )],
                }
            };
        }

        let type_name = Self::type_name(ty);

        // Check nominal satisfaction first
        let key = (type_name.clone(), trait_ref.name.clone());
        if self.nominal_impls.contains_key(&key) {
            return MixinSatisfaction::Nominal;
        }

        if require_nominal {
            return MixinSatisfaction::Unsatisfied {
                missing_methods: vec![format!(
                    "no explicit `include {}` in `{}`",
                    trait_ref.name, type_name
                )],
            };
        }

        // Check structural satisfaction: does the type have all required methods?
        let trait_info = self.find_trait_info(&trait_ref.name, symbols);
        if let Some(info) = trait_info {
            let type_meths = self.type_methods.get(&type_name);
            let mut missing = Vec::new();

            for required in &info.required_methods {
                let found = type_meths
                    .map(|meths| meths.iter().any(|m| m.name == *required))
                    .unwrap_or(false);

                if !found {
                    missing.push(required.clone());
                }
            }

            if missing.is_empty() {
                MixinSatisfaction::Structural
            } else {
                MixinSatisfaction::Unsatisfied {
                    missing_methods: missing,
                }
            }
        } else {
            // Unknown trait — assume unsatisfied
            MixinSatisfaction::Unsatisfied {
                missing_methods: vec![format!("unknown mixin `{}`", trait_ref.name)],
            }
        }
    }

    /// Look up a method across a slice of trait bounds.
    ///
    /// Returned outcomes:
    ///   * `Ok(Some(sig))`   — exactly one bound declares `method_name`;
    ///   * `Ok(None)`        — no bound declares it;
    ///   * `Err(Vec<String>)` — the method name is provided by more than one
    ///     bound (caller should report an ambiguity diagnostic listing the
    ///     traits).
    pub fn lookup_method_on_bounds(
        &self,
        bounds: &[MixinRef],
        method_name: &str,
    ) -> Result<Option<FnSignature>, Vec<String>> {
        let mut found: Option<FnSignature> = None;
        let mut providers: Vec<String> = Vec::new();
        for b in bounds {
            if let Some(methods) = self.trait_method_sigs.get(&b.name) {
                if let Some(sig) = methods.get(method_name) {
                    providers.push(b.name.clone());
                    if found.is_none() {
                        found = Some(sig.clone());
                    }
                }
            } else if matches!(b.name.as_str(), "Hashable" | "Hash") && method_name == "hash_code" {
                providers.push(b.name.clone());
                if found.is_none() {
                    found = Some(FnSignature {
                        self_mode: Some(HirSelfMode::Ref),
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params: vec![],
                        return_ty: Ty::Int,
                        c_symbol: None,
                    });
                }
            }
        }
        if providers.len() > 1 {
            Err(providers)
        } else {
            Ok(found)
        }
    }

    /// Look up a method on a type (including inherited methods and trait impls).
    pub fn lookup_method(
        &self,
        ty: &Ty,
        method_name: &str,
        symbols: &SymbolTable,
    ) -> Option<FnSignature> {
        let type_name = Self::type_name(ty);

        // Check direct type methods first
        if let Some(meths) = self.type_methods.get(&type_name) {
            if let Some(m) = meths.iter().find(|m| m.name == method_name) {
                return Some(m.signature.clone());
            }
        }

        // Check trait impls
        for ((tname, _), methods) in &self.nominal_impls {
            if *tname == type_name {
                if let Some(m) = methods.iter().find(|m| m.name == method_name) {
                    return Some(m.signature.clone());
                }
            }
        }

        // Check trait default methods for each trait the type implements.
        // If the impl block itself didn't provide `method_name` (handled
        // above), the trait's own default body supplies the signature.
        for (impl_target, trait_name) in self.nominal_impls.keys() {
            if *impl_target == type_name {
                if let Some(methods) = self.trait_method_sigs.get(trait_name) {
                    if let Some(sig) = methods.get(method_name) {
                        return Some(sig.clone());
                    }
                }
            }
        }

        // Check parent class (inheritance)
        if let Ty::Class { name, .. } = ty {
            for def in symbols.iter() {
                if def.name == *name {
                    if let DefKind::Class { info } = &def.kind {
                        if let Some(parent_id) = info.parent {
                            if let Some(parent_def) = symbols.get(parent_id) {
                                let parent_ty = Ty::Class {
                                    name: parent_def.name.clone(),
                                    generic_args: vec![],
                                };
                                return self.lookup_method(&parent_ty, method_name, symbols);
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Collect all impl blocks for the HIR program.
    pub fn collect_impls(&mut self, program: &HirProgram, symbols: &SymbolTable) {
        for item in &program.items {
            self.collect_item_impls(item, symbols, &[]);
        }
    }

    /// Phase E.E of #06.95: register every class found in the
    /// `type_registry` under its qualified key. Bootstrap-loaded
    /// classes (especially module-nested ones like `BufReader.File`)
    /// don't appear in the user program's HirProgram items, so the
    /// `collect_impls` walk above misses them entirely. Walking the
    /// registry directly catches every Class definition the resolver
    /// produced — including the qualified-name entries
    /// `insert_type_qualified` populates for module-nested classes.
    ///
    /// For each class, registers ALL methods (user-body via
    /// `HirClassDef.methods`-equivalent walk, plus lib-decl methods
    /// appended onto `ClassInfo.methods` by `pass1_class_lib_methods`)
    /// under the registry key. The lookup at method-call time then
    /// hits the qualified name when the receiver carries one
    /// (`Ty::Class { name: "BufReader.File" }`).
    pub fn register_classes_from_registry(
        &mut self,
        type_registry: &HashMap<String, DefId>,
        symbols: &SymbolTable,
    ) {
        for (qualified, &def_id) in type_registry {
            let Some(def) = symbols.get(def_id) else {
                continue;
            };
            let DefKind::Class { info } = &def.kind else {
                continue;
            };
            let mut methods: Vec<(String, FnSignature)> = Vec::new();
            // Read methods from `ClassInfo.methods` first (user-side
            // classes that went through Pass 2 have their lib-decl
            // entries already merged in here).
            for method_id in &info.methods {
                if let Some(m_def) = symbols.get(*method_id) {
                    if let DefKind::Method { signature, .. } = &m_def.kind {
                        if !methods.iter().any(|(n, _)| n == &m_def.name) {
                            methods.push((m_def.name.clone(), signature.clone()));
                        }
                    }
                }
            }
            // Bootstrap-loaded classes never go through Pass 2, so
            // `ClassInfo.methods` stays empty even when lib decls were
            // registered in Pass 1 via `pass1_class_lib_methods`. Scan
            // the symbol table for `DefKind::Method { parent: def_id }`
            // to find them.
            if methods.is_empty() {
                for m_def in symbols.iter() {
                    if let DefKind::Method { parent, signature } = &m_def.kind {
                        if *parent == def_id && !methods.iter().any(|(n, _)| n == &m_def.name) {
                            methods.push((m_def.name.clone(), signature.clone()));
                        }
                    }
                }
            }
            if !methods.is_empty() {
                self.register_impl(qualified, None, methods);
            }
        }
    }

    fn collect_item_impls(
        &mut self,
        item: &HirItem,
        symbols: &SymbolTable,
        module_path: &[String],
    ) {
        match item {
            HirItem::Mixin(tdef) => {
                use crate::resolve::symbols::ParamInfo;
                let mut new_entries: Vec<(String, FnSignature)> = Vec::new();
                for ti in &tdef.items {
                    match ti {
                        HirMixinItem::MethodSig {
                            name,
                            self_mode,
                            is_class_method,
                            params,
                            return_ty,
                            ..
                        } => {
                            let sig = FnSignature {
                                self_mode: *self_mode,
                                is_class_method: *is_class_method,
                                is_async: false,
                                generic_params: vec![],
                                params: params
                                    .iter()
                                    .map(|p| ParamInfo {
                                        name: p.name.clone(),
                                        ty: p.ty.clone(),
                                        auto_assign: p.auto_assign,
                                        default: p.default.clone(),
                                    })
                                    .collect(),
                                return_ty: return_ty.clone(),
                                c_symbol: None,
                            };
                            new_entries.push((name.clone(), sig));
                        }
                        HirMixinItem::DefaultMethod(f) => {
                            new_entries.push((f.name.clone(), self.func_to_sig(f)));
                        }
                        HirMixinItem::AssocType { .. } => {}
                    }
                }
                let entry = self.trait_method_sigs.entry(tdef.name.clone()).or_default();
                for (k, v) in new_entries {
                    entry.insert(k, v);
                }
            }
            HirItem::Class(class) => {
                // Phase E.E of #06.95: module-nested classes need the
                // QUALIFIED name (e.g. `BufReader.File`) so the typeck
                // lookup keys match the receiver's `Ty::Class.name`.
                let type_name = if module_path.is_empty() {
                    class.name.clone()
                } else {
                    format!("{}.{}", module_path.join("."), class.name)
                };
                // Register user-body class methods
                let mut methods: Vec<(String, FnSignature)> = class
                    .methods
                    .iter()
                    .map(|m| (m.name.clone(), self.func_to_sig(m)))
                    .collect();
                // Phase E.E: also include lib-decl methods that the
                // resolver appended onto `ClassInfo.methods` via
                // `pass1_class_lib_methods`. These live in the symbol
                // table as `DefKind::Method` entries — not in
                // `HirClassDef.methods` — so the historical walk
                // missed them entirely.
                if let Some(def) = symbols.get(class.def_id) {
                    if let DefKind::Class { info } = &def.kind {
                        for method_id in &info.methods {
                            if let Some(m_def) = symbols.get(*method_id) {
                                if let DefKind::Method { signature, .. } = &m_def.kind {
                                    if !methods.iter().any(|(n, _)| n == &m_def.name) {
                                        methods.push((m_def.name.clone(), signature.clone()));
                                    }
                                }
                            }
                        }
                    }
                }
                self.register_impl(&type_name, None, methods);
                self.register_derived_impls(
                    &type_name,
                    &Ty::Class {
                        name: type_name.clone(),
                        generic_args: vec![],
                    },
                    &class.derive_traits,
                );

                // Register inner impl blocks
                for imp in &class.impl_blocks {
                    if let Some(ref trait_ref) = imp.trait_ref {
                        let methods: Vec<(String, FnSignature)> = imp
                            .items
                            .iter()
                            .filter_map(|item| match item {
                                HirImplItem::Method(m) => {
                                    Some((m.name.clone(), self.func_to_sig(m)))
                                }
                                _ => None,
                            })
                            .collect();
                        self.register_impl(&type_name, Some(&trait_ref.name), methods);
                    }
                }
            }
            HirItem::Struct(strukt) => {
                self.register_derived_impls(
                    &strukt.name,
                    &Ty::Struct {
                        name: strukt.name.clone(),
                        generic_args: vec![],
                    },
                    &strukt.derive_traits,
                );
            }
            HirItem::Enum(enm) => {
                self.register_derived_impls(
                    &enm.name,
                    &Ty::Enum {
                        name: enm.name.clone(),
                        generic_args: vec![],
                    },
                    &enm.derive_traits,
                );
            }
            HirItem::Impl(imp) => {
                let type_name = Self::type_name(&imp.target_ty);
                let trait_name = imp.trait_ref.as_ref().map(|tr| tr.name.as_str());
                let methods: Vec<(String, FnSignature)> = imp
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        HirImplItem::Method(m) => Some((m.name.clone(), self.func_to_sig(m))),
                        _ => None,
                    })
                    .collect();
                self.register_impl(&type_name, trait_name, methods);
            }
            HirItem::Module(m) => {
                let mut child_path: Vec<String> = module_path.to_vec();
                child_path.push(m.name.clone());
                for sub_item in &m.items {
                    self.collect_item_impls(sub_item, symbols, &child_path);
                }
            }
            _ => {}
        }
    }

    fn register_derived_impls(
        &mut self,
        type_name: &str,
        target_ty: &Ty,
        derive_traits: &[String],
    ) {
        for trait_name in derive_traits {
            let methods = match trait_name.as_str() {
                "Clone" => vec![(
                    "clone".to_string(),
                    FnSignature {
                        self_mode: Some(HirSelfMode::Ref),
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params: vec![],
                        return_ty: target_ty.clone(),
                        c_symbol: None,
                    },
                )],
                "PartialEq" => vec![(
                    "eq".to_string(),
                    FnSignature {
                        self_mode: Some(HirSelfMode::Ref),
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params: vec![crate::resolve::symbols::ParamInfo {
                            name: "other".to_string(),
                            ty: Ty::Ref(Box::new(target_ty.clone())),
                            auto_assign: false,
                            default: None,
                        }],
                        return_ty: Ty::Bool,
                        c_symbol: None,
                    },
                )],
                "Hashable" | "Hash" => vec![(
                    "hash_code".to_string(),
                    FnSignature {
                        self_mode: Some(HirSelfMode::Ref),
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params: vec![],
                        return_ty: Ty::Int,
                        c_symbol: None,
                    },
                )],
                "Default" => vec![(
                    "default".to_string(),
                    FnSignature {
                        self_mode: None,
                        is_class_method: true,
                        is_async: false,
                        generic_params: vec![],
                        params: vec![],
                        return_ty: target_ty.clone(),
                        c_symbol: None,
                    },
                )],
                "Ord" => vec![(
                    "cmp".to_string(),
                    FnSignature {
                        self_mode: Some(HirSelfMode::Ref),
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params: vec![crate::resolve::symbols::ParamInfo {
                            name: "other".to_string(),
                            ty: Ty::Ref(Box::new(target_ty.clone())),
                            auto_assign: false,
                            default: None,
                        }],
                        return_ty: Ty::Int,
                        c_symbol: None,
                    },
                )],
                "PartialOrd" => vec![(
                    "partial_cmp".to_string(),
                    FnSignature {
                        self_mode: Some(HirSelfMode::Ref),
                        is_class_method: false,
                        is_async: false,
                        generic_params: vec![],
                        params: vec![crate::resolve::symbols::ParamInfo {
                            name: "other".to_string(),
                            ty: Ty::Ref(Box::new(target_ty.clone())),
                            auto_assign: false,
                            default: None,
                        }],
                        return_ty: Ty::Int,
                        c_symbol: None,
                    },
                )],
                _ => vec![],
            };
            self.register_impl(type_name, Some(trait_name), methods);
        }
    }

    fn func_to_sig(&self, func: &HirFuncDef) -> FnSignature {
        use crate::resolve::symbols::ParamInfo;
        FnSignature {
            self_mode: func.self_mode,
            is_class_method: func.is_class_method,
            is_async: func.is_async,
            generic_params: func
                .generic_params
                .iter()
                .map(|gp| {
                    crate::resolve::symbols::GenericParamInfo::type_param(
                        gp.name.clone(),
                        gp.bounds.clone(),
                    )
                })
                .collect(),
            params: func
                .params
                .iter()
                .map(|p| ParamInfo {
                    name: p.name.clone(),
                    ty: p.ty.clone(),
                    auto_assign: p.auto_assign,
                    default: p.default.clone(),
                })
                .collect(),
            return_ty: func.return_ty.clone(),
            c_symbol: None,
        }
    }

    fn find_trait_info<'a>(&self, name: &str, symbols: &'a SymbolTable) -> Option<&'a MixinInfo> {
        for def in symbols.iter() {
            if def.name == name {
                if let DefKind::Trait { ref info } = def.kind {
                    return Some(info);
                }
            }
        }
        None
    }

    fn type_name(ty: &Ty) -> String {
        match ty {
            // Phase E.E of #06.95: peel reference layers so a
            // method call on `&var BufReader.File` looks up under
            // `"BufReader.File"`, not `"&mut BufReader.File"` — the
            // type_methods map is keyed by the underlying class name.
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => Self::type_name(inner),
            Ty::Class { name, .. } => name.clone(),
            Ty::Struct { name, .. } => name.clone(),
            Ty::Enum { name, .. } => name.clone(),
            Ty::Int => "Int".to_string(),
            Ty::Float => "Float".to_string(),
            Ty::Bool => "Bool".to_string(),
            Ty::String => "String".to_string(),
            Ty::Str => "&str".to_string(),
            Ty::USize => "USize".to_string(),
            Ty::Char => "Char".to_string(),
            Ty::Unit => "()".to_string(),
            other => format!("{}", other),
        }
    }
}

impl Default for MixinResolver {
    fn default() -> Self {
        Self::new()
    }
}
