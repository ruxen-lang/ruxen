use super::*;
use crate::resolve::symbols::DefKind;

impl<'a> Lowerer<'a> {
    /// Record every (trait → concrete-impl-target) mapping in the program.
    /// Used to dispatch method calls on generic type parameters when the
    /// trait bound has a unique implementor.
    pub(super) fn collect_trait_impls(&mut self, program: &HirProgram) {
        fn visit(
            item: &HirItem,
            map: &mut HashMap<String, Vec<String>>,
            into_map: &mut HashSet<(String, String)>,
        ) {
            match item {
                HirItem::Impl(imp) => {
                    let target = type_name_from_ty(&imp.target_ty);
                    if let Some(ref trait_ref) = imp.trait_ref {
                        map.entry(trait_ref.name.clone())
                            .or_default()
                            .push(target.clone());
                        if trait_ref.name == "Into" {
                            if let Some(arg) = trait_ref.generic_args.first() {
                                let dst = type_name_from_ty(arg);
                                into_map.insert((target.clone(), dst));
                            }
                        }
                    }
                    // ruby-naming.spec.md §3.4a: `extension X` body may
                    // carry `include Mixin` directives. Each include
                    // declares that target type X implements Mixin —
                    // semantically equivalent to the legacy
                    // `impl Mixin for X`.
                    for item in &imp.items {
                        if let HirImplItem::Include {
                            trait_name,
                            negative_trait,
                            ..
                        } = item
                        {
                            if *negative_trait {
                                continue;
                            }
                            map.entry(trait_name.clone())
                                .or_default()
                                .push(target.clone());
                        }
                    }
                }
                HirItem::Class(class) => {
                    for derive_trait in &class.derive_traits {
                        map.entry(derive_trait.clone())
                            .or_default()
                            .push(class.name.clone());
                    }
                    for inner in &class.impl_blocks {
                        if let Some(ref trait_ref) = inner.trait_ref {
                            map.entry(trait_ref.name.clone())
                                .or_default()
                                .push(class.name.clone());
                            if trait_ref.name == "Into" {
                                if let Some(arg) = trait_ref.generic_args.first() {
                                    let dst = type_name_from_ty(arg);
                                    into_map.insert((class.name.clone(), dst));
                                }
                            }
                        }
                    }
                }
                HirItem::Struct(strukt) => {
                    for derive_trait in &strukt.derive_traits {
                        map.entry(derive_trait.clone())
                            .or_default()
                            .push(strukt.name.clone());
                    }
                    // ruby-naming.spec.md §3.4a: in-body `include Mixin`
                    // directives on structs register the same impl edges
                    // an `impl Mixin for Strukt` block would.
                    for inner in &strukt.impl_blocks {
                        if let Some(ref trait_ref) = inner.trait_ref {
                            map.entry(trait_ref.name.clone())
                                .or_default()
                                .push(strukt.name.clone());
                            if trait_ref.name == "Into" {
                                if let Some(arg) = trait_ref.generic_args.first() {
                                    let dst = type_name_from_ty(arg);
                                    into_map.insert((strukt.name.clone(), dst));
                                }
                            }
                        }
                    }
                }
                HirItem::Enum(enm) => {
                    for derive_trait in &enm.derive_traits {
                        map.entry(derive_trait.clone())
                            .or_default()
                            .push(enm.name.clone());
                    }
                    // Same rule as Struct above.
                    for inner in &enm.impl_blocks {
                        if let Some(ref trait_ref) = inner.trait_ref {
                            map.entry(trait_ref.name.clone())
                                .or_default()
                                .push(enm.name.clone());
                            if trait_ref.name == "Into" {
                                if let Some(arg) = trait_ref.generic_args.first() {
                                    let dst = type_name_from_ty(arg);
                                    into_map.insert((enm.name.clone(), dst));
                                }
                            }
                        }
                    }
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        visit(sub, map, into_map);
                    }
                }
                _ => {}
            }
        }
        for item in &program.items {
            visit(item, &mut self.trait_impls, &mut self.into_impls);
        }
    }

    /// Walk the program and record every trait's default method bodies,
    /// keyed by `(trait_name, method_name)`.  Impl blocks that don't
    /// override a default method get a monomorphised copy of the body
    /// emitted as a regular `{TypeName}_{method}` MIR function.
    pub(super) fn collect_trait_default_methods(&mut self, program: &HirProgram) {
        fn visit(item: &HirItem, map: &mut HashMap<String, HashMap<String, HirFuncDef>>) {
            match item {
                HirItem::Mixin(tdef) => {
                    let entry = map.entry(tdef.name.clone()).or_default();
                    for ti in &tdef.items {
                        if let HirMixinItem::DefaultMethod(f) = ti {
                            entry.insert(f.name.clone(), f.clone());
                        }
                    }
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        visit(sub, map);
                    }
                }
                _ => {}
            }
        }
        for item in &program.items {
            visit(item, &mut self.trait_default_methods);
        }
    }

    /// Walk the program and record every top-level `const` definition's
    /// initializer so references can be substituted at use sites.
    pub(super) fn collect_const_values(&mut self, program: &HirProgram) {
        fn visit(item: &HirItem, map: &mut HashMap<DefId, HirExpr>) {
            match item {
                HirItem::Const(c) => {
                    map.insert(c.def_id, c.value.clone());
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        visit(sub, map);
                    }
                }
                _ => {}
            }
        }
        for item in &program.items {
            visit(item, &mut self.const_values);
        }
    }

    /// SINGLE ENTRY POINT for user-Drop class registration. Adding
    /// new sources from which `def drop` can be discovered means
    /// changing only the `class_has_drop_method` closure below — never
    /// fork the walk into a sibling collector. The downstream
    /// drop-elaboration pass in `mir/lower/drops.rs` consults exactly
    /// the resulting `user_drop_classes` set; there is no second
    /// registry.
    ///
    /// Walk the program and record every class that defines its own
    /// `def drop` method. This drives the drop-elaboration pass to
    /// emit a call to `{ClassName}_drop` before the no-op
    /// `MirInst::Drop` cleanup at scope exit.
    ///
    /// A class qualifies as a "drop class" when ANY of these holds:
    ///   1. Its HIR body declares `def drop` (user-side `class X ...
    ///      def drop(self) ... end end`).
    ///   2. An `impl Drop for X` block declares `def drop`.
    ///   3. Its class-body `lib` block declares `def drop as
    ///      "riven_X_drop"` — the stdlib pattern post-Phase-D-6.
    ///
    /// The third check walks the SymbolTable's `ClassInfo.methods`
    /// because lib-decl methods are registered there (as DefIds
    /// appended by `pass1_class_lib_methods`) but do NOT appear in
    /// `HirClassDef.methods` (which only holds user-body `def`s).
    ///
    /// Phase D-6 of #06.95 made this fully GENERIC — no hardcoded
    /// class-name list. Adding `def drop as "riven_..."` to a class
    /// in its .rvn lib block is the SINGLE mechanism that opts it
    /// into user_drop_classes. This also fixes the historical
    /// double-free pattern observed in #06.95 Phase E.B.3 first
    /// attempt: the prior code path inserted hardcoded names AND
    /// detected lib-decl methods, so a class with both registrations
    /// got two drop emissions at scope exit.
    ///
    /// Relationship to `mixin Drop` (from
    /// `library/std/core/src/lib.rvn`): `include Drop` is currently a
    /// no-op marker — the actual registration trigger is `def drop`.
    /// Spec
    /// `docs/specs/system/compiler_consolidation.spec.md` §B4 proposes
    /// to flip this so `include Drop` becomes the required contract
    /// (with `def drop` as the implementation). That is a behavioural
    /// change requiring touching ~16 stdlib files to add `include
    /// Drop` plus a new error code; deferred from this consolidation
    /// commit because the spec stop condition warns against silently
    /// flipping semantics — see the B4 report at commit time.
    pub(super) fn collect_user_drop_classes(&mut self, program: &HirProgram) {
        let symbols = self.symbols;
        let class_has_drop_method = |class: &HirClassDef| -> bool {
            if class.methods.iter().any(|m| m.name == "drop") {
                return true;
            }
            for impl_block in &class.impl_blocks {
                for item in &impl_block.items {
                    if let HirImplItem::Method(m) = item {
                        if m.name == "drop" {
                            return true;
                        }
                    }
                }
            }
            if let Some(def) = symbols.get(class.def_id) {
                if let DefKind::Class { info } = &def.kind {
                    for method_id in &info.methods {
                        if let Some(m_def) = symbols.get(*method_id) {
                            if m_def.name == "drop" {
                                return true;
                            }
                        }
                    }
                }
            }
            // Lib-decl methods (e.g. `lib "..." def drop as
            // "riven_mutex_guard_drop"(self) end`) are registered as
            // `DefKind::Method { parent: class.def_id, .. }` in the
            // symbol table but are NOT necessarily mirrored into
            // `info.methods` (the resolver's body-method list).
            // Without this scan, MutexGuard / Sender / Receiver /
            // every other class whose drop is declared via a lib
            // block silently skipped the user-drop pathway and the
            // pthread mutex (or refcount, or fd) leaked or
            // deadlocked at scope exit. Sweep the symbol table for
            // any `DefKind::Method` whose parent matches and whose
            // name is "drop".
            for def in symbols.iter() {
                if let DefKind::Method { parent, .. } = &def.kind {
                    if *parent == class.def_id && def.name == "drop" {
                        return true;
                    }
                }
            }
            false
        };

        fn visit(
            item: &HirItem,
            set: &mut HashSet<String>,
            module_path: &[String],
            class_has_drop_method: &dyn Fn(&HirClassDef) -> bool,
        ) {
            match item {
                HirItem::Class(class) => {
                    if class_has_drop_method(class) {
                        let qualified = if module_path.is_empty() {
                            class.name.clone()
                        } else {
                            format!("{}.{}", module_path.join("."), class.name)
                        };
                        set.insert(qualified);
                    }
                }
                HirItem::Impl(impl_block) => {
                    let target = type_name_from_ty(&impl_block.target_ty);
                    if !target.is_empty() {
                        for inner in &impl_block.items {
                            if let HirImplItem::Method(m) = inner {
                                if m.name == "drop" {
                                    set.insert(target.clone());
                                    break;
                                }
                            }
                        }
                    }
                }
                HirItem::Module(m) => {
                    let mut child_path: Vec<String> = module_path.to_vec();
                    child_path.push(m.name.clone());
                    for sub in &m.items {
                        visit(sub, set, &child_path, class_has_drop_method);
                    }
                }
                _ => {}
            }
        }

        for item in &program.items {
            visit(
                item,
                &mut self.user_drop_classes,
                &[],
                &class_has_drop_method,
            );
        }
    }
}
