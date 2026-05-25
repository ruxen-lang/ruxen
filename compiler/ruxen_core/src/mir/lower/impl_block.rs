use super::*;

impl<'a> Lowerer<'a> {
    // ── Impl block helper ───────────────────────────────────────────────

    pub(super) fn lower_impl_block(
        &mut self,
        impl_block: &HirImplBlock,
        type_name: &str,
        mir: &mut MirProgram,
    ) -> Result<(), String> {
        self.lower_impl_block_with_outer_methods(impl_block, type_name, mir, &HashSet::new())
    }

    pub(super) fn lower_impl_block_with_outer_methods(
        &mut self,
        impl_block: &HirImplBlock,
        type_name: &str,
        mir: &mut MirProgram,
        outer_methods: &HashSet<String>,
    ) -> Result<(), String> {
        // Track which method names the impl defines explicitly so we can
        // decide which trait defaults to monomorphise. `outer_methods`
        // covers the enclosing class/extension body's own `def`s —
        // critical for ruby-naming.spec.md §3.4a where `include Mixin`
        // sits beside the methods that satisfy or override it.
        let mut defined_methods: HashSet<String> = outer_methods.clone();
        for item in &impl_block.items {
            match item {
                HirImplItem::Method(method) => {
                    defined_methods.insert(method.name.clone());
                    let mangled = format!("{}_{}", type_name, method.name);
                    let mir_fn = self.lower_method(&mangled, method)?;
                    mir.functions.push(mir_fn);
                }
                HirImplItem::AssocType { .. } => {}
                HirImplItem::Include { .. } => {
                    // Include directives are handled at `collect_trait_impls`
                    // time — no MIR-side action here. The trait's default
                    // methods are monomorphised below via the regular
                    // trait_default_methods loop, using `defined_methods`
                    // (which already includes `outer_methods` if the user
                    // defined the method beside the include).
                }
            }
        }

        // For `impl Trait for Type`, emit a monomorphised copy of every
        // default method the impl did not override. The default body is
        // cloned and its `Self` type occurrences are rewritten to the
        // concrete impl target so `self.field` / `self.method` dispatch
        // resolves through the normal class path.
        if let Some(ref trait_ref) = impl_block.trait_ref {
            if let Some(defaults) = self.trait_default_methods.get(&trait_ref.name).cloned() {
                let concrete_self = impl_block.target_ty.clone();
                for (mname, default_fn) in defaults {
                    if defined_methods.contains(&mname) {
                        continue;
                    }
                    let mut cloned = default_fn.clone();
                    rewrite_self_in_func(&mut cloned, &concrete_self);
                    let mangled = format!("{}_{}", type_name, cloned.name);
                    let mir_fn = self.lower_method(&mangled, &cloned)?;
                    mir.functions.push(mir_fn);
                }
            }
        }
        Ok(())
    }
}
