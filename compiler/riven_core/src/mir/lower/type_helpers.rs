use super::*;

impl<'a> Lowerer<'a> {
    /// Returns Some(struct_name) if `ty` (peeling refs/aliases) is a struct
    /// whose declaration includes `derive Debug`; otherwise None.
    pub(super) fn struct_with_derive_debug(&self, ty: &Ty) -> Option<String> {
        self.struct_with_derive_trait(ty, "Debug")
    }

    /// Phase 2 #06.C2: like `struct_with_derive_debug` but for enums.
    /// Returns the enum's name when `ty` resolves (through refs/
    /// aliases/newtypes) to an enum whose `derive_traits` contains
    /// `Debug`.
    pub(super) fn enum_with_derive_debug(&self, ty: &Ty) -> Option<String> {
        self.enum_with_derive_trait(ty, "Debug")
    }

    pub(super) fn enum_with_derive_trait(&self, ty: &Ty, trait_name: &str) -> Option<String> {
        // ruby-naming.spec.md §3.6: structural mixins are implicitly
        // included when the type's fields qualify. Defer to
        // `ty_has_derive_trait` so the dispatcher honours implicit
        // Debug / Clone / etc. — without this an enum that hasn't
        // *explicitly* loud-included Debug falls through the dispatch
        // table and gets formatted as a raw pointer via `Int_fmt`.
        let name = match ty {
            Ty::Enum { name, .. } => name.clone(),
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => {
                return self.enum_with_derive_trait(inner, trait_name)
            }
            Ty::Alias { target, .. } => return self.enum_with_derive_trait(target, trait_name),
            Ty::Newtype { inner, .. } => return self.enum_with_derive_trait(inner, trait_name),
            _ => return None,
        };
        if crate::resolve::symbols::ty_has_derive_trait(ty, self.symbols, trait_name) {
            Some(name)
        } else {
            None
        }
    }

    pub(super) fn receiver_type_name(&self, expr: &HirExpr) -> Option<String> {
        use crate::resolve::symbols::DefKind;

        let HirExprKind::VarRef(def_id) = expr.kind else {
            return None;
        };
        let def = self.symbols.get(def_id)?;
        match &def.kind {
            // Phase E.E of #06.95: for Class/Struct/Enum DefIds,
            // return None so the caller falls back to
            // `type_name_from_ty(&object.ty)`. The Ty carries the
            // QUALIFIED name (`BufReader.File`) for module-nested
            // classes, while the symbol-table `def.name` is just the
            // unqualified leaf (`File`) — using `def.name` for the
            // mangle key would produce `File_new` and miss the FFI
            // alias `BufReader_File_new → riven_bufreader_new_file`.
            DefKind::Class { .. } | DefKind::Struct { .. } | DefKind::Enum { .. } => None,
            DefKind::TypeAlias { target } => Some(type_name_from_ty(target)),
            _ => None,
        }
    }

    /// Returns true when `expr` is a bare type identifier used as the
    /// "receiver" of a `Type.method(arg)` call — i.e. a `VarRef` whose
    /// DefId is a class, struct, or enum. In that case the user wrote
    /// the call in *static-style* even if `method` is declared as an
    /// instance method (`def method(self, ...) as "c_sym"` inside a
    /// class lib block). The call-site MUST NOT prepend a phantom
    /// `Unit` (zero) as the self argument — the user's first explicit
    /// arg IS the self handle, and the FFI signature already has the
    /// receiver type prepended at registration time (see
    /// `register_class_lib_method_in` in `resolve/ffi_registration.rs`).
    pub(super) fn is_class_identifier(&self, expr: &HirExpr) -> bool {
        use crate::resolve::symbols::DefKind;
        let HirExprKind::VarRef(def_id) = expr.kind else {
            return false;
        };
        let Some(def) = self.symbols.get(def_id) else {
            return false;
        };
        matches!(
            def.kind,
            DefKind::Class { .. } | DefKind::Struct { .. } | DefKind::Enum { .. }
        )
    }

    pub(super) fn type_supports_trait(&self, ty: &Ty, trait_name: &str) -> bool {
        if self.struct_with_derive_trait(ty, trait_name).is_some() {
            return true;
        }
        match ty {
            Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                bounds.iter().any(|bound| bound.name == trait_name)
            }
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => self.type_supports_trait(inner, trait_name),
            Ty::Alias { target, .. } => self.type_supports_trait(target, trait_name),
            Ty::Newtype { inner, .. } => self.type_supports_trait(inner, trait_name),
            _ => false,
        }
    }

    pub(super) fn struct_with_derive_trait(&self, ty: &Ty, trait_name: &str) -> Option<String> {
        // Peel reference/alias/newtype layers, then consult
        // `ty_has_derive_trait` so implicit-include structural mixins
        // (spec §3.6) are honoured alongside explicit `derive_traits`.
        let mut cur = ty;
        loop {
            match cur {
                Ty::Ref(inner)
                | Ty::RefMut(inner)
                | Ty::RefLifetime(_, inner)
                | Ty::RefMutLifetime(_, inner) => cur = inner,
                Ty::Alias { target, .. } => cur = target,
                Ty::Newtype { inner, .. } => cur = inner,
                Ty::Struct { name, .. } => {
                    if crate::resolve::symbols::ty_has_derive_trait(cur, self.symbols, trait_name) {
                        return Some(name.clone());
                    }
                    return None;
                }
                _ => return None,
            }
        }
    }
}

/// Check if a type is an Option type (including via references and inferred types).
/// Returns true if the type is a string-like type whose runtime representation
/// is already a `char*` and needs no conversion for string interpolation.
pub(super) fn is_string_like(ty: &Ty) -> bool {
    match ty {
        Ty::String | Ty::Str => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_string_like(inner),
        _ => false,
    }
}

/// Returns true if the expression's type is unresolved but the expression
/// is a method call that likely returns a string at runtime. This handles
/// cases where type inference left Infer(...) types unresolved for methods
/// like `to_display`, `message`, `summary`, `clone` on string types, etc.
pub(super) fn is_inferred_string_expr(expr: &HirExpr) -> bool {
    if !matches!(expr.ty, Ty::Infer(_)) {
        return false;
    }
    // Known string-returning method names.
    let string_methods = [
        "to_display",
        "to_string",
        "message",
        "summary",
        "serialize",
        "clone",
        "title_ref",
        "deadline_ref",
        "to_lower",
        "trim",
        "push_str",
        "unwrap_or",
        "unwrap_or_else",
    ];

    match &expr.kind {
        HirExprKind::MethodCall { method_name, .. } => {
            string_methods.contains(&method_name.as_str())
        }
        // FieldAccess can also be a no-arg method call.
        HirExprKind::FieldAccess { field_name, .. } => {
            string_methods.contains(&field_name.as_str())
        }
        _ => false,
    }
}

/// Extract a user-visible type name from a `Ty` for method mangling.
pub fn type_name_from_ty(ty: &Ty) -> String {
    match ty {
        Ty::Class { name, .. } => name.clone(),
        Ty::Struct { name, .. } => name.clone(),
        Ty::Enum { name, .. } => name.clone(),
        Ty::Ref(inner) | Ty::RefMut(inner) => type_name_from_ty(inner),
        Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => type_name_from_ty(inner),
        other => other.type_name(),
    }
}

/// Get the name of a definition from the symbol table.
pub fn def_id_name(def_id: DefId, symbols: &SymbolTable) -> String {
    symbols
        .get(def_id)
        .map(|d| d.name.clone())
        .unwrap_or_else(|| format!("_unknown_{}", def_id))
}

/// If `ty` (after peeling reference layers) is a struct that derives —
/// explicitly or implicitly per ruby-naming.spec.md §3.6 — `PartialEq`,
/// return the struct's name. Otherwise return `None`.
pub(super) fn struct_name_with_partial_eq(ty: &Ty, symbols: &SymbolTable) -> Option<String> {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(inner) | Ty::RefMut(inner) => cur = inner,
            Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => cur = inner,
            Ty::Struct { name, .. } => {
                if crate::resolve::symbols::ty_has_derive_trait(cur, symbols, "PartialEq") {
                    return Some(name.clone());
                }
                return None;
            }
            _ => return None,
        }
    }
}

/// Return `Some((struct_name, partial))` when `ty` (peeling refs and
/// aliases) is a struct that declares `derive Ord` or `derive
/// PartialOrd`. The boolean is `true` only when the struct derives
/// `PartialOrd` *without* `Ord`, in which case the BinaryOp lowering
/// must dispatch to `<Type>_partial_cmp` rather than `<Type>_cmp`.
pub(super) fn struct_name_with_ord(ty: &Ty, symbols: &SymbolTable) -> Option<(String, bool)> {
    // ruby-naming.spec.md §3.6: Ord / PartialOrd are implicitly
    // included when every field structurally supports them. Route
    // through `ty_has_derive_trait` so both explicit and implicit
    // forms surface here. Prefer `Ord` over `PartialOrd` when both
    // apply (Ord is total — `<Type>_cmp`; PartialOrd is
    // `<Type>_partial_cmp` which returns `Option[Ordering]`).
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(inner) | Ty::RefMut(inner) => cur = inner,
            Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => cur = inner,
            Ty::Struct { name, .. } => {
                if crate::resolve::symbols::ty_has_derive_trait(cur, symbols, "Ord") {
                    return Some((name.clone(), false));
                }
                if crate::resolve::symbols::ty_has_derive_trait(cur, symbols, "PartialOrd") {
                    return Some((name.clone(), true));
                }
                return None;
            }
            _ => return None,
        }
    }
}

/// Return the ordered `(field_index, field_ty)` list for a struct, or
/// `None` if the name doesn't refer to a known struct.
pub(super) fn struct_field_layout(name: &str, symbols: &SymbolTable) -> Option<Vec<(usize, Ty)>> {
    use crate::resolve::symbols::DefKind;
    for def in symbols.iter() {
        if def.name == name {
            if let DefKind::Struct { ref info } = def.kind {
                let mut out = Vec::with_capacity(info.fields.len());
                for &fid in &info.fields {
                    let field_def = symbols.get(fid)?;
                    if let DefKind::Field { index, ref ty, .. } = field_def.kind {
                        out.push((index, ty.clone()));
                    }
                }
                return Some(out);
            }
        }
    }
    None
}

impl<'a> Lowerer<'a> {
    /// Phase B-4: number of header slots prepended to a class's
    /// allocation. One slot per runtime-dispatch mixin include — for
    /// v1, every runtime-dispatch class has exactly one class_info_ptr
    /// header at slot 0 (the class_info struct itself carries one
    /// pointer per mixin), so this function returns 0 or 1. Spec
    /// §B2/§B8: single class_info_ptr at offset 0.
    ///
    /// `class_name` is the resolved class identifier (NOT
    /// `Class[GenericArgs]` mangling — caller strips generics).
    /// Returns 0 for any name that isn't a runtime-dispatch class.
    ///
    /// Callers: every MIR lowering site that computes a `field_index`
    /// for `GetField` / `SetField` on a class receiver. The shift is
    /// **not** applied to struct fields, tuple slots, Option/Result
    /// payloads, range fields, or any built-in 2-slot shape — those
    /// do NOT carry a class_info header.
    /// Recover the owning class name from a mangled MIR fn name like
    /// `ClassName_methodName`. Mangling is one-way (`format!("{C}_{m}")`),
    /// and both halves can carry underscores — class names like
    /// `__HandlerFuture` (synth async state-machine), method names like
    /// `do_something` (snake_case). So neither `split('_').next()` nor
    /// `rsplit_once('_')` is correct in general.
    ///
    /// Strategy: walk underscore positions right-to-left and return the
    /// longest prefix that matches a class/struct/enum in the symbol
    /// table. For `__HandlerFuture_init` this yields `__HandlerFuture`;
    /// for `Foo_do_something` it yields `Foo`.
    ///
    /// Returns `None` for top-level functions (no underscore, or no
    /// matching type), letting callers fall back to a non-class-method
    /// shape (e.g. `Ty::Unit` self placeholder, empty shift).
    pub(super) fn class_name_from_mangled(&self, mangled: &str) -> Option<String> {
        use crate::resolve::symbols::DefKind;
        let mut end = mangled.len();
        while let Some(pos) = mangled[..end].rfind('_') {
            let candidate = &mangled[..pos];
            if !candidate.is_empty() {
                for def in self.symbols.iter() {
                    if matches!(
                        &def.kind,
                        DefKind::Class { .. } | DefKind::Struct { .. } | DefKind::Enum { .. }
                    ) && (def.name == candidate || def.name.replace('.', "_") == candidate)
                    {
                        return Some(def.name.clone());
                    }
                }
            }
            end = pos;
        }
        None
    }

    /// Recover the receiver type for `<Primitive>_method` mangled names —
    /// the fallback for `extension Int { def to_display }` and friends.
    /// `class_name_from_mangled` only matches user-defined Class/Struct/Enum
    /// entries in the symbol table; primitive types don't live there, so an
    /// extension on a primitive yields a `None` lookup and the caller
    /// previously fell back to `Ty::Unit`, silently dropping the self
    /// parameter from the Cranelift signature.
    ///
    /// Walks prefixes right-to-left (same shape as `class_name_from_mangled`)
    /// and returns the primitive `Ty` for the first match.
    pub(super) fn primitive_self_ty_from_mangled(&self, mangled: &str) -> Option<Ty> {
        let mut end = mangled.len();
        while let Some(pos) = mangled[..end].rfind('_') {
            let candidate = &mangled[..pos];
            if let Some(ty) = primitive_ty_by_name(candidate) {
                return Some(ty);
            }
            end = pos;
        }
        None
    }

    pub(super) fn class_field_index_shift(&self, class_name: &str) -> usize {
        use crate::resolve::symbols::DefKind;
        for def in self.symbols.iter() {
            if def.name == class_name {
                if let DefKind::Class { info } = &def.kind {
                    if !info.runtime_dispatch_includes.is_empty() {
                        // v1 ships a single class_info_ptr header at
                        // slot 0; class_info itself widens for >1
                        // mixin includes.
                        return 1;
                    }
                    return 0;
                }
            }
        }
        0
    }

    /// Phase B-4 convenience: extract the class name from a Ty and
    /// look up its header shift. Peels Ref/RefMut/Alias/Newtype.
    /// Returns 0 for non-class types (structs, tuples, primitives).
    pub(super) fn class_field_shift_for_ty(&self, ty: &Ty) -> usize {
        let mut cur = ty;
        loop {
            match cur {
                Ty::Ref(inner) | Ty::RefMut(inner) => cur = inner,
                Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => cur = inner,
                Ty::Alias { target, .. } => cur = target,
                Ty::Newtype { inner, .. } => cur = inner,
                Ty::Class { name, .. } => return self.class_field_index_shift(name),
                _ => return 0,
            }
        }
    }

    /// Phase C: if `ty` is a reference to a single-bound
    /// runtime-dispatch mixin (e.g. `&Future`, `&var Future`),
    /// return the mixin's name. Otherwise return None — the receiver
    /// either isn't a `&Mixin` shape or the mixin isn't
    /// `dispatch runtime` (E1118 would have flagged that at typeck).
    ///
    /// Carries Ref / RefMut / RefLifetime / RefMutLifetime peels.
    /// `Ty::AnyMixin(bounds)` with multiple bounds (`&Send + Sync`)
    /// can't be dispatched dynamically because no single vtable
    /// exists — return None. v1 only supports single-mixin dyn
    /// references; multi-bound trait objects are out of scope per
    /// spec "Out of scope (v2)".
    pub(super) fn dyn_mixin_receiver_name(&self, ty: &Ty) -> Option<String> {
        use crate::resolve::symbols::DefKind;
        let mut cur = ty;
        loop {
            match cur {
                Ty::Ref(inner) | Ty::RefMut(inner) => cur = inner,
                Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => cur = inner,
                Ty::Alias { target, .. } => cur = target,
                Ty::Newtype { inner, .. } => cur = inner,
                Ty::AnyMixin(bounds) => {
                    if bounds.len() != 1 {
                        return None;
                    }
                    let name = &bounds[0].name;
                    // Verify the mixin is registered as
                    // dispatch_mode = Runtime; ordinary mixins would
                    // already have produced E1118 at typeck.
                    for def in self.symbols.iter() {
                        if def.name == *name {
                            if matches!(
                                &def.kind,
                                DefKind::Trait { info } if matches!(
                                    info.dispatch_mode,
                                    crate::parser::ast::DispatchMode::Runtime
                                )
                            ) {
                                return Some(name.clone());
                            }
                            return None;
                        }
                    }
                    return None;
                }
                _ => return None,
            }
        }
    }
}

/// Map a primitive type name to its `Ty` variant.
///
/// Used by `primitive_self_ty_from_mangled` to recover the receiver type
/// for extension methods on primitives (`extension Int { def to_display }`).
/// Returns `None` for any name that isn't a primitive built-in.
pub(super) fn primitive_ty_by_name(name: &str) -> Option<Ty> {
    Some(match name {
        "Int" => Ty::Int,
        "Int8" => Ty::Int8,
        "Int16" => Ty::Int16,
        "Int32" => Ty::Int32,
        "Int64" => Ty::Int64,
        "UInt" => Ty::UInt,
        "UInt8" => Ty::UInt8,
        "UInt16" => Ty::UInt16,
        "UInt32" => Ty::UInt32,
        "UInt64" => Ty::UInt64,
        "ISize" => Ty::ISize,
        "USize" => Ty::USize,
        "Float" => Ty::Float,
        "Float32" => Ty::Float32,
        "Float64" => Ty::Float64,
        "Bool" => Ty::Bool,
        "Char" => Ty::Char,
        _ => return None,
    })
}
