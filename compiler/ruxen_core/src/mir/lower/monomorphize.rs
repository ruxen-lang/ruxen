//! Generic-class monomorphization (option 1).
//!
//! Inside a generic class method the field/param types are
//! `Ty::TypeParam { name }`, so MIR binop lowering (`==`/`!=`) falls back
//! to a pointer compare and string-interpolation (`#{v}`) falls back to
//! `Int_fmt` (pointer-as-int) — neither dispatches to the concrete type's
//! `PartialEq`/`Display`. The existing `Ty::String` / `Ty::Array` / … fast
//! paths only fire on a *concrete* operand type.
//!
//! This pass specializes generic class method bodies per concrete
//! instantiation that actually appears in the program. After substituting
//! `T = String`, the field type becomes `Ty::String` and the existing
//! special-cases fire with zero changes to `binops.rs` / `interpolation.rs`.
//!
//! ── What is monomorphized ──
//! Only **user-defined methods** (`class.methods` + inline impl-block
//! methods), including the user `init`. Synthesized `clone`/`drop` are left
//! on the shared opaque body (call sites are NOT redirected for those), so
//! this pass never has to clone derive-synthesized MIR.
//!
//! ── Gating (the three regression surfaces) ──
//!   1. FFI-shell exclusion: a class with ANY `lib`-block method
//!      (`ffi_libs[*].functions[*].parent_type == class`) is skipped
//!      wholesale, so `Mutex[T]`/`Sender[T]`/`JoinHandle[T]`/… are never
//!      cloned and keep their uniform-word C ABI.
//!   2. Multi-param generics (`Pair[A,B]`) + `&T` returns: the substitution
//!      map carries every type-param name in declaration order, and the
//!      generalized `Ty::subst_params` recurses through `Ref*`.
//!   3. Only fully-concrete instantiations are recorded (no nested
//!      `TypeParam`/`Infer`), so we never emit a half-substituted body.
//!
//! ── Fallback ──
//! The opaque (un-suffixed) generic body is still emitted by the normal
//! `lower_item` path, so any call site whose receiver instantiation we did
//! NOT monomorphize (e.g. a generic arg that stayed a type param) keeps
//! resolving to it.

use super::derive::DeriveField;
use super::*;
use crate::hir::visit::{walk_expr, walk_item, Visit};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Marker separating the base class name from its concrete generic args in
/// a monomorphized mangled name. Chosen so it cannot collide with a real
/// source identifier path and is C-symbol-safe (alphanumerics + `_`).
/// Example: `Box[String]` method `eq` → `Box__mono__String_eq`.
const MONO_SEP: &str = "__mono__";

/// The callee MIR emits for a method call whose receiver is still a generic
/// parameter (`?T_to_display`). An unresolved inference variable spells the
/// same way through `Ty::Infer`'s Display (`?T3200_to_display`). Only an
/// un-monomorphised generic body emits one, and
/// `Lowerer::prune_unmonomorphised_bodies` guarantees none reaches codegen.
pub(super) fn abstract_callee(param: &str, method: &str) -> String {
    format!("{ABSTRACT_CALLEE_PREFIX}{param}_{method}")
}

/// First character of every abstract callee (see [`abstract_callee`]).
pub(super) const ABSTRACT_CALLEE_PREFIX: char = '?';

/// A recorded instantiation key: the concrete generic-arg vector for one
/// use of a generic class.
pub(super) type MonoKey = Vec<Ty>;

/// A lib-shell class's per-`T`-monomorphizable surface: the class's
/// generic parameter names (declaration order, so they zip against a head's
/// generic args) and the body methods that dispatch on them, directly or
/// through a call to another such method of the same shell.
struct BuiltinHeadShell {
    params: Vec<String>,
    methods: Vec<HirFuncDef>,
}

/// One `(receiver instantiation, method)` specialization to emit.
pub(super) struct BuiltinHeadMonoInstance {
    /// The owning shell's name (`Array`), the key into the eligible shells.
    class: String,
    /// The owning shell's generic parameter names, in declaration order.
    params: Vec<String>,
    /// The receiver's concrete generic arguments, in the same order.
    args: Vec<Ty>,
    /// The instantiation's STRUCTURAL type — `Ty::Array(Int)`, rebuilt by
    /// `Ty::from_builtin_head` rather than copied from whichever call site was seen
    /// first (the nominal `Ty::Class { name: "Array" }` spelling reaches the
    /// same symbol). Pinned as the specialized body's `self` type.
    self_ty: Ty,
    /// Emitted symbol (`Array__mono__Int_join`).
    symbol: String,
    def: HirFuncDef,
}

/// True when `ty`, peeled of references, IS one of `params` — i.e. a value of
/// the bare element type, whose MIR lowering is type-directed and therefore
/// wrong while the parameter is abstract.
fn is_bare_type_param(ty: &Ty, params: &[String]) -> bool {
    matches!(ty.peel_refs(), Ty::TypeParam { name, .. } if params.iter().any(|p| p == name))
}

/// True when `ty` mentions one of `params` anywhere. Used for the operand of a
/// binary operator, where lowering dispatches on the WHOLE operand type
/// (`Array[T] + Array[T]` routes differently from `Int + Int`), not just on a
/// bare element.
fn mentions_type_param(ty: &Ty, params: &[String]) -> bool {
    ty.any(&mut |t| matches!(t, Ty::TypeParam { name, .. } if params.contains(name)))
}

/// True when `func`'s body would be mis-lowered with `params` left abstract:
/// it feeds a type-parameter-typed value into one of MIR's three type-directed
/// lowerings. This is the eligibility gate for builtin-head specialization —
/// a purely structural property of the body, with no method-name knowledge.
///
///   * **interpolation** — `"#{x}"` picks a `{Type}_fmt` by the part's type;
///     abstract falls back to `Int_fmt` (pointer-as-int).
///   * **binary operator** — `a < b` / `a == b` pick a comparison by operand
///     type; abstract falls back to a machine compare of the raw words.
///   * **method / paren-less call on a bare element** — `x.to_s` mangles
///     `{Type}_to_s`; abstract mangles the type-parameter placeholder.
fn dispatches_on_type_param(func: &HirFuncDef, params: &[String]) -> bool {
    let mut found = false;
    walk_dispatch_sites_in_expr(&func.body, &mut |site| {
        found = found
            || match site {
                DispatchSite::Display(ty) | DispatchSite::Receiver(ty, _) => {
                    is_bare_type_param(ty, params)
                }
                DispatchSite::Operand(ty) => mentions_type_param(ty, params),
                DispatchSite::Function(..) | DispatchSite::Method(..) => false,
            };
    });
    found
}

/// One eligible generic method together with the receiver-qualified symbol
/// prefix used for emitted specializations (`Frame_measure`).
#[derive(Clone)]
pub(super) struct MonoMethod {
    pub(super) owner_symbol: String,
    pub(super) def: HirFuncDef,
}

/// Build the C-safe mangled *base* for a `(class, generic_args)`
/// instantiation, e.g. `Box__mono__String`, `Pair__mono__Int__String`.
/// Method callees are then `format!("{base}_{method}")`.
pub(super) fn mono_base(class: &str, args: &[Ty]) -> String {
    let mut s = String::with_capacity(class.len() + args.len() * 8);
    s.push_str(&class.replace('.', "_"));
    s.push_str(MONO_SEP);
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            s.push_str("__");
        }
        s.push_str(&mono_arg_token(a));
    }
    s
}

/// Render one generic-arg type as a symbol token. Mirrors
/// `type_name_from_ty` but flattens every non-identifier character to `_`
/// so the result is a legal C symbol fragment.
fn mono_arg_token(ty: &Ty) -> String {
    let raw = type_name_from_ty(ty);
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// If `mangled` is a monomorphized name (`Base__mono__Args_method`), return
/// the original base class name (`Base`, with `_` for the dotted form).
/// Used by `class_name_from_mangled` so self-type recovery in `lower_method`
/// resolves the receiver class for a specialized body.
pub(super) fn strip_mono_suffix(mangled: &str) -> Option<&str> {
    let idx = mangled.find(MONO_SEP)?;
    Some(&mangled[..idx])
}

/// True when `ty` (after peeling refs) contains no unresolved type — every
/// leaf is a concrete primitive / String / user type, with no `TypeParam`
/// or `Infer` anywhere. Only such instantiations are safe to specialize.
fn is_fully_concrete(ty: &Ty) -> bool {
    !ty.any(&mut |t| matches!(t, Ty::TypeParam { .. } | Ty::Infer(_)))
}

impl<'a> Lowerer<'a> {
    /// Populate `self.mono_classes` (eligible generic classes, by name →
    /// declaration) and `self.mono_instances` (class name → set of concrete
    /// generic-arg vectors seen at use sites). Runs once at the top of
    /// `lower_program`, after `collect_trait_impls` so name resolution is
    /// stable.
    pub(super) fn collect_mono_instances(&mut self, program: &HirProgram) {
        // 1. Classes excluded because they carry FFI `lib` methods.
        let mut ffi_classes: HashSet<String> = HashSet::new();
        for lib in &program.ffi_libs {
            for f in &lib.functions {
                if let Some(parent) = &f.parent_type {
                    ffi_classes.insert(parent.clone());
                }
            }
        }

        // 2. Eligible generic classes: user-defined, ≥1 type param, no FFI
        //    shell methods, and at least one user-defined method to clone.
        fn collect_classes(
            item: &HirItem,
            ffi_classes: &HashSet<String>,
            out: &mut HashMap<String, HirClassDef>,
        ) {
            match item {
                HirItem::Class(c) => {
                    let has_method = !c.methods.is_empty()
                        || c.impl_blocks
                            .iter()
                            .any(|b| b.items.iter().any(|i| matches!(i, HirImplItem::Method(_))));
                    if !c.generic_params.is_empty() && !ffi_classes.contains(&c.name) && has_method
                    {
                        out.insert(c.name.clone(), c.clone());
                    }
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        collect_classes(sub, ffi_classes, out);
                    }
                }
                // No generic-class instances to collect from these. Enumerated
                // explicitly (no `_`) to match the no-wildcard discipline of
                // `walk_tys_in_item` / `walk_tys_in_expr` below — a new HirItem
                // variant must be triaged here at compile time.
                HirItem::Function(_)
                | HirItem::Struct(_)
                | HirItem::Enum(_)
                | HirItem::Impl(_)
                | HirItem::Const(_)
                | HirItem::Mixin(_)
                | HirItem::TypeAlias(_)
                | HirItem::Newtype(_) => {}
            }
        }
        let mut classes: HashMap<String, HirClassDef> = HashMap::new();
        for item in &program.items {
            collect_classes(item, &ffi_classes, &mut classes);
        }

        // 3. Walk every HIR expression/type and record each fully-concrete
        //    `Class { name, generic_args }` whose `name` is eligible. The
        //    instantiation set is deduped via a BTreeSet keyed on the
        //    debug form of the arg vector (Ty is not Hash/Ord).
        let mut seen: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut instances: HashMap<String, Vec<MonoKey>> = HashMap::new();

        let mut record = |ty: &Ty, classes: &HashMap<String, HirClassDef>| {
            if let Ty::Class { name, generic_args } = ty {
                if !generic_args.is_empty()
                    && classes.contains_key(name)
                    && generic_args.iter().all(is_fully_concrete)
                {
                    // Dedup on the EMITTED mangled base, not the debug form of
                    // the arg vector: two `Ty` spellings of the same logical
                    // instantiation (e.g. `Ty::String` vs
                    // `Ty::Class{name:"String"}`) debug-format differently but
                    // collapse to the same `mono_base`, and emitting both would
                    // be a DuplicateDefinition.
                    let key = mono_base(name, generic_args);
                    if seen.entry(name.clone()).or_default().insert(key) {
                        instances
                            .entry(name.clone())
                            .or_default()
                            .push(generic_args.clone());
                    }
                }
            }
        };

        // Visit all type positions reachable from the program. We reuse the
        // type-bearing walk in `walk_tys_in_program`.
        walk_tys_in_program(program, &mut |ty| record(ty, &classes));

        // Precompute the set of bases we will emit, BEFORE `lower_items`
        // runs. Call-site rewriting (`mono_base_for_ty`) consults this set
        // while lowering bodies, so it must be populated now — not in
        // `emit_mono_instances`, which runs afterwards. The arity gate here
        // matches the one in `emit_mono_instances`, so the two agree on
        // exactly which bases exist.
        let mut emitted: HashSet<String> = HashSet::new();
        for (class_name, keys) in &instances {
            let Some(class) = classes.get(class_name) else {
                continue;
            };
            let arity = class.generic_params.len();
            for args in keys {
                if args.len() == arity {
                    emitted.insert(mono_base(class_name, args));
                }
            }
        }

        self.mono_classes = classes;
        self.mono_instances = instances;
        self.mono_emitted = emitted;

        // Record real (non-FFI) body methods on FFI-shell GENERIC builtin
        // classes (`Array[T]`, `Option[T]`, `Result[T,E]`). Such classes
        // are excluded from monomorphization above, so a body method emits
        // ONCE as an opaque `{Class}_{method}` (type params abstract). The
        // call site mangles the generic-suffixed `Array[Int]_map`; record
        // the stripped `Array_map` so `resolve_ffi_alias_callee` can route
        // the suffixed callee to the opaque body. (Closure combinators only
        // shuffle pointer/word values, so the abstract body is ABI-sound.)
        fn collect_lib_body_methods(
            item: &HirItem,
            ffi_classes: &HashSet<String>,
            trait_defaults: &HashMap<String, HashMap<String, HirFuncDef>>,
            out: &mut HashSet<String>,
        ) {
            match item {
                HirItem::Class(c) => {
                    if !c.generic_params.is_empty() && ffi_classes.contains(&c.name) {
                        let class_cs = c.name.replace('.', "_");
                        for m in &c.methods {
                            out.insert(format!("{}_{}", class_cs, m.name));
                        }
                        // Combinators pulled in via `include Mixin` (e.g.
                        // `class Array[T] include Enumerable[T]`) are emitted
                        // as opaque `{Class}_{method}` bodies by
                        // `lower_impl_block_with_outer_methods` (the trait-
                        // default monomorphization arm), exactly like an own
                        // body method. Register each included default the
                        // class does NOT itself override so the call site can
                        // route the generic-suffixed `Array[Int]_reduce`
                        // callee to the opaque body, same as the own-body
                        // surface above.
                        let own: HashSet<&str> =
                            c.methods.iter().map(|m| m.name.as_str()).collect();
                        for inner in &c.impl_blocks {
                            let Some(trait_ref) = &inner.trait_ref else {
                                continue;
                            };
                            if inner.negative_trait {
                                continue;
                            }
                            let Some(defaults) = trait_defaults.get(&trait_ref.name) else {
                                continue;
                            };
                            for mname in defaults.keys() {
                                if own.contains(mname.as_str()) {
                                    continue;
                                }
                                out.insert(format!("{}_{}", class_cs, mname));
                            }
                        }
                    }
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        collect_lib_body_methods(sub, ffi_classes, trait_defaults, out);
                    }
                }
                HirItem::Function(_)
                | HirItem::Struct(_)
                | HirItem::Enum(_)
                | HirItem::Impl(_)
                | HirItem::Const(_)
                | HirItem::Mixin(_)
                | HirItem::TypeAlias(_)
                | HirItem::Newtype(_) => {}
            }
        }
        let mut lib_body_methods: HashSet<String> = HashSet::new();
        for item in &program.items {
            collect_lib_body_methods(
                item,
                &ffi_classes,
                &self.trait_default_methods,
                &mut lib_body_methods,
            );
        }
        self.lib_body_methods = lib_body_methods;
    }

    /// Emit one monomorphized MIR copy of every user-defined method (incl.
    /// `init`) of each eligible generic class, for each recorded concrete
    /// instantiation. Records every emitted `(class, args)` base in
    /// `self.mono_emitted` so call-site rewriting only redirects to a name
    /// that actually exists.
    pub(super) fn emit_mono_instances(&mut self, mir: &mut MirProgram) -> Result<(), String> {
        // Snapshot to avoid borrowing `self` immutably while lowering
        // mutates it.
        let classes = self.mono_classes.clone();
        let instances = self.mono_instances.clone();

        for (class_name, keys) in &instances {
            let Some(class) = classes.get(class_name) else {
                continue;
            };
            // Param-name → arg position is declaration order.
            let param_names: Vec<String> = class
                .generic_params
                .iter()
                .map(|g| g.name.clone())
                .collect();

            // Gather the class's user-defined methods (body methods + inline
            // impl-block methods). These are never FFI shells.
            let mut methods: Vec<HirFuncDef> = class.methods.clone();
            for b in &class.impl_blocks {
                for item in &b.items {
                    if let HirImplItem::Method(m) = item {
                        methods.push(m.clone());
                    }
                }
            }
            let included_defaults = self.included_mixin_defaults(class, &methods);

            // Defensive: never emit the same mangled base twice even if two
            // keys slipped through collection (the symbol table rejects a
            // DuplicateDefinition). Collection already dedups on `mono_base`.
            let mut emitted_bases: HashSet<String> = HashSet::new();
            for args in keys {
                if args.len() != param_names.len() {
                    // Arity mismatch (should not happen post-typeck): skip
                    // rather than emit a malformed body.
                    continue;
                }
                let base = mono_base(class_name, args);
                if !emitted_bases.insert(base.clone()) {
                    continue;
                }
                let subst: HashMap<String, Ty> = param_names
                    .iter()
                    .cloned()
                    .zip(args.iter().cloned())
                    .collect();

                for method in &methods {
                    let mut cloned = method.clone();
                    crate::mir::lower::trait_default::subst_type_params_in_func(
                        &mut cloned,
                        &subst,
                    );
                    let mangled = format!("{}_{}", base, cloned.name);
                    let mir_fn = self.lower_method(&mangled, &cloned)?;
                    mir.functions.push(mir_fn);
                }
                for (default_fn, mixin_subst) in &included_defaults {
                    // The mixin's own params are spelled in the class's
                    // params (`include Enumerable[T]`), and `Self` is the
                    // class's target type: both resolve to concrete types
                    // through this instantiation's class substitution.
                    let default_subst: HashMap<String, Ty> = mixin_subst
                        .iter()
                        .map(|(k, v)| (k.clone(), v.subst_params(&subst)))
                        .collect();
                    let mut cloned = default_fn.clone();
                    crate::mir::lower::trait_default::subst_type_params_in_func(
                        &mut cloned,
                        &default_subst,
                    );
                    let mangled = format!("{}_{}", base, cloned.name);
                    let mir_fn = self.lower_method(&mangled, &cloned)?;
                    mir.functions.push(mir_fn);
                }
                // A call on this instance mangles to `<base>_clone` (and the
                // other derived methods) too, so the derived bodies are emitted
                // per instance, over the substituted field types.
                let class_ty = Ty::Class {
                    name: class.name.clone(),
                    generic_args: vec![],
                };
                let fields: Vec<DeriveField> = self
                    .derive_fields_of_class(class)
                    .into_iter()
                    .map(|f| DeriveField {
                        ty: f.ty.subst_params(&subst),
                        ..f
                    })
                    .collect();
                mir.functions.extend(self.synthesize_derives_as(
                    &class_ty,
                    &class.name,
                    &base,
                    &fields,
                ));
                // `self.mono_emitted` was already populated for `base` in
                // `collect_mono_instances` (it must exist before
                // `lower_items` so call sites can redirect); nothing to do
                // here.
            }
        }
        Ok(())
    }

    /// The default methods `class` gets from each `include`d mixin and does
    /// not define itself (`own`), each with the map from the mixin's generic
    /// params, and `Self`, to the class-level types the `include` names.
    /// The opaque class emits these through
    /// `lower_impl_block_with_outer_methods`; every monomorphised
    /// instantiation needs its own copy, or a call redirected to
    /// `<mono base>_<m>` has no body to link against (e2e 2062).
    fn included_mixin_defaults(
        &self,
        class: &HirClassDef,
        own: &[HirFuncDef],
    ) -> Vec<(HirFuncDef, HashMap<String, Ty>)> {
        let mut out = Vec::new();
        for block in &class.impl_blocks {
            let Some(trait_ref) = &block.trait_ref else {
                continue;
            };
            if block.negative_trait {
                continue;
            }
            let Some(defaults) = self.trait_default_methods.get(&trait_ref.name) else {
                continue;
            };
            let mixin_params: Vec<String> = self
                .symbols
                .named(&trait_ref.name)
                .find_map(|def| match &def.kind {
                    crate::resolve::symbols::DefKind::Trait { info } => {
                        Some(info.generic_params.iter().map(|g| g.name.clone()).collect())
                    }
                    _ => None,
                })
                .unwrap_or_default();
            let mut subst: HashMap<String, Ty> = mixin_params
                .into_iter()
                .zip(trait_ref.generic_args.iter().cloned())
                .collect();
            subst.insert("Self".to_string(), block.target_ty.clone());
            let mut names: Vec<&String> = defaults.keys().collect();
            names.sort();
            for name in names {
                if own.iter().any(|m| &m.name == name)
                    || block
                        .items
                        .iter()
                        .any(|i| matches!(i, HirImplItem::Method(m) if &m.name == name))
                {
                    continue;
                }
                out.push((defaults[name].clone(), subst.clone()));
            }
        }
        out
    }

    /// Given a method-call/constructor receiver type, return the
    /// monomorphic mangled *base* (`Box__mono__String`) when this exact
    /// instantiation was emitted; `None` otherwise (→ keep the opaque
    /// `{Class}_{method}` callee). Peels refs.
    pub(super) fn mono_base_for_ty(&self, ty: &Ty) -> Option<String> {
        match ty.peel_refs() {
            Ty::Class { name, generic_args } if !generic_args.is_empty() => {
                let base = mono_base(name, generic_args);
                self.mono_emitted.contains(&base).then_some(base)
            }
            _ => None,
        }
    }

    // ── Per-`T` monomorphization for BUILTIN-HEAD generic classes ──────────
    //
    // `Array[T]` / `Set[T]` / `Hash[K,V]` / `Option[T]` / `Result[T,E]` are
    // declared in `.rx` as generic classes but their INSTANCES are represented
    // by a structural `Ty` head (`Ty::Array(elem)`), not `Ty::Class`. They
    // carry `lib` blocks, so gating rule #1 above excludes them from
    // generic-CLASS monomorphization: each `.rx` body method emits ONCE with
    // its type params abstract (`Array_map`), and the call site routes the
    // generic-suffixed callee back to it (`lib_body_methods`).
    //
    // That is sound only while a body never DISPATCHES on its element type.
    // A body that interpolates an element (`"#{x}"`), compares two elements
    // (`a < b`, `a == b`) or calls a method on one lowers through the
    // type-directed MIR paths, and with `T` abstract those fall back to the
    // pointer-as-`Int` floor — `["a", "b"].join` printed the elements' buffer
    // ADDRESSES, and `["b", "a", "b"].uniq` kept all three (raw-word compare).
    //
    // This pass specializes exactly those bodies, per concrete receiver
    // instantiation observed at a call site: `Array__mono__String_join`. After
    // substituting `T = String` the body's element expressions are concrete and
    // the existing `binops.rs` / `interpolation.rs` fast paths fire unchanged —
    // the same trick as the generic-CLASS pass, keyed on a structural head.
    //
    // Deliberately NOT specialized: a body that only pushes / iterates /
    // closure-calls (`map`, `select`, `reduce`, …). Its abstract lowering is
    // already correct, so leaving it alone keeps this pass's blast radius to
    // the bodies that were previously mis-lowered.

    /// Collect every `(builtin head instantiation, method)` pair that a call
    /// site in the program actually needs a per-`T` specialization for, and
    /// record the mangled symbols so `builtin_head_mono_callee` can redirect
    /// those call sites. Runs alongside `collect_mono_instances`, BEFORE
    /// `lower_items` (call-site rewriting consults the emitted set while
    /// bodies are lowered).
    pub(super) fn collect_builtin_head_mono_instances(&mut self, program: &HirProgram) {
        // 1. Eligible methods: a body method (never an FFI `lib` decl —
        //    those live in `program.ffi_libs`, not `class.methods`) on a
        //    builtin-head class shell, whose body dispatches on one of the
        //    class's own type params. `collect` gathers every body method;
        //    step 1b filters to the eligible ones.
        fn collect(
            item: &HirItem,
            handled_by_class_mono: &HashMap<String, HirClassDef>,
            lib_shells: &HashSet<String>,
            out: &mut HashMap<String, BuiltinHeadShell>,
        ) {
            match item {
                HirItem::Class(c) => {
                    if c.generic_params.is_empty() || !lib_shells.contains(&c.name) {
                        return;
                    }
                    // Precedence: generic-CLASS monomorphization owns any class
                    // it accepted (a non-FFI generic class). Both passes mangle
                    // through `mono_base`, so specializing such a class here too
                    // would emit the same symbol twice — a DuplicateDefinition.
                    // In practice the builtin shells all carry `lib` blocks and
                    // are excluded there, so this only bites if a program
                    // declares its own non-FFI class under a head name.
                    if handled_by_class_mono.contains_key(&c.name) {
                        return;
                    }
                    let params: Vec<String> =
                        c.generic_params.iter().map(|g| g.name.clone()).collect();
                    let mut methods: Vec<HirFuncDef> = Vec::new();
                    for m in c.methods.iter().chain(
                        c.impl_blocks
                            .iter()
                            .flat_map(|b| b.items.iter())
                            .filter_map(|i| match i {
                                HirImplItem::Method(m) => Some(m),
                                _ => None,
                            }),
                    ) {
                        methods.push(m.clone());
                    }
                    if !methods.is_empty() {
                        let shell = out
                            .entry(c.name.clone())
                            .or_insert_with(|| BuiltinHeadShell {
                                params,
                                methods: Vec::new(),
                            });
                        shell.methods.extend(methods);
                    }
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        collect(sub, handled_by_class_mono, lib_shells, out);
                    }
                }
                HirItem::Function(_)
                | HirItem::Struct(_)
                | HirItem::Enum(_)
                | HirItem::Impl(_)
                | HirItem::Const(_)
                | HirItem::Mixin(_)
                | HirItem::TypeAlias(_)
                | HirItem::Newtype(_) => {}
            }
        }
        // Every generic class with `lib` methods is a shell: its body
        // methods are otherwise lowered once with their params abstract.
        self.lib_shell_classes = program
            .ffi_libs
            .iter()
            .flat_map(|lib| &lib.functions)
            .filter_map(|f| f.parent_type.clone())
            .collect();
        let mut shells: HashMap<String, BuiltinHeadShell> = HashMap::new();
        let handled_by_class_mono = self.mono_classes.clone();
        for item in &program.items {
            collect(
                item,
                &handled_by_class_mono,
                &self.lib_shell_classes,
                &mut shells,
            );
        }

        // 1b. Close eligibility over calls within the shell. A method that
        //     dispatches on `T` only by calling an eligible method of its own
        //     shell on a receiver that carries the shell's type params
        //     (`sort` building `out: Array[T]` and calling `out.sort!`) must
        //     be specialized too: its opaque body would call the opaque
        //     callee, whose element compare is a raw-word compare. Grows
        //     monotonically over a finite method set, so it terminates.
        for (class, shell) in shells.iter_mut() {
            let mut eligible: HashSet<String> = shell
                .methods
                .iter()
                .filter(|m| dispatches_on_type_param(m, &shell.params))
                .map(|m| m.name.clone())
                .collect();
            loop {
                let mut grew = false;
                for m in &shell.methods {
                    if eligible.contains(&m.name) {
                        continue;
                    }
                    let mut calls_eligible = false;
                    walk_dispatch_sites_in_expr(&m.body, &mut |site| {
                        let DispatchSite::Receiver(recv_ty, name) = site else {
                            return;
                        };
                        let Some((recv_class, recv_args)) = self.shell_head(recv_ty) else {
                            return;
                        };
                        if recv_class != *class
                            || !recv_args
                                .iter()
                                .any(|a| mentions_type_param(a, &shell.params))
                        {
                            return;
                        }
                        let canonical = self.symbols.canonical_method_name(class, name);
                        let overload_prefix = format!("{canonical}__overload");
                        calls_eligible = calls_eligible
                            || eligible
                                .iter()
                                .any(|e| e == canonical || e.starts_with(&overload_prefix));
                    });
                    if calls_eligible {
                        eligible.insert(m.name.clone());
                        grew = true;
                    }
                }
                if !grew {
                    break;
                }
            }
            shell.methods.retain(|m| eligible.contains(&m.name));
            // A `where` bound on the class's own generic makes the body
            // meaningful only per instantiation.
            for m in &shell.methods {
                let bounds_class_generic = m
                    .generic_params
                    .iter()
                    .any(|g| !g.bounds.is_empty() && shell.params.contains(&g.name));
                if bounds_class_generic {
                    self.specialized_only.insert(m.def_id);
                }
            }
        }
        shells.retain(|_, shell| !shell.methods.is_empty());
        if shells.is_empty() {
            return;
        }

        // 2. Discover the instantiations that are actually used: for every
        //    receiver-bearing call in the program, if the receiver is a
        //    fully-concrete builtin head whose class has an eligible method
        //    of that (alias-canonical) name, record one specialization.
        //
        //    Discovery starts from the SOURCE program, then follows calls
        //    inside each recorded specialization only while they stay on the
        //    SAME instantiation (`Array[Float]#sort` calling `out.sort!` on
        //    its `out: Array[Float]`). A specialized body that calls an
        //    eligible method on a DIFFERENT instantiation than any source call
        //    site uses (only reachable through a nested head such as
        //    `Array[Array[Int]]`) keeps the opaque callee, i.e. today's
        //    behavior. That keeps this pass terminating by construction: no
        //    specialization can breed a fresh element type, and each
        //    (instantiation, method) symbol is recorded at most once.
        let mut instances: Vec<BuiltinHeadMonoInstance> = Vec::new();
        let mut emitted: HashSet<String> = HashSet::new();
        for_each_fn_body(program, &mut |body| {
            walk_dispatch_sites_in_expr(body, &mut |call| {
                let DispatchSite::Receiver(recv_ty, name) = call else {
                    return;
                };
                let Some((class, args)) = self.shell_head(recv_ty) else {
                    return;
                };
                if !args.iter().all(is_fully_concrete) {
                    return;
                }
                self.record_builtin_head_instances(
                    &shells,
                    &class,
                    &args,
                    name,
                    &mut instances,
                    &mut emitted,
                );
            });
        });
        // A generic class's bodies are specialized per instantiation too
        // (`collect_mono_instances`): `State[Int]#peek` calling `g.get` on
        // its `MutexGuard[T]` needs `MutexGuard[Int]#get`.
        for (class_name, keys) in &self.mono_instances {
            let Some(class) = self.mono_classes.get(class_name) else {
                continue;
            };
            let params: Vec<String> = class
                .generic_params
                .iter()
                .map(|g| g.name.clone())
                .collect();
            let bodies: Vec<&HirFuncDef> = class
                .methods
                .iter()
                .chain(
                    class
                        .impl_blocks
                        .iter()
                        .flat_map(|b| b.items.iter())
                        .filter_map(|i| match i {
                            HirImplItem::Method(m) => Some(m),
                            _ => None,
                        }),
                )
                .collect();
            for args in keys {
                let subst: HashMap<String, Ty> =
                    params.iter().cloned().zip(args.iter().cloned()).collect();
                for def in &bodies {
                    walk_dispatch_sites_in_expr(&def.body, &mut |call| {
                        let DispatchSite::Receiver(recv_ty, name) = call else {
                            return;
                        };
                        let Some((shell, shell_args)) =
                            self.shell_head(&recv_ty.subst_params(&subst))
                        else {
                            return;
                        };
                        if !shell_args.iter().all(is_fully_concrete) {
                            return;
                        }
                        self.record_builtin_head_instances(
                            &shells,
                            &shell,
                            &shell_args,
                            name,
                            &mut instances,
                            &mut emitted,
                        );
                    });
                }
            }
        }
        let mut next = 0;
        while next < instances.len() {
            let class = instances[next].class.clone();
            let args = instances[next].args.clone();
            let subst: HashMap<String, Ty> = instances[next]
                .params
                .iter()
                .cloned()
                .zip(args.iter().cloned())
                .collect();
            let body = instances[next].def.body.clone();
            next += 1;
            let mut same_instantiation_calls: Vec<String> = Vec::new();
            walk_dispatch_sites_in_expr(&body, &mut |call| {
                let DispatchSite::Receiver(recv_ty, name) = call else {
                    return;
                };
                let Some((recv_class, recv_args)) = self.shell_head(recv_ty) else {
                    return;
                };
                if recv_class != class {
                    return;
                }
                let recv_args: Vec<Ty> = recv_args.iter().map(|a| a.subst_params(&subst)).collect();
                if recv_args == args {
                    same_instantiation_calls.push(name.to_string());
                }
            });
            for name in &same_instantiation_calls {
                self.record_builtin_head_instances(
                    &shells,
                    &class,
                    &args,
                    name,
                    &mut instances,
                    &mut emitted,
                );
            }
        }

        self.builtin_head_mono_instances = instances;
        self.builtin_head_mono_emitted = emitted;
    }

    /// Record one specialization per eligible `shells[class]` method that a
    /// call to `name` on the fully-concrete instantiation `class[args]`
    /// reaches (the alias-canonical body plus its overloads), skipping
    /// symbols already in `emitted`.
    fn record_builtin_head_instances(
        &self,
        shells: &HashMap<String, BuiltinHeadShell>,
        class: &str,
        args: &[Ty],
        name: &str,
        instances: &mut Vec<BuiltinHeadMonoInstance>,
        emitted: &mut HashSet<String>,
    ) {
        let Some(shell) = shells.get(class) else {
            return;
        };
        if shell.params.len() != args.len() {
            return;
        }
        // Ruby `alias new old` on the shell resolves to the canonical body
        // name, exactly as the call site's mangling does.
        let canonical = self.symbols.canonical_method_name(class, name);
        let overload_prefix = format!("{canonical}__overload");
        // Normalize the pinned `self` type to the STRUCTURAL head rather than
        // reusing the call site's spelling: the nominal `Ty::Class { name:
        // "Array" }` spelling reaches the same symbol, and whichever call site
        // happened to be visited first would otherwise decide the specialized
        // body's `self` type.
        let self_ty = Ty::from_builtin_head(class, args).unwrap_or_else(|| Ty::Class {
            name: class.to_string(),
            generic_args: args.to_vec(),
        });
        for def in &shell.methods {
            if def.name != canonical && !def.name.starts_with(&overload_prefix) {
                continue;
            }
            let symbol = format!("{}_{}", mono_base(class, args), def.name);
            if !emitted.insert(symbol.clone()) {
                continue;
            }
            instances.push(BuiltinHeadMonoInstance {
                class: class.to_string(),
                params: shell.params.clone(),
                args: args.to_vec(),
                self_ty: self_ty.clone(),
                symbol,
                def: def.clone(),
            });
        }
    }

    /// Emit the specialized bodies recorded above. Runs after `lower_items`,
    /// alongside `emit_mono_instances`, so the opaque fallback bodies are
    /// already in place for every instantiation this pass did not cover.
    pub(super) fn emit_builtin_head_mono_instances(
        &mut self,
        mir: &mut MirProgram,
    ) -> Result<(), String> {
        let instances = std::mem::take(&mut self.builtin_head_mono_instances);
        for inst in &instances {
            let subst: HashMap<String, Ty> = inst
                .params
                .iter()
                .cloned()
                .zip(inst.args.iter().cloned())
                .collect();
            let mut cloned = inst.def.clone();
            crate::mir::lower::trait_default::subst_type_params_in_func(&mut cloned, &subst);
            // The mangled name (`Array__mono__Int_join`) strips to the class
            // shell `Array`, which is NOT the receiver's runtime type — the
            // receiver is the structural head `Ty::Array(Int)`. Pin it, the
            // same way `emit_universal_kernel_bodies` pins a synthesized
            // `Array[Int]_tap` receiver.
            self.self_ty_override = Some(inst.self_ty.clone());
            let lowered = self.lower_method(&inst.symbol, &cloned);
            self.self_ty_override = None;
            mir.functions.push(lowered?);
        }
        Ok(())
    }

    /// The lib shell class a receiver's methods live on, with its generic
    /// args: a builtin head (`Ty::Array(Int)` is `Array[Int]`) or an
    /// instance of a generic class with `lib` methods (`MutexGuard[String]`).
    fn shell_head(&self, ty: &Ty) -> Option<(String, Vec<Ty>)> {
        if let Some((class, args)) = ty.builtin_head() {
            return Some((class.to_string(), args));
        }
        match ty.peel_refs() {
            Ty::Class { name, generic_args }
                if !generic_args.is_empty() && self.lib_shell_classes.contains(name) =>
            {
                Some((name.clone(), generic_args.clone()))
            }
            _ => None,
        }
    }

    /// At a receiver-bearing call site, the specialized callee for this
    /// `(receiver instantiation, method)` when one was emitted; `None`
    /// otherwise (→ keep the opaque `{Class}_{method}` callee).
    pub(super) fn builtin_head_mono_callee(
        &self,
        receiver_ty: &Ty,
        method_name: &str,
    ) -> Option<String> {
        if self.builtin_head_mono_emitted.is_empty() {
            return None;
        }
        let (class, args) = self.shell_head(receiver_ty)?;
        if !args.iter().all(is_fully_concrete) {
            return None;
        }
        let symbol = format!("{}_{}", mono_base(&class, &args), method_name);
        self.builtin_head_mono_emitted
            .contains(&symbol)
            .then_some(symbol)
    }

    // ── Q17: generic FREE-FUNCTION / mixin-bound monomorphization ──────────
    //
    // A generic free function whose type param is mixin-bounded
    // (`def paint_all[T: Paintable](s: &var T, …)`) is lowered ONCE with `T`
    // abstract, so a call to a bound method inside its body
    // (`s.fill_rect(…)`) mangles to the literal bound placeholder
    // `T: Paintable_fill_rect`, which link-fails. The single-implementor
    // case is masked only because `unique_bound_impl` devirtualizes it.
    //
    // We specialize one body per concrete instantiation seen at a call site
    // (`paint_all__mono__TallySurface`); after substituting `T = TallySurface`
    // the receiver param's type is concrete, so the body's `s.fill_rect(…)`
    // lowers through the ordinary `Ty::Class` dispatch arm — no new dispatch
    // logic. Mirrors the generic-CLASS machinery above; reuses `mono_base`
    // and `subst_type_params_in_func`.

    /// Populate `self.mono_fns` (eligible generic free functions, by name →
    /// declaration) and `self.fn_mono_instances` (function name → set of
    /// concrete generic-arg vectors recovered at call sites). Runs once in
    /// `lower_program`, after `collect_mono_instances` (so trait impls are
    /// already known for the no-monomorphization-needed gate).
    pub(super) fn collect_generic_fn_instances(&mut self, program: &HirProgram) {
        // 1. Eligible generic free functions: a top-level (or module-nested)
        //    `def` with ≥1 type param, where at least one type param is
        //    MIXIN-BOUNDED (an unbounded `[T]` param is never dispatched on,
        //    so it needs no specialization — the abstract body is sound, like
        //    the closure-combinator `Array_map` case).
        fn collect_fns(item: &HirItem, out: &mut HashMap<String, HirFuncDef>) {
            match item {
                HirItem::Function(f) => {
                    let has_bounded_param = f.generic_params.iter().any(|g| !g.bounds.is_empty());
                    if !f.generic_params.is_empty() && has_bounded_param {
                        out.insert(f.name.clone(), f.clone());
                    }
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        collect_fns(sub, out);
                    }
                }
                HirItem::Class(_)
                | HirItem::Struct(_)
                | HirItem::Enum(_)
                | HirItem::Impl(_)
                | HirItem::Const(_)
                | HirItem::Mixin(_)
                | HirItem::TypeAlias(_)
                | HirItem::Newtype(_) => {}
            }
        }
        let mut fns: HashMap<String, HirFuncDef> = HashMap::new();
        for item in &program.items {
            collect_fns(item, &mut fns);
        }

        // 2. Discover instantiations by a WORKLIST FIXPOINT (demand-driven).
        //    For each call to an eligible generic fn, recover `{ param →
        //    concrete }` by unifying the callee's declared param types against
        //    the actual arg `.ty`s, and record the concrete arg vector when
        //    EVERY generic param resolved to a fully-concrete type.
        //
        //    Generic-CALLING-generic (`render_twice[T] { render(d) }`) needs
        //    transitivity: scanning the SOURCE program sees `render(d)` with an
        //    abstract `&var T` arg, so `render`'s instantiation is invisible
        //    until `render_twice` is itself specialized. So after recording an
        //    instantiation we substitute that fn's body and re-scan the
        //    SPECIALIZED body — its inner calls now carry concrete arg types.
        //    The worklist runs to a fixpoint; deduping on the mangled base
        //    guarantees termination (finitely many concrete instantiations).
        let mut seen: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut instances: HashMap<String, Vec<MonoKey>> = HashMap::new();
        // Recover the concrete instantiation for one call, if fully concrete.
        let recover = |callee_name: &str, arg_tys: &[Ty]| -> Option<(String, MonoKey)> {
            let def = fns.get(callee_name)?;
            let param_names: Vec<String> =
                def.generic_params.iter().map(|g| g.name.clone()).collect();
            let mut subst: HashMap<String, Ty> = HashMap::new();
            for (p, a) in def.params.iter().zip(arg_tys.iter()) {
                unify_collect_type_params(&p.ty, a, &mut subst);
            }
            let all_concrete = param_names
                .iter()
                .all(|n| subst.get(n).map(is_fully_concrete).unwrap_or(false));
            if !all_concrete {
                return None;
            }
            let arg_vec: MonoKey = param_names
                .iter()
                .map(|n| subst.get(n).cloned().unwrap())
                .collect();
            Some((callee_name.to_string(), arg_vec))
        };

        // Seed the worklist from the source program's call sites. The
        // fixpoint below re-discovers instantiations reached only through
        // specialized generic bodies (Q17 generic-calling-generic).
        let mut worklist: Vec<(String, MonoKey)> = Vec::new();
        let seed = |callee_name: &str,
                    args: &[HirExpr],
                    seen: &mut HashMap<String, BTreeSet<String>>,
                    instances: &mut HashMap<String, Vec<MonoKey>>,
                    worklist: &mut Vec<(String, MonoKey)>| {
            let arg_tys: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
            if let Some(inst) = recover(callee_name, &arg_tys) {
                let key = mono_base(&inst.0, &inst.1);
                if seen.entry(inst.0.clone()).or_default().insert(key) {
                    instances
                        .entry(inst.0.clone())
                        .or_default()
                        .push(inst.1.clone());
                    worklist.push(inst);
                }
            }
        };
        for_each_fn_body(program, &mut |body| {
            walk_dispatch_sites_in_expr(body, &mut |call| {
                if let DispatchSite::Function(callee_name, args) = call {
                    seed(callee_name, args, &mut seen, &mut instances, &mut worklist);
                }
            });
        });

        // Fixpoint: for each newly-recorded instantiation, substitute the
        // fn's body and scan the SPECIALIZED body for further generic calls
        // (whose arg types are now concrete after substitution).
        while let Some((fn_name, args)) = worklist.pop() {
            let Some(def) = fns.get(&fn_name) else {
                continue;
            };
            let param_names: Vec<String> =
                def.generic_params.iter().map(|g| g.name.clone()).collect();
            if args.len() != param_names.len() {
                continue;
            }
            let subst: HashMap<String, Ty> = param_names
                .iter()
                .cloned()
                .zip(args.iter().cloned())
                .collect();
            let mut cloned = def.clone();
            crate::mir::lower::trait_default::subst_type_params_in_func(&mut cloned, &subst);
            // Scan the substituted body's calls; their arg types are concrete
            // after substitution.
            let mut discovered: Vec<(String, MonoKey)> = Vec::new();
            walk_dispatch_sites_in_expr(&cloned.body, &mut |call| {
                if let DispatchSite::Function(callee_name, call_args) = call {
                    let arg_tys: Vec<Ty> = call_args.iter().map(|a| a.ty.clone()).collect();
                    if let Some(inst) = recover(callee_name, &arg_tys) {
                        discovered.push(inst);
                    }
                }
            });
            for inst in discovered {
                let key = mono_base(&inst.0, &inst.1);
                if seen.entry(inst.0.clone()).or_default().insert(key) {
                    instances
                        .entry(inst.0.clone())
                        .or_default()
                        .push(inst.1.clone());
                    worklist.push(inst);
                }
            }
        }

        // Pre-populate the `(fn, args) → mangled` redirect table BEFORE
        // `lower_items` runs, because call-site rewriting (`fn_mono_callee`)
        // consults it while lowering bodies — exactly as `collect_mono_
        // instances` pre-populates `mono_emitted` for the same reason.
        // `emit_generic_fn_instances` (which runs after `lower_items`) emits
        // the bodies for precisely these entries.
        let mut emitted: HashMap<String, Vec<(MonoKey, String)>> = HashMap::new();
        for (fn_name, keys) in &instances {
            let Some(def) = fns.get(fn_name) else {
                continue;
            };
            let arity = def.generic_params.len();
            for args in keys {
                if args.len() == arity {
                    let base = mono_base(fn_name, args);
                    emitted
                        .entry(fn_name.clone())
                        .or_default()
                        .push((args.clone(), base));
                }
            }
        }

        // The opaque (un-monomorphized) body of an eligible generic free fn is
        // ALWAYS suppressed — it can only ever emit bound-placeholder callees
        // (`T: Paintable_fill_rect`) that link-fail, and it is never the body a
        // valid call resolves to:
        //   * a CONCRETE-context call (from a non-generic fn) is redirected to
        //     a monomorphic copy by `fn_mono_callee`; if its type args cannot
        //     be recovered, that is a genuine "cannot monomorphize" error, and
        //     `fn_call.rs` surfaces a clear diagnostic at the call site rather
        //     than emitting the opaque callee;
        //   * an ABSTRACT-context call (from inside another generic fn's body)
        //     is resolved by THAT enclosing fn's own specialized copy — the
        //     opaque enclosing body is itself suppressed, so the abstract call
        //     is never lowered.
        // `generic_fn_keeps_opaque` returns false for every eligible generic
        // fn, suppressing all of them.

        self.mono_fns = fns;
        self.fn_mono_instances = instances;
        self.fn_mono_emitted = emitted;
    }

    // ── Q17b: generic METHOD / mixin-bound monomorphization ─────────────

    /// Collect concrete instantiations of methods that declare at least one
    /// mixin-bounded type parameter. Unlike generic-class monomorphization,
    /// the receiver is fixed (`Frame`); only the method's own type parameters
    /// are recovered by unifying its declared parameters with call arguments.
    pub(super) fn collect_generic_method_instances(&mut self, program: &HirProgram) {
        // A method's OWN bounded generic: a `where` predicate on the owning
        // type's generic (`def get -> T where T: Clone` in `class
        // MutexGuard[T]`) is not one, the receiver's instantiation fixes it
        // (`collect_builtin_head_mono_instances`).
        fn eligible(method: &HirFuncDef, owner_generics: &[HirGenericParam]) -> bool {
            method
                .generic_params
                .iter()
                .any(|g| !g.bounds.is_empty() && !owner_generics.iter().any(|o| o.name == g.name))
        }

        fn collect_methods(
            item: &HirItem,
            module_path: &mut Vec<String>,
            out: &mut HashMap<DefId, MonoMethod>,
        ) {
            let mut record = |owner: &str, generics: &[HirGenericParam], methods: &[HirFuncDef]| {
                let qualified = if module_path.is_empty() {
                    owner.to_string()
                } else {
                    format!("{}.{}", module_path.join("."), owner)
                };
                let owner_symbol = qualified.replace('.', "_");
                for method in methods {
                    if eligible(method, generics) {
                        out.insert(
                            method.def_id,
                            MonoMethod {
                                owner_symbol: owner_symbol.clone(),
                                def: method.clone(),
                            },
                        );
                    }
                }
            };

            match item {
                HirItem::Class(c) => record(&c.name, &c.generic_params, &c.methods),
                HirItem::Struct(s) => record(&s.name, &s.generic_params, &s.methods),
                HirItem::Enum(e) => record(&e.name, &e.generic_params, &e.methods),
                HirItem::Module(m) => {
                    module_path.push(m.name.clone());
                    for sub in &m.items {
                        collect_methods(sub, module_path, out);
                    }
                    module_path.pop();
                }
                HirItem::Function(_)
                | HirItem::Impl(_)
                | HirItem::Const(_)
                | HirItem::Mixin(_)
                | HirItem::TypeAlias(_)
                | HirItem::Newtype(_) => {}
            }
        }

        let mut methods = HashMap::new();
        let mut module_path = Vec::new();
        for item in &program.items {
            collect_methods(item, &mut module_path, &mut methods);
        }

        let mut seen: HashMap<DefId, BTreeSet<String>> = HashMap::new();
        let mut instances: HashMap<DefId, Vec<MonoKey>> = HashMap::new();
        for_each_fn_body(program, &mut |body| {
            walk_dispatch_sites_in_expr(body, &mut |call| {
                let DispatchSite::Method(method_id, args) = call else {
                    return;
                };
                let Some(method) = methods.get(&method_id) else {
                    return;
                };
                let param_names: Vec<String> = method
                    .def
                    .generic_params
                    .iter()
                    .map(|g| g.name.clone())
                    .collect();
                let mut subst = HashMap::new();
                for (param, arg) in method.def.params.iter().zip(args.iter()) {
                    unify_collect_type_params(&param.ty, &arg.ty, &mut subst);
                }
                let Some(arg_vec) = param_names
                    .iter()
                    .map(|name| subst.get(name).cloned())
                    .collect::<Option<MonoKey>>()
                else {
                    return;
                };
                if !arg_vec.iter().all(is_fully_concrete) {
                    return;
                }
                let method_base = format!("{}_{}", method.owner_symbol, method.def.name);
                let emitted_name = mono_base(&method_base, &arg_vec);
                if seen.entry(method_id).or_default().insert(emitted_name) {
                    instances.entry(method_id).or_default().push(arg_vec);
                }
            });
        });

        // Populate redirects before normal lowering. Eligible opaque method
        // bodies are suppressed, so every valid call must resolve to one of
        // these concrete symbols or receive the explicit residual diagnostic.
        let mut emitted = HashMap::new();
        for (method_id, keys) in &instances {
            let Some(method) = methods.get(method_id) else {
                continue;
            };
            let method_base = format!("{}_{}", method.owner_symbol, method.def.name);
            for args in keys {
                if args.len() == method.def.generic_params.len() {
                    emitted
                        .entry(*method_id)
                        .or_insert_with(Vec::new)
                        .push((args.clone(), mono_base(&method_base, args)));
                }
            }
        }

        self.mono_methods = methods;
        self.method_mono_instances = instances;
        self.method_mono_emitted = emitted;
    }

    /// Emit one specialized self-bearing MIR body for every concrete generic
    /// method instantiation recorded above.
    pub(super) fn emit_generic_method_instances(
        &mut self,
        mir: &mut MirProgram,
    ) -> Result<(), String> {
        let methods = self.mono_methods.clone();
        let instances = self.method_mono_instances.clone();
        for (method_id, keys) in &instances {
            let Some(method) = methods.get(method_id) else {
                continue;
            };
            let param_names: Vec<String> = method
                .def
                .generic_params
                .iter()
                .map(|g| g.name.clone())
                .collect();
            let method_base = format!("{}_{}", method.owner_symbol, method.def.name);
            let mut emitted_names = HashSet::new();
            for args in keys {
                if args.len() != param_names.len() {
                    continue;
                }
                let mangled = mono_base(&method_base, args);
                if !emitted_names.insert(mangled.clone()) {
                    continue;
                }
                let subst: HashMap<String, Ty> = param_names
                    .iter()
                    .cloned()
                    .zip(args.iter().cloned())
                    .collect();
                let mut cloned = method.def.clone();
                crate::mir::lower::trait_default::subst_type_params_in_func(&mut cloned, &subst);
                mir.functions.push(self.lower_method(&mangled, &cloned)?);
            }
        }
        Ok(())
    }

    /// Resolve a generic method call to its emitted monomorphic symbol.
    pub(super) fn method_mono_callee(&self, method_id: DefId, arg_tys: &[Ty]) -> Option<String> {
        let method = self.mono_methods.get(&method_id)?;
        let emitted = self.method_mono_emitted.get(&method_id)?;
        let param_names: Vec<String> = method
            .def
            .generic_params
            .iter()
            .map(|g| g.name.clone())
            .collect();
        let mut subst = HashMap::new();
        for (param, actual) in method.def.params.iter().zip(arg_tys.iter()) {
            unify_collect_type_params(&param.ty, actual, &mut subst);
        }
        let args: MonoKey = param_names
            .iter()
            .map(|name| subst.get(name).cloned())
            .collect::<Option<_>>()?;
        if !args.iter().all(is_fully_concrete) {
            return None;
        }
        let method_base = format!("{}_{}", method.owner_symbol, method.def.name);
        let wanted = mono_base(&method_base, &args);
        emitted
            .iter()
            .find(|(_, symbol)| *symbol == wanted)
            .map(|(_, symbol)| symbol.clone())
    }

    pub(super) fn generic_method_keeps_opaque(&self, method_id: DefId) -> bool {
        !self.mono_methods.contains_key(&method_id) && !self.specialized_only.contains(&method_id)
    }

    /// Whether `method_id` exists only as per-instantiation copies (see
    /// `Lowerer::specialized_only`).
    pub(super) fn is_specialized_only(&self, method_id: DefId) -> bool {
        self.specialized_only.contains(&method_id)
    }

    pub(super) fn is_generic_mixin_method(&self, method_id: DefId) -> bool {
        self.mono_methods.contains_key(&method_id)
    }

    /// Emit one monomorphized copy of every eligible generic free function,
    /// per recorded concrete instantiation, and record each `(fn, args) →
    /// mangled` redirect in `self.fn_mono_emitted` so call-site lowering can
    /// rewrite the callee. Runs alongside `emit_mono_instances`.
    pub(super) fn emit_generic_fn_instances(&mut self, mir: &mut MirProgram) -> Result<(), String> {
        let fns = self.mono_fns.clone();
        let instances = self.fn_mono_instances.clone();

        for (fn_name, keys) in &instances {
            let Some(def) = fns.get(fn_name) else {
                continue;
            };
            let param_names: Vec<String> =
                def.generic_params.iter().map(|g| g.name.clone()).collect();
            let mut emitted_bases: HashSet<String> = HashSet::new();
            for args in keys {
                if args.len() != param_names.len() {
                    continue;
                }
                let base = mono_base(fn_name, args);
                if !emitted_bases.insert(base.clone()) {
                    continue;
                }
                let subst: HashMap<String, Ty> = param_names
                    .iter()
                    .cloned()
                    .zip(args.iter().cloned())
                    .collect();
                let mut cloned = def.clone();
                crate::mir::lower::trait_default::subst_type_params_in_func(&mut cloned, &subst);
                // The specialized body is a plain free function (no `self`),
                // so it lowers through `lower_function`, not `lower_method`.
                // `self.fn_mono_emitted[fn_name]` was already populated for
                // `base` in `collect_generic_fn_instances` (it must exist
                // before `lower_items` so call sites can redirect); nothing to
                // record here.
                cloned.name = base.clone();
                let mir_fn = self.lower_function(&cloned)?;
                mir.functions.push(mir_fn);
            }
        }
        Ok(())
    }

    /// At a free-function call site, return the mangled monomorphic callee
    /// when the call's concrete type arguments match an emitted
    /// instantiation; `None` otherwise (→ keep the opaque `callee_name`).
    /// `arg_tys` are the call's actual argument types, in order.
    pub(super) fn fn_mono_callee(&self, callee_name: &str, arg_tys: &[Ty]) -> Option<String> {
        let def = self.mono_fns.get(callee_name)?;
        let emitted = self.fn_mono_emitted.get(callee_name)?;
        let param_names: Vec<String> = def.generic_params.iter().map(|g| g.name.clone()).collect();
        let mut subst: HashMap<String, Ty> = HashMap::new();
        for (p, a) in def.params.iter().zip(arg_tys.iter()) {
            unify_collect_type_params(&p.ty, a, &mut subst);
        }
        let arg_vec: Vec<Ty> = param_names
            .iter()
            .map(|n| subst.get(n).cloned())
            .collect::<Option<Vec<_>>>()?;
        if !arg_vec.iter().all(is_fully_concrete) {
            return None;
        }
        // Match against the emitted set by the mangled base (the same dedup
        // key collection used), so two spellings of one logical instantiation
        // resolve identically.
        let want = mono_base(callee_name, &arg_vec);
        emitted
            .iter()
            .find(|(_, base)| *base == want)
            .map(|(_, base)| base.clone())
    }

    /// True when the opaque (un-monomorphized) body of `fn_name` must still be
    /// emitted by the normal `lower_item` path. For an ELIGIBLE generic free fn
    /// the answer is always `false`: its opaque body is unconditionally
    /// suppressed (see the rationale in `collect_generic_fn_instances`). A
    /// non-eligible name is not ours to suppress — return `true`.
    pub(super) fn generic_fn_keeps_opaque(&self, fn_name: &str) -> bool {
        !self.mono_fns.contains_key(fn_name)
    }

    /// True when `fn_name` is an eligible generic free function. Called only
    /// after `fn_mono_callee` found no emitted concrete match; such a call can
    /// never fall back to the suppressed opaque body, so `fn_call.rs` must
    /// surface a clear diagnostic.
    pub(super) fn generic_fn_unmonomorphizable(&self, fn_name: &str) -> bool {
        self.mono_fns.contains_key(fn_name)
    }
}

/// Unify a declared type (which may mention `Ty::TypeParam`) against a
/// concrete actual type, recording each `param-name → concrete-ty` binding
/// into `out`. Structural and conservative: it descends through refs and the
/// common generic containers, binding a `TypeParam` leaf to whatever concrete
/// type sits opposite it. A shape mismatch (e.g. declared `Array[T]` vs an
/// actual `Int`) simply contributes no binding for that position — the caller
/// gates on "every param bound to a concrete type", so an unbindable param
/// safely blocks specialization rather than emitting a half-substituted body.
fn unify_collect_type_params(decl: &Ty, actual: &Ty, out: &mut HashMap<String, Ty>) {
    // Peel matching reference layers on both sides first; also peel a
    // reference on just the actual side (a `&var T` param receives a
    // `&var Concrete` arg — same depth — but be tolerant of one-sided refs).
    let decl = decl.peel_refs();
    let actual = actual.peel_refs();

    match (decl, actual) {
        (Ty::TypeParam { name, .. }, concrete) => {
            // Don't bind to another unresolved type param / infer var.
            if is_fully_concrete(concrete) {
                out.entry(name.clone()).or_insert_with(|| concrete.clone());
            }
        }
        (Ty::Array(d), Ty::Array(a))
        | (Ty::Set(d), Ty::Set(a))
        | (Ty::Option(d), Ty::Option(a))
        | (Ty::FixedArray(d, _), Ty::FixedArray(a, _)) => {
            unify_collect_type_params(d, a, out);
        }
        (Ty::Map(dk, dv), Ty::Map(ak, av)) => {
            unify_collect_type_params(dk, ak, out);
            unify_collect_type_params(dv, av, out);
        }
        (Ty::Result(da, db), Ty::Result(aa, ab)) => {
            unify_collect_type_params(da, aa, out);
            unify_collect_type_params(db, ab, out);
        }
        (Ty::Tuple(de), Ty::Tuple(ae)) if de.len() == ae.len() => {
            for (d, a) in de.iter().zip(ae.iter()) {
                unify_collect_type_params(d, a, out);
            }
        }
        (
            Ty::Class {
                generic_args: dargs,
                ..
            },
            Ty::Class {
                generic_args: aargs,
                ..
            },
        )
        | (
            Ty::Struct {
                generic_args: dargs,
                ..
            },
            Ty::Struct {
                generic_args: aargs,
                ..
            },
        )
        | (
            Ty::Enum {
                generic_args: dargs,
                ..
            },
            Ty::Enum {
                generic_args: aargs,
                ..
            },
        ) if dargs.len() == aargs.len() => {
            for (d, a) in dargs.iter().zip(aargs.iter()) {
                unify_collect_type_params(d, a, out);
            }
        }
        // No type param to bind here (or a shape mismatch) — contribute
        // nothing; the caller's all-concrete gate handles the consequence.
        _ => {}
    }
}

/// Invoke `f(body)` for every function / method body in the program, and
/// for every const initializer.
fn for_each_fn_body(program: &HirProgram, f: &mut impl FnMut(&HirExpr)) {
    struct Bodies<'f, F> {
        f: &'f mut F,
    }
    impl<F: FnMut(&HirExpr)> Visit for Bodies<'_, F> {
        fn visit_func(&mut self, func: &HirFuncDef) {
            (self.f)(&func.body);
        }
        // `walk_item` reaches `visit_expr` only for a const initializer
        // (function bodies stop at `visit_func` above).
        fn visit_expr(&mut self, expr: &HirExpr) {
            (self.f)(expr);
        }
    }
    let mut bodies = Bodies { f };
    for it in &program.items {
        bodies.visit_item(it);
    }
}

/// A site in a body where MIR lowering makes a decision from a static type:
/// a call (whose callee symbol is chosen from the callee/receiver) or an
/// operand of one of the type-directed lowerings. Reported by
/// `walk_dispatch_sites_in_expr`, which the monomorphization passes share.
enum DispatchSite<'a> {
    Function(&'a str, &'a [HirExpr]),
    Method(DefId, &'a [HirExpr]),
    /// Any receiver-bearing call — `recv.name(...)` (a `MethodCall`) or the
    /// paren-less `recv.name` shorthand (a `FieldAccess`, which MIR lowers as
    /// a call in `field_access.rs`). Carries the receiver's type and the
    /// source-level method name. Used by builtin-head monomorphization, which
    /// keys on the RECEIVER instantiation rather than on argument types.
    Receiver(&'a Ty, &'a str),
    /// A binary-operator operand: `binops.rs` selects the operation from the
    /// operand types.
    Operand(&'a Ty),
    /// A string-interpolation part: `interpolation.rs` selects a
    /// `{Type}_fmt` display helper from the part's type.
    Display(&'a Ty),
}

fn walk_dispatch_sites_in_expr(expr: &HirExpr, f: &mut impl FnMut(DispatchSite<'_>)) {
    DispatchSites { f }.visit_expr(expr);
}

struct DispatchSites<'f, F> {
    f: &'f mut F,
}

impl<F: FnMut(DispatchSite<'_>)> Visit for DispatchSites<'_, F> {
    fn visit_expr(&mut self, expr: &HirExpr) {
        let f = &mut *self.f;
        if let HirExprKind::FnCall {
            callee_name, args, ..
        } = &expr.kind
        {
            f(DispatchSite::Function(callee_name, args));
        }
        if let HirExprKind::FieldAccess {
            object, field_name, ..
        } = &expr.kind
        {
            f(DispatchSite::Receiver(&object.ty, field_name));
        }
        if let HirExprKind::MethodCall {
            object,
            method,
            method_name,
            args,
            ..
        } = &expr.kind
        {
            f(DispatchSite::Method(*method, args));
            f(DispatchSite::Receiver(&object.ty, method_name));
        }
        if let HirExprKind::BinaryOp { op, left, right } = &expr.kind {
            // `binops.rs` lowers an operator on a builtin head to the head's
            // `.rx` operator method (`Array#==`), so the left operand is that
            // call's receiver.
            if let Some(method) = op.method_name() {
                f(DispatchSite::Receiver(&left.ty, method));
            }
            f(DispatchSite::Operand(&left.ty));
            f(DispatchSite::Operand(&right.ty));
        }
        if let HirExprKind::Interpolation { parts } = &expr.kind {
            for p in parts {
                if let HirInterpolationPart::Expr { expr: e, .. } = p {
                    f(DispatchSite::Display(&e.ty));
                }
            }
        }
        walk_expr(self, expr);
    }
}

/// Walk every `Ty` reachable from the program (function/method bodies,
/// signatures, class and struct fields, const types) and call `f` on each.
/// Deliberately conservative: it visits every type the shared HIR walk
/// reaches, so any `Box[String]` that appears as a value type, a binding
/// type, or a constructor result type is recorded. Type aliases and
/// newtypes only re-name an existing type; the underlying instantiation is
/// recorded at the use site, so their targets are not walked.
fn walk_tys_in_program(program: &HirProgram, f: &mut impl FnMut(&Ty)) {
    let mut tys = Tys { f };
    for item in &program.items {
        tys.visit_item(item);
    }
}

struct Tys<'f, F> {
    f: &'f mut F,
}

impl<F: FnMut(&Ty)> Visit for Tys<'_, F> {
    fn visit_item(&mut self, item: &HirItem) {
        // Item-level types `walk_item` does not visit.
        if let HirItem::Class(HirClassDef { fields, .. })
        | HirItem::Struct(HirStructDef { fields, .. }) = item
        {
            for field in fields {
                (self.f)(&field.ty);
            }
        }
        if let HirItem::Const(c) = item {
            (self.f)(&c.ty);
        }
        walk_item(self, item);
    }

    fn visit_ty(&mut self, ty: &Ty) {
        (self.f)(ty);
    }
}
impl<'a> Lowerer<'a> {
    /// Remove every function that dispatches on a still-generic receiver (an
    /// abstract callee, see [`abstract_callee`]) when no reachable function
    /// calls it, and fail when one does. Such a body is the opaque fallback of
    /// a generic class whose instantiations all got monomorphised copies, so
    /// it is dead; a live one would have no concrete method to call.
    ///
    /// Roots are the entry point and every function except the opaque
    /// generic-class bodies and the abstract-dispatching ones; reachability
    /// follows `Call` callees,
    /// `FuncAddr` targets, closure drop functions and vtable slots.
    pub(super) fn prune_unmonomorphised_bodies(&self, mir: &mut MirProgram) -> Result<(), String> {
        let abstract_call = |f: &MirFunction| -> Option<String> {
            f.blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .find_map(|inst| match inst {
                    MirInst::Call { callee, .. } if callee.starts_with(ABSTRACT_CALLEE_PREFIX) => {
                        Some(callee.clone())
                    }
                    _ => None,
                })
        };
        let poisoned: HashMap<String, String> = mir
            .functions
            .iter()
            .filter_map(|f| abstract_call(f).map(|callee| (f.name.clone(), callee)))
            .collect();
        if poisoned.is_empty() {
            return Ok(());
        }

        let by_name: HashMap<&str, &MirFunction> =
            mir.functions.iter().map(|f| (f.name.as_str(), f)).collect();
        let mut live: HashSet<&str> = HashSet::new();
        // (function, the live function that references it; `None` for a root)
        let mut work: Vec<(&str, Option<&str>)> = mir
            .functions
            .iter()
            .map(|f| f.name.as_str())
            .filter(|n| !poisoned.contains_key(*n) && !self.opaque_generic_bodies.contains(*n))
            .map(|n| (n, None))
            .collect();
        // The entry point is always live: pruning a poisoned `main` dropped
        // the program's entry and surfaced as a link error (`undefined
        // reference to main`) instead of this pass's diagnostic.
        work.extend(mir.entry.as_deref().map(|e| (e, None)));
        work.extend(
            mir.vtables
                .iter()
                .flat_map(|v| v.method_symbols.iter().map(|m| (m.as_str(), None))),
        );
        while let Some((name, caller)) = work.pop() {
            if !live.insert(name) {
                continue;
            }
            let Some(f) = by_name.get(name) else {
                continue;
            };
            if let Some(callee) = poisoned.get(name) {
                return Err(format!(
                    "cannot lower `{name}`{}: it calls `{}` on a value whose type is still a \
                     generic parameter, and no monomorphised copy replaces this call \
                     (generic-calling-generic requires a concrete leaf — see \
                     docs/decisions/q17-cross-package-monomorphization.md).",
                    caller
                        .map(|c| format!(" (called from `{c}`)"))
                        .unwrap_or_default(),
                    callee.trim_start_matches(ABSTRACT_CALLEE_PREFIX)
                ));
            }
            for inst in f.blocks.iter().flat_map(|b| &b.instructions) {
                match inst {
                    MirInst::Call { callee, .. } => work.push((callee, Some(name))),
                    MirInst::FuncAddr { func_name, .. } => work.push((func_name, Some(name))),
                    _ => {}
                }
            }
            work.extend(
                f.closure_drops
                    .iter()
                    .map(|d| (d.drop_fn.as_str(), Some(name))),
            );
        }
        // An opaque generic body nothing live reaches goes too: it may call
        // a poisoned body this pass removes.
        let dead: HashSet<String> = mir
            .functions
            .iter()
            .filter(|f| {
                poisoned.contains_key(&f.name)
                    || (self.opaque_generic_bodies.contains(&f.name)
                        && !live.contains(f.name.as_str()))
            })
            .map(|f| f.name.clone())
            .collect();
        mir.functions.retain(|f| !dead.contains(&f.name));
        Ok(())
    }
}

#[cfg(test)]
mod builtin_head_tests {
    use super::*;

    #[test]
    fn nominal_and_structural_spellings_key_identically() {
        // `self` inside `class Array[T]` types nominally; a call site's
        // receiver types structurally. Both must produce the same mangled
        // base or a self-call inside a specialized body would miss it.
        let structural = Ty::Array(Box::new(Ty::String));
        let nominal = Ty::Class {
            name: "Array".to_string(),
            generic_args: vec![Ty::String],
        };
        let (sc, sa) = Ty::builtin_head(&structural).expect("structural head");
        let (nc, na) = Ty::builtin_head(&nominal).expect("nominal head");
        assert_eq!(mono_base(sc, &sa), mono_base(nc, &na));
    }

    #[test]
    fn type_param_detection_separates_bare_from_nested() {
        let params = vec!["T".to_string()];
        let bare = Ty::TypeParam {
            name: "T".to_string(),
            bounds: vec![],
        };
        let nested = Ty::Array(Box::new(bare.clone()));
        // A method-call RECEIVER only needs specializing when it IS the
        // element; `self.push(x)` (receiver `Array[T]`) does not.
        assert!(is_bare_type_param(&bare, &params));
        assert!(is_bare_type_param(
            &Ty::Ref(Box::new(bare.clone())),
            &params
        ));
        assert!(!is_bare_type_param(&nested, &params));
        // A binary OPERAND dispatches on the whole operand type, so a
        // `T`-carrying container counts.
        assert!(mentions_type_param(&nested, &params));
        assert!(!mentions_type_param(&Ty::Array(Box::new(Ty::Int)), &params));
    }
}
