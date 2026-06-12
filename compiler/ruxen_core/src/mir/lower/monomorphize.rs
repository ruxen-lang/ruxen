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
//!      generalized `subst_type_params_in_ty` recurses through `Ref*`.
//!   3. Only fully-concrete instantiations are recorded (no nested
//!      `TypeParam`/`Infer`), so we never emit a half-substituted body.
//!
//! ── Fallback ──
//! The opaque (un-suffixed) generic body is still emitted by the normal
//! `lower_item` path, so any call site whose receiver instantiation we did
//! NOT monomorphize (e.g. a generic arg that stayed a type param) keeps
//! resolving to it.

use super::*;
use std::collections::{BTreeSet, HashMap, HashSet};

/// Marker separating the base class name from its concrete generic args in
/// a monomorphized mangled name. Chosen so it cannot collide with a real
/// source identifier path and is C-symbol-safe (alphanumerics + `_`).
/// Example: `Box[String]` method `eq` → `Box__mono__String_eq`.
const MONO_SEP: &str = "__mono__";

/// A recorded instantiation key: the concrete generic-arg vector for one
/// use of a generic class.
pub(super) type MonoKey = Vec<Ty>;

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
    match ty {
        Ty::TypeParam { .. } | Ty::Infer(_) => false,
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner)
        | Ty::FixedArray(inner, _)
        | Ty::Option(inner)
        | Ty::Array(inner)
        | Ty::Set(inner) => is_fully_concrete(inner),
        Ty::Map(k, v) => is_fully_concrete(k) && is_fully_concrete(v),
        Ty::Result(a, b) => is_fully_concrete(a) && is_fully_concrete(b),
        Ty::Tuple(elems) => elems.iter().all(is_fully_concrete),
        Ty::Class { generic_args, .. }
        | Ty::Struct { generic_args, .. }
        | Ty::Enum { generic_args, .. } => generic_args.iter().all(is_fully_concrete),
        _ => true,
    }
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
                // `self.mono_emitted` was already populated for `base` in
                // `collect_mono_instances` (it must exist before
                // `lower_items` so call sites can redirect); nothing to do
                // here.
            }
        }
        Ok(())
    }

    /// Given a method-call/constructor receiver type, return the
    /// monomorphic mangled *base* (`Box__mono__String`) when this exact
    /// instantiation was emitted; `None` otherwise (→ keep the opaque
    /// `{Class}_{method}` callee). Peels refs.
    pub(super) fn mono_base_for_ty(&self, ty: &Ty) -> Option<String> {
        let mut cur = ty;
        loop {
            match cur {
                Ty::Ref(inner)
                | Ty::RefMut(inner)
                | Ty::RefLifetime(_, inner)
                | Ty::RefMutLifetime(_, inner) => cur = inner,
                Ty::Class { name, generic_args } => {
                    if generic_args.is_empty() {
                        return None;
                    }
                    let base = mono_base(name, generic_args);
                    if self.mono_emitted.contains(&base) {
                        return Some(base);
                    }
                    return None;
                }
                _ => return None,
            }
        }
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
        // Set of (fn name) for which we saw at least one call we could NOT
        // monomorphize (a generic-through-generic shape with a non-concrete
        // leaf in the SOURCE program). Its opaque body must be kept so that
        // call resolves (and devirtualizes via `unique_bound_impl`, or errors).
        // A call inside a SPECIALIZED body does not taint the opaque body — it
        // is resolved by the specialized copy — so the recursive scans below
        // do NOT contribute to `needs_opaque`.
        let mut needs_opaque: HashSet<String> = HashSet::new();

        // Recover the concrete instantiation for one call, if fully concrete.
        // `taint` is set only for source-program scans (not specialized-body
        // scans): an unresolvable arg in the source means the opaque body is
        // referenced and must stay.
        let recover = |callee_name: &str,
                       arg_tys: &[Ty],
                       needs_opaque: &mut HashSet<String>,
                       taint: bool|
         -> Option<(String, MonoKey)> {
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
                if taint {
                    needs_opaque.insert(callee_name.to_string());
                }
                return None;
            }
            let arg_vec: MonoKey = param_names
                .iter()
                .map(|n| subst.get(n).cloned().unwrap())
                .collect();
            Some((callee_name.to_string(), arg_vec))
        };

        // Seed the worklist from the source program's call sites. Only a call
        // from a NON-GENERIC context (`taint = true`) can force a callee's
        // opaque body to stay: a generic fn's own opaque body is suppressed
        // when it is fully monomorphized, so an opaque call living INSIDE
        // another generic fn's body is unreachable (its enclosing opaque body
        // is gone) and must not pin the callee's opaque body. This is what
        // lets `render_twice[T] { render(d) }` suppress BOTH opaque bodies
        // once every concrete call is monomorphized (Q17 generic-calling-
        // generic). The fixpoint below re-discovers `render`'s real
        // instantiations through the specialized `render_twice` copies.
        let mut worklist: Vec<(String, MonoKey)> = Vec::new();
        let seed = |callee_name: &str,
                    args: &[HirExpr],
                    enclosing_is_generic: bool,
                    needs_opaque: &mut HashSet<String>,
                    seen: &mut HashMap<String, BTreeSet<String>>,
                    instances: &mut HashMap<String, Vec<MonoKey>>,
                    worklist: &mut Vec<(String, MonoKey)>| {
            let arg_tys: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
            if let Some(inst) = recover(callee_name, &arg_tys, needs_opaque, !enclosing_is_generic)
            {
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
        for_each_fn_body(program, &mut |body, enclosing_is_generic| {
            walk_calls_in_expr(body, &mut |callee_name, args| {
                seed(
                    callee_name,
                    args,
                    enclosing_is_generic,
                    &mut needs_opaque,
                    &mut seen,
                    &mut instances,
                    &mut worklist,
                );
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
            // Scan the substituted body's calls. `taint = false`: a call here
            // is resolved by THIS specialized copy, so an unresolvable arg
            // (should not happen post-substitution) does not force the
            // callee's opaque body to be referenced.
            let mut discovered: Vec<(String, MonoKey)> = Vec::new();
            walk_calls_in_expr(&cloned.body, &mut |callee_name, call_args| {
                let arg_tys: Vec<Ty> = call_args.iter().map(|a| a.ty.clone()).collect();
                if let Some(inst) = recover(callee_name, &arg_tys, &mut needs_opaque, false) {
                    discovered.push(inst);
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
        // `needs_opaque` therefore feeds the call-site diagnostic, not opaque
        // emission. `keeps_opaque` stays empty: `generic_fn_keeps_opaque`
        // returns false for every eligible generic fn, suppressing all of them.
        let _ = (&needs_opaque, &emitted);

        self.mono_fns = fns;
        self.fn_mono_instances = instances;
        self.fn_mono_needs_opaque = needs_opaque;
        self.fn_mono_emitted = emitted;
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

    /// True when a CONCRETE-context (non-generic-caller) call to `fn_name`
    /// could not be monomorphized — its type args did not resolve to a
    /// fully-concrete vector. Such a call cannot fall back to the opaque body
    /// (it is suppressed and would link-fail), so `fn_call.rs` surfaces a clear
    /// diagnostic instead. Recorded during `collect_generic_fn_instances`.
    pub(super) fn generic_fn_unmonomorphizable(&self, fn_name: &str) -> bool {
        self.fn_mono_needs_opaque.contains(fn_name)
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
    fn peel(ty: &Ty) -> &Ty {
        match ty {
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => peel(inner),
            other => other,
        }
    }
    let decl = peel(decl);
    let actual = peel(actual);

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

/// Invoke `f(body, enclosing_is_generic)` for every function / method body in
/// the program, reporting whether the enclosing function declares any generic
/// params. Used by Q17 collection so a call inside a generic body is treated
/// differently from one in a concrete (non-generic) body for opaque-body
/// retention.
fn for_each_fn_body(program: &HirProgram, f: &mut impl FnMut(&HirExpr, bool)) {
    fn visit(item: &HirItem, f: &mut impl FnMut(&HirExpr, bool)) {
        match item {
            HirItem::Function(func) => f(&func.body, !func.generic_params.is_empty()),
            HirItem::Class(c) => {
                for m in &c.methods {
                    f(&m.body, !m.generic_params.is_empty());
                }
                for b in &c.impl_blocks {
                    for it in &b.items {
                        if let HirImplItem::Method(m) = it {
                            f(&m.body, !m.generic_params.is_empty());
                        }
                    }
                }
            }
            HirItem::Struct(s) => {
                for m in &s.methods {
                    f(&m.body, !m.generic_params.is_empty());
                }
            }
            HirItem::Enum(e) => {
                for m in &e.methods {
                    f(&m.body, !m.generic_params.is_empty());
                }
            }
            HirItem::Impl(b) => {
                for it in &b.items {
                    if let HirImplItem::Method(m) = it {
                        f(&m.body, !m.generic_params.is_empty());
                    }
                }
            }
            HirItem::Module(m) => {
                for sub in &m.items {
                    visit(sub, f);
                }
            }
            // A const initializer / mixin default body is a concrete context
            // (no enclosing generic fn params).
            HirItem::Const(c) => f(&c.value, false),
            HirItem::Mixin(m) => {
                for it in &m.items {
                    if let HirMixinItem::DefaultMethod(func) = it {
                        f(&func.body, !func.generic_params.is_empty());
                    }
                }
            }
            HirItem::TypeAlias(_) | HirItem::Newtype(_) => {}
        }
    }
    for it in &program.items {
        visit(it, f);
    }
}

fn walk_calls_in_expr(expr: &HirExpr, f: &mut impl FnMut(&str, &[HirExpr])) {
    match &expr.kind {
        HirExprKind::FnCall {
            callee_name, args, ..
        } => {
            f(callee_name, args);
            for a in args {
                walk_calls_in_expr(a, f);
            }
        }
        HirExprKind::FieldAccess { object, .. } => walk_calls_in_expr(object, f),
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            walk_calls_in_expr(object, f);
            for a in args {
                walk_calls_in_expr(a, f);
            }
            if let Some(b) = block {
                walk_calls_in_expr(b, f);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            walk_calls_in_expr(left, f);
            walk_calls_in_expr(right, f);
        }
        HirExprKind::UnaryOp { operand, .. } => walk_calls_in_expr(operand, f),
        HirExprKind::Borrow { expr: inner, .. } => walk_calls_in_expr(inner, f),
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            for s in stmts {
                walk_calls_in_stmt(s, f);
            }
            if let Some(t) = tail {
                walk_calls_in_expr(t, f);
            }
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_calls_in_expr(cond, f);
            walk_calls_in_expr(then_branch, f);
            if let Some(e) = else_branch {
                walk_calls_in_expr(e, f);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            walk_calls_in_expr(scrutinee, f);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_calls_in_expr(g, f);
                }
                walk_calls_in_expr(&arm.body, f);
            }
        }
        HirExprKind::Loop { body } => walk_calls_in_expr(body, f),
        HirExprKind::While { condition, body } => {
            walk_calls_in_expr(condition, f);
            walk_calls_in_expr(body, f);
        }
        HirExprKind::For { iterable, body, .. } => {
            walk_calls_in_expr(iterable, f);
            walk_calls_in_expr(body, f);
        }
        HirExprKind::Assign { target, value, .. }
        | HirExprKind::CompoundAssign { target, value, .. } => {
            walk_calls_in_expr(target, f);
            walk_calls_in_expr(value, f);
        }
        HirExprKind::Return(e) | HirExprKind::Break(e) => {
            if let Some(inner) = e {
                walk_calls_in_expr(inner, f);
            }
        }
        HirExprKind::Closure { body, .. } => walk_calls_in_expr(body, f),
        HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
            for (_, e) in fields {
                walk_calls_in_expr(e, f);
            }
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            for e in elems {
                walk_calls_in_expr(e, f);
            }
        }
        HirExprKind::Index { object, index } => {
            walk_calls_in_expr(object, f);
            walk_calls_in_expr(index, f);
        }
        HirExprKind::Cast { expr: inner, .. } => walk_calls_in_expr(inner, f),
        HirExprKind::ArrayFill { value, .. } => walk_calls_in_expr(value, f),
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                walk_calls_in_expr(s, f);
            }
            if let Some(e) = end {
                walk_calls_in_expr(e, f);
            }
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                if let HirInterpolationPart::Expr { expr: e, .. } = p {
                    walk_calls_in_expr(e, f);
                }
            }
        }
        HirExprKind::MacroCall { args, .. } => {
            for a in args {
                walk_calls_in_expr(a, f);
            }
        }
        HirExprKind::MapLiteral(pairs) => {
            for (k, v) in pairs {
                walk_calls_in_expr(k, f);
                walk_calls_in_expr(v, f);
            }
        }
        HirExprKind::IntLiteral(_)
        | HirExprKind::FloatLiteral(_)
        | HirExprKind::StringLiteral(_)
        | HirExprKind::BoolLiteral(_)
        | HirExprKind::CharLiteral(_)
        | HirExprKind::UnitLiteral
        | HirExprKind::RegexLiteral { .. }
        | HirExprKind::VarRef(_)
        | HirExprKind::Continue
        | HirExprKind::NullLiteral
        | HirExprKind::Error => {}
    }
}

fn walk_calls_in_stmt(stmt: &HirStatement, f: &mut impl FnMut(&str, &[HirExpr])) {
    match stmt {
        HirStatement::Let { value, .. } => {
            if let Some(v) = value {
                walk_calls_in_expr(v, f);
            }
        }
        HirStatement::Expr(e) => walk_calls_in_expr(e, f),
    }
}

/// Walk every `Ty` reachable from the program (function/method bodies,
/// signatures, fields) and call `f` on each. Deliberately conservative:
/// it visits every expression's `.ty` and every let/param/return type, so
/// any `Box[String]` that appears as a value type, a binding type, or a
/// constructor result type is recorded.
fn walk_tys_in_program(program: &HirProgram, f: &mut impl FnMut(&Ty)) {
    for item in &program.items {
        walk_tys_in_item(item, f);
    }
}

fn walk_tys_in_item(item: &HirItem, f: &mut impl FnMut(&Ty)) {
    match item {
        HirItem::Function(func) => walk_tys_in_func(func, f),
        HirItem::Class(c) => {
            for field in &c.fields {
                f(&field.ty);
            }
            for m in &c.methods {
                walk_tys_in_func(m, f);
            }
            for b in &c.impl_blocks {
                for it in &b.items {
                    if let HirImplItem::Method(m) = it {
                        walk_tys_in_func(m, f);
                    }
                }
            }
        }
        HirItem::Struct(s) => {
            for field in &s.fields {
                f(&field.ty);
            }
            for m in &s.methods {
                walk_tys_in_func(m, f);
            }
        }
        HirItem::Enum(e) => {
            for m in &e.methods {
                walk_tys_in_func(m, f);
            }
        }
        HirItem::Impl(b) => {
            for it in &b.items {
                if let HirImplItem::Method(m) = it {
                    walk_tys_in_func(m, f);
                }
            }
        }
        HirItem::Module(m) => {
            for sub in &m.items {
                walk_tys_in_item(sub, f);
            }
        }
        // Const initializers and mixin default-method bodies CAN host
        // generic-class instantiations; walk them so a class used only there
        // is still monomorphized (same hole class as the map-literal value).
        HirItem::Const(c) => {
            f(&c.ty);
            walk_tys_in_expr(&c.value, f);
        }
        HirItem::Mixin(m) => {
            for it in &m.items {
                if let HirMixinItem::DefaultMethod(func) = it {
                    walk_tys_in_func(func, f);
                }
            }
        }
        // Type aliases / newtypes only re-name an existing type; the
        // underlying instantiation is recorded at the use site, and the alias
        // target itself is not a fresh generic-class use to monomorphize.
        // Enumerated explicitly (no `_ =>`) so a new `HirItem` variant that
        // carries methods/exprs becomes a compile error here.
        HirItem::TypeAlias(_) | HirItem::Newtype(_) => {}
    }
}

fn walk_tys_in_func(func: &HirFuncDef, f: &mut impl FnMut(&Ty)) {
    for p in &func.params {
        f(&p.ty);
    }
    f(&func.return_ty);
    walk_tys_in_expr(&func.body, f);
}

fn walk_tys_in_expr(expr: &HirExpr, f: &mut impl FnMut(&Ty)) {
    f(&expr.ty);
    match &expr.kind {
        HirExprKind::FieldAccess { object, .. } => walk_tys_in_expr(object, f),
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            walk_tys_in_expr(object, f);
            for a in args {
                walk_tys_in_expr(a, f);
            }
            if let Some(b) = block {
                walk_tys_in_expr(b, f);
            }
        }
        HirExprKind::FnCall { args, .. } => {
            for a in args {
                walk_tys_in_expr(a, f);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            walk_tys_in_expr(left, f);
            walk_tys_in_expr(right, f);
        }
        HirExprKind::UnaryOp { operand, .. } => walk_tys_in_expr(operand, f),
        HirExprKind::Borrow { expr: inner, .. } => walk_tys_in_expr(inner, f),
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            for s in stmts {
                walk_tys_in_stmt(s, f);
            }
            if let Some(t) = tail {
                walk_tys_in_expr(t, f);
            }
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            walk_tys_in_expr(cond, f);
            walk_tys_in_expr(then_branch, f);
            if let Some(e) = else_branch {
                walk_tys_in_expr(e, f);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            walk_tys_in_expr(scrutinee, f);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    walk_tys_in_expr(g, f);
                }
                walk_tys_in_expr(&arm.body, f);
            }
        }
        HirExprKind::Loop { body } => walk_tys_in_expr(body, f),
        HirExprKind::While { condition, body } => {
            walk_tys_in_expr(condition, f);
            walk_tys_in_expr(body, f);
        }
        HirExprKind::For { iterable, body, .. } => {
            walk_tys_in_expr(iterable, f);
            walk_tys_in_expr(body, f);
        }
        HirExprKind::Assign { target, value, .. } => {
            walk_tys_in_expr(target, f);
            walk_tys_in_expr(value, f);
        }
        HirExprKind::CompoundAssign { target, value, .. } => {
            walk_tys_in_expr(target, f);
            walk_tys_in_expr(value, f);
        }
        HirExprKind::Return(e) | HirExprKind::Break(e) => {
            if let Some(inner) = e {
                walk_tys_in_expr(inner, f);
            }
        }
        HirExprKind::Closure { body, .. } => walk_tys_in_expr(body, f),
        HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
            for (_, e) in fields {
                walk_tys_in_expr(e, f);
            }
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            for e in elems {
                walk_tys_in_expr(e, f);
            }
        }
        HirExprKind::Index { object, index } => {
            walk_tys_in_expr(object, f);
            walk_tys_in_expr(index, f);
        }
        HirExprKind::Cast {
            expr: inner,
            target,
        } => {
            walk_tys_in_expr(inner, f);
            f(target);
        }
        HirExprKind::ArrayFill { value, .. } => walk_tys_in_expr(value, f),
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                walk_tys_in_expr(s, f);
            }
            if let Some(e) = end {
                walk_tys_in_expr(e, f);
            }
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                if let HirInterpolationPart::Expr { expr: e, .. } = p {
                    walk_tys_in_expr(e, f);
                }
            }
        }
        HirExprKind::MacroCall { args, .. } => {
            for a in args {
                walk_tys_in_expr(a, f);
            }
        }
        // Map literals carry key/value sub-expressions whose types (and the
        // generic-class instantiations inside them) must be walked. The
        // previous `_ => {}` silently dropped these, so a generic class used
        // ONLY as a map-literal value (e.g. `{ 1 => Box.new(5) }`) was never
        // monomorphized — a latent link-failure hole the exhaustive match
        // now closes.
        HirExprKind::MapLiteral(pairs) => {
            for (k, v) in pairs {
                walk_tys_in_expr(k, f);
                walk_tys_in_expr(v, f);
            }
        }
        // Leaves with no nested `HirExpr` / `Ty` to walk. Enumerated
        // explicitly (no `_ =>`) so a new `HirExprKind` variant that DOES
        // carry a reachable type becomes a compile error here instead of a
        // silently-skipped monomorphization target.
        HirExprKind::IntLiteral(_)
        | HirExprKind::FloatLiteral(_)
        | HirExprKind::StringLiteral(_)
        | HirExprKind::BoolLiteral(_)
        | HirExprKind::CharLiteral(_)
        | HirExprKind::UnitLiteral
        | HirExprKind::RegexLiteral { .. }
        | HirExprKind::VarRef(_)
        | HirExprKind::Continue
        | HirExprKind::NullLiteral
        | HirExprKind::Error => {}
    }
}

fn walk_tys_in_stmt(stmt: &HirStatement, f: &mut impl FnMut(&Ty)) {
    match stmt {
        HirStatement::Let { ty, value, .. } => {
            f(ty);
            if let Some(v) = value {
                walk_tys_in_expr(v, f);
            }
        }
        HirStatement::Expr(e) => walk_tys_in_expr(e, f),
    }
}
