use super::*;

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

    /// Walk the program and record every class that defines its own
    /// `def drop` method (typically inside an `impl Drop` block, but we
    /// also accept a top-level `def drop` on the class). This drives the
    /// drop-elaboration pass to emit a call to `{ClassName}_drop` before
    /// the no-op `MirInst::Drop` cleanup at scope exit.
    pub(super) fn collect_user_drop_classes(&mut self, program: &HirProgram) {
        fn class_has_drop_method(class: &HirClassDef) -> bool {
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
            false
        }

        fn visit(item: &HirItem, set: &mut HashSet<String>) {
            match item {
                HirItem::Class(class) => {
                    if class_has_drop_method(class) {
                        set.insert(class.name.clone());
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
                    for sub in &m.items {
                        visit(sub, set);
                    }
                }
                _ => {}
            }
        }

        for item in &program.items {
            visit(item, &mut self.user_drop_classes);
        }

        // Phase 2 stdlib (#06): built-in classes whose runtime side
        // owns inner heap (Vec spines, env entries, captured stdout/
        // stderr buffers) need a `Command_drop` / `Output_drop` call
        // before the generic `riven_dealloc` so the inner allocations
        // are released. Pre-populate the set so the drop-elaboration
        // pass treats them like user-defined drop classes — see
        // `insert_drops`. The runtime fns are mapped via the standard
        // `{Type}_drop -> riven_<type>_drop` dispatch in
        // `codegen::runtime::runtime_name`.
        self.user_drop_classes.insert("Command".to_string());
        self.user_drop_classes.insert("Output".to_string());
    }
}
