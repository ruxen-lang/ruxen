use super::*;
use crate::resolve::symbols::DefKind;

impl<'a> Lowerer<'a> {
    /// Record every (trait → concrete-impl-target) mapping in the program.
    /// Used to dispatch method calls on generic type parameters when the
    /// trait bound has a unique implementor. `impl Into[Dst] for Src`
    /// blocks also record the `(Src, Dst)` pair in `into_impls`.
    pub(super) fn collect_trait_impls(&mut self, program: &HirProgram) {
        for (imp, target) in impl_blocks_with_target(&program.items) {
            if let Some(ref trait_ref) = imp.trait_ref {
                record_impl(&mut self.trait_impls, &trait_ref.name, &target);
                if trait_ref.name == "Into" {
                    if let Some(arg) = trait_ref.generic_args.first() {
                        self.into_impls
                            .insert((target.clone(), type_name_from_ty(arg)));
                    }
                }
            }
            // ruby-naming.spec.md §3.4a: an `include Mixin` directive in a
            // body declares that the target implements Mixin, exactly as
            // `impl Mixin for X` does.
            for item in &imp.items {
                if let HirImplItem::Include {
                    trait_name,
                    negative_trait: false,
                    ..
                } = item
                {
                    record_impl(&mut self.trait_impls, trait_name, &target);
                }
            }
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
    ///      "ruxen_X_drop"` — the stdlib pattern post-Phase-D-6.
    ///
    /// The third check walks the SymbolTable's `ClassInfo.methods`
    /// because lib-decl methods are registered there (as DefIds
    /// appended by `pass1_class_lib_methods`) but do NOT appear in
    /// `HirClassDef.methods` (which only holds user-body `def`s).
    ///
    /// Phase D-6 of #06.95 made this fully GENERIC — no hardcoded
    /// class-name list. Adding `def drop as "ruxen_..."` to a class
    /// in its .rx lib block is the SINGLE mechanism that opts it
    /// into user_drop_classes. This also fixes the historical
    /// double-free pattern observed in #06.95 Phase E.B.3 first
    /// attempt: the prior code path inserted hardcoded names AND
    /// detected lib-decl methods, so a class with both registrations
    /// got two drop emissions at scope exit.
    ///
    /// Relationship to `mixin Drop` (from
    /// `library/std/core/src/lib.rx`): `include Drop` is currently a
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
            // "ruxen_mutex_guard_drop"(self) end`) are registered as
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
            symbols
                .children_of(class.def_id)
                .any(|def| def.name == "drop" && matches!(def.kind, DefKind::Method { .. }))
        };

        fn visit(
            item: &HirItem,
            set: &mut HashSet<DefId>,
            symbols: &SymbolTable,
            class_has_drop_method: &dyn Fn(&HirClassDef) -> bool,
        ) {
            match item {
                HirItem::Class(class) => {
                    if class_has_drop_method(class) {
                        set.insert(class.def_id);
                    }
                }
                HirItem::Impl(impl_block) => {
                    let has_drop = impl_block
                        .items
                        .iter()
                        .any(|inner| matches!(inner, HirImplItem::Method(m) if m.name == "drop"));
                    let target = type_name_from_ty(&impl_block.target_ty);
                    if let Some(def) = has_drop.then(|| symbols.type_def(&target)).flatten() {
                        set.insert(def.id);
                    }
                }
                HirItem::Module(m) => {
                    for sub in &m.items {
                        visit(sub, set, symbols, class_has_drop_method);
                    }
                }
                _ => {}
            }
        }

        for item in &program.items {
            visit(
                item,
                &mut self.user_drop_classes,
                symbols,
                &class_has_drop_method,
            );
        }
    }
}

/// Record that `target` implements `trait_name`, once.
fn record_impl(map: &mut HashMap<String, Vec<String>>, trait_name: &str, target: &str) {
    let targets = map.entry(trait_name.to_string()).or_default();
    if !targets.iter().any(|t| t == target) {
        targets.push(target.to_string());
    }
}

/// Every impl block in `items` (recursing into modules), paired with the
/// name of the type it adds to: a top-level `impl`/`extension` names its
/// target type, and a class, struct or enum body's blocks name their owner.
fn impl_blocks_with_target(items: &[HirItem]) -> Vec<(&HirImplBlock, String)> {
    let mut out = Vec::new();
    for item in items {
        match item {
            HirItem::Impl(imp) => out.push((imp, type_name_from_ty(&imp.target_ty))),
            HirItem::Class(class) => out.extend(
                class
                    .impl_blocks
                    .iter()
                    .map(|imp| (imp, class.name.clone())),
            ),
            HirItem::Struct(strukt) => out.extend(
                strukt
                    .impl_blocks
                    .iter()
                    .map(|imp| (imp, strukt.name.clone())),
            ),
            HirItem::Enum(enm) => {
                out.extend(enm.impl_blocks.iter().map(|imp| (imp, enm.name.clone())))
            }
            HirItem::Module(m) => out.extend(impl_blocks_with_target(&m.items)),
            _ => {}
        }
    }
    out
}
