use super::*;
use crate::implicit_includes::{derive_applies, peel_to_nominal};

impl<'a> Lowerer<'a> {
    fn instantiate_method_return_ty(&self, parent: DefId, receiver_ty: &Ty, return_ty: &Ty) -> Ty {
        use crate::resolve::symbols::DefKind;

        let generic_args = match receiver_ty.peel_refs() {
            Ty::Class { generic_args, .. }
            | Ty::Struct { generic_args, .. }
            | Ty::Enum { generic_args, .. } => generic_args,
            _ => return return_ty.clone(),
        };
        let Some(parent_def) = self.symbols.get(parent) else {
            return return_ty.clone();
        };
        let generic_params = match &parent_def.kind {
            DefKind::Class { info } => &info.generic_params,
            DefKind::Struct { info } => &info.generic_params,
            DefKind::Enum { info } => &info.generic_params,
            _ => return return_ty.clone(),
        };
        if generic_params.len() != generic_args.len() {
            return return_ty.clone();
        }
        let subst = generic_params
            .iter()
            .map(|param| param.name.clone())
            .zip(generic_args.iter().cloned())
            .collect();
        return_ty.subst_params(&subst)
    }

    /// Prefer a concrete MIR binding type when HIR left a variable reference
    /// abstract. Inlined collection closures are lowered in their caller, so
    /// their parameter can be refined from `T`/`Infer` to the collection's
    /// concrete element type even though the original closure HIR stays
    /// generic.
    pub(super) fn mir_refined_expr_ty(&self, expr: &HirExpr) -> Ty {
        fn is_abstract(ty: &Ty) -> bool {
            matches!(
                ty.peel_refs(),
                Ty::TypeParam { .. } | Ty::Infer(_) | Ty::Error
            )
        }

        if !is_abstract(&expr.ty) {
            return expr.ty.clone();
        }
        let HirExprKind::VarRef(def_id) = expr.kind else {
            return expr.ty.clone();
        };
        let Some(local_id) = self.def_to_local.get(&def_id) else {
            return expr.ty.clone();
        };
        self.fn_ref()
            .locals
            .get(*local_id as usize)
            .map(|local| local.ty.clone())
            .filter(|ty| !is_abstract(ty))
            .unwrap_or_else(|| expr.ty.clone())
    }

    /// Recover the declared return type of a call-shaped expression directly
    /// from its resolved method definition. This is an independent safety net
    /// for MIR locals whose inferred type is stale: a declared `String` return
    /// must never be sent through interpolation's integer fallback.
    pub(super) fn declared_method_return_ty(&self, expr: &HirExpr) -> Option<Ty> {
        use crate::resolve::symbols::DefKind;

        let resolved_method = match &expr.kind {
            HirExprKind::MethodCall { object, method, .. } if *method != UNRESOLVED_DEF => {
                let receiver_ty = self.mir_refined_expr_ty(object);
                self.symbols.get(*method).and_then(|def| match &def.kind {
                    DefKind::Method { parent, signature } => {
                        Some(self.instantiate_method_return_ty(
                            *parent,
                            &receiver_ty,
                            &signature.return_ty,
                        ))
                    }
                    _ => None,
                })
            }
            _ => None,
        };
        if resolved_method.is_some() {
            return resolved_method;
        }

        let (object, method_name, arg_count) = match &expr.kind {
            HirExprKind::MethodCall {
                object,
                method_name,
                args,
                block,
                ..
            } => (
                object.as_ref(),
                method_name.as_str(),
                args.len() + usize::from(block.is_some()),
            ),
            HirExprKind::FieldAccess {
                object, field_name, ..
            } => (object.as_ref(), field_name.as_str(), 0),
            _ => return None,
        };
        let receiver_ty = self.mir_refined_expr_ty(object);
        let receiver_name = type_name_from_ty(&receiver_ty);
        self.symbols
            .methods_named(&receiver_name, method_name)
            .find_map(|def| {
                let DefKind::Method { parent, signature } = &def.kind else {
                    return None;
                };
                (signature.params.len() == arg_count).then(|| {
                    self.instantiate_method_return_ty(*parent, &receiver_ty, &signature.return_ty)
                })
            })
    }

    /// Returns Some(type_name) if `ty` (peeling refs/aliases) is a struct
    /// **or class** whose declaration includes `derive Debug` (explicitly
    /// or structurally per ruby-naming.spec.md §3.6); otherwise None.
    ///
    /// Classes are included because `mir/lower/mod.rs` synthesises
    /// `{ClassName}_to_debug` alongside the struct one — without that,
    /// `"#{obj}"` on a class fell through every interpolation arm and
    /// printed the receiver's heap address (B74).
    pub(super) fn struct_with_derive_debug(&self, ty: &Ty) -> Option<String> {
        self.derived_nominal(ty, "Debug")
    }

    /// Phase 2 #06.C2: like `struct_with_derive_debug` but for enums.
    /// Returns the enum's name when `ty` resolves (through refs/
    /// aliases/newtypes) to an enum that structurally includes `Debug`.
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
                return self.enum_with_derive_trait(inner, trait_name);
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
            // alias `BufReader_File_new → ruxen_bufreader_new_file`.
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
        if self.derived_nominal(ty, trait_name).is_some() {
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

    /// The struct or class name behind `ty` (peeling references, aliases
    /// and newtypes) when a `{Name}_{method}` body exists for
    /// `trait_name`'s derive, synthesised or user-written. The one dispatch
    /// predicate every derived call site uses, so a site never names a body
    /// that was not emitted. Enums dispatch through
    /// [`Self::enum_with_derive_trait`] (variant-switch bodies).
    pub(super) fn derived_nominal(&self, ty: &Ty, trait_name: &str) -> Option<String> {
        derived_nominal(ty, self.symbols, trait_name)
    }
}

/// Free-function form of [`Lowerer::derived_nominal`], for callers that
/// hold only a `&SymbolTable`.
pub(super) fn derived_nominal(ty: &Ty, symbols: &SymbolTable, trait_name: &str) -> Option<String> {
    match peel_to_nominal(ty)? {
        nominal @ (Ty::Struct { name, .. } | Ty::Class { name, .. })
            if derive_applies(nominal, symbols, trait_name) =>
        {
            Some(name.clone())
        }
        _ => None,
    }
}

/// Returns true if the type is a string-like type whose runtime representation
/// is already a `char*` and needs no conversion for string interpolation.
pub(super) fn is_string_like(ty: &Ty) -> bool {
    match ty {
        Ty::String => true,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner)
        | Ty::Alias { target: inner, .. }
        | Ty::Newtype { inner, .. } => is_string_like(inner),
        _ => false,
    }
}

/// Extract a user-visible type name from a `Ty` for method mangling.
pub fn type_name_from_ty(ty: &Ty) -> String {
    match ty.peel_refs() {
        Ty::Class { name, .. } | Ty::Struct { name, .. } | Ty::Enum { name, .. } => name.clone(),
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

/// The nominal type name behind `ty` after peeling references, aliases
/// and newtypes, or `None` when `ty` is not a struct / class / enum.
///
/// Used to check that two operands of a structural comparison really are
/// the same nominal type before either is addressed slot-by-slot.
pub(super) fn peeled_nominal_name(ty: &Ty) -> Option<String> {
    match peel_to_nominal(ty)? {
        Ty::Struct { name, .. } | Ty::Class { name, .. } | Ty::Enum { name, .. } => {
            Some(name.clone())
        }
        _ => None,
    }
}

/// Number of header slots prepended to a class's allocation, addressed
/// by name. Free-function twin of
/// [`Lowerer::class_field_index_shift`] so the layout helpers below,
/// which only have a `&SymbolTable`, agree with the `Lowerer` methods on
/// where a class's first user field lives.
pub(super) fn class_field_index_shift_in(class_name: &str, symbols: &SymbolTable) -> usize {
    use crate::resolve::symbols::DefKind;
    for def in symbols.named(class_name) {
        if let DefKind::Class { info } = &def.kind {
            // v1 ships a single class_info_ptr header at slot 0.
            return usize::from(!info.runtime_dispatch_includes.is_empty());
        }
    }
    0
}

/// Return the ordered `(slot_index, field_ty)` list for a struct or a
/// class, or `None` if the name doesn't refer to either.
///
/// For a class this is the FLATTENED chain — every ancestor's fields
/// first, oldest ancestor first, then the class's own — shifted past the
/// `class_info` header. That is the layout `alloc_size` budgets, so the
/// inline structural-equality lowering reads the same slots the
/// constructor wrote. Walking `info.fields` alone would compare a
/// subclass's own fields and skip every inherited one.
pub(super) fn struct_field_layout(name: &str, symbols: &SymbolTable) -> Option<Vec<(usize, Ty)>> {
    use crate::resolve::symbols::DefKind;
    for def in symbols.named(name) {
        if let DefKind::Class { .. } = def.kind {
            return class_field_layout(name, symbols);
        }
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
    None
}

/// `(slot_index, field_ty)` for a class, ancestors first.
///
/// Mirrors `derive.rs::collect_all_class_fields` + the header shift;
/// they must stay in step, because the inline `==` lowering reads these
/// slots while the synthesised `{Class}_eq` reads that one. `None` when
/// `name` is not a class.
fn class_field_layout(name: &str, symbols: &SymbolTable) -> Option<Vec<(usize, Ty)>> {
    use crate::resolve::symbols::DefKind;

    let find_class = |n: &str| {
        symbols
            .named(n)
            .find(|d| matches!(d.kind, DefKind::Class { .. }))
    };

    // Oldest ancestor first, so slot order matches `alloc_size`.
    let mut chain: Vec<&str> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cur = Some(name.to_string());
    while let Some(n) = cur.take() {
        // Terminate on a malformed `class A < B` / `class B < A` cycle
        // rather than growing the chain forever.
        if !seen.insert(n.clone()) {
            break;
        }
        let def = find_class(&n)?;
        chain.push(def.name.as_str());
        if let DefKind::Class { info } = &def.kind {
            cur = info
                .parent
                .and_then(|p| symbols.get(p))
                .map(|d| d.name.clone());
        }
    }
    chain.reverse();

    let mut out = Vec::new();
    let mut slot = class_field_index_shift_in(name, symbols);
    for cls in chain {
        let Some(def) = find_class(cls) else { continue };
        let DefKind::Class { info } = &def.kind else {
            continue;
        };
        for &fid in &info.fields {
            if let Some(DefKind::Field { ty, .. }) = symbols.get(fid).map(|d| &d.kind) {
                out.push((slot, ty.clone()));
                slot += 1;
            }
        }
    }
    Some(out)
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
        // Generic-class monomorphization: a specialized body is named
        // `Base__mono__Args_method`. Strip the `__mono__Args` segment so the
        // prefix walk below sees the original `Base` (or its dotted form)
        // and recovers the receiver class for self-type derivation.
        if let Some(base) = crate::mir::lower::monomorphize::strip_mono_suffix(mangled) {
            if let Some(def) = self.symbols.type_def(base) {
                return Some(def.name.clone());
            }
        }
        let mut end = mangled.len();
        while let Some(pos) = mangled[..end].rfind('_') {
            let candidate = &mangled[..pos];
            if !candidate.is_empty() {
                if let Some(def) = self.symbols.type_def(candidate) {
                    return Some(def.name.clone());
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
        // Single implementation, shared with the free-function layout
        // helpers, so the two can never disagree on where a class's
        // first user field lives.
        class_field_index_shift_in(class_name, self.symbols)
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
                    if let Some(def) = self.symbols.named(name).next() {
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
