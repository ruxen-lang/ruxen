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
                _ => {}
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
