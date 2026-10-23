//! Call-site redirects to the instances the engine emitted, and the pruning
//! of opaque template bodies nothing live reaches (see `instances/mod.rs`).

use super::*;

impl<'a> Lowerer<'a> {
    // ── Call-site redirects ──────────────────────────────────────────────

    /// The instance base of a class instance type (`Box__mono__String`),
    /// when the engine emitted it.
    pub(in crate::mir::lower) fn class_instance_base(&self, ty: &Ty) -> Option<String> {
        match ty.peel_refs() {
            Ty::Class { name, generic_args } if !generic_args.is_empty() => {
                let base = mono_base(name, generic_args);
                self.instances.emitted.contains(&base).then_some(base)
            }
            _ => None,
        }
    }

    /// The shell-method instance a receiver-bearing call reaches
    /// (`Array__mono__String_join`), when the engine emitted it.
    pub(in crate::mir::lower) fn shell_instance_callee(
        &self,
        receiver_ty: &Ty,
        method_name: &str,
    ) -> Option<String> {
        let (class, args) = self.shell_head(receiver_ty)?;
        let symbol = format!("{}_{}", mono_base(&class, &args), method_name);
        self.instances.emitted.contains(&symbol).then_some(symbol)
    }

    /// Whether `method` is a method template: a call to it reaches an
    /// instance or nothing.
    pub(in crate::mir::lower) fn is_template_method(&self, method: DefId) -> bool {
        self.instances.methods.contains_key(&method)
    }

    /// The method instance a call to `method` on `receiver_ty` with
    /// `arg_tys` (its block last) reaches, when the engine emitted it.
    pub(in crate::mir::lower) fn method_instance_callee(
        &self,
        method: DefId,
        receiver_ty: &Ty,
        arg_tys: &[Ty],
    ) -> Option<String> {
        let j = *self.instances.methods.get(&method)?;
        let args = self.method_instance_args(j, receiver_ty, arg_tys)?;
        let symbol = self.instance_symbol(j, &args);
        self.instances.emitted.contains(&symbol).then_some(symbol)
    }

    /// Whether `name` is a function template.
    pub(in crate::mir::lower) fn is_template_fn(&self, name: &str) -> bool {
        self.instances.functions.contains_key(name)
    }

    /// The function instance a call to `name` with `arg_tys` reaches, when
    /// the engine emitted it.
    pub(in crate::mir::lower) fn fn_instance_callee(
        &self,
        name: &str,
        arg_tys: &[Ty],
    ) -> Option<String> {
        let j = *self.instances.functions.get(name)?;
        let args = self.fn_instance_args(j, arg_tys)?;
        let symbol = self.instance_symbol(j, &args);
        self.instances.emitted.contains(&symbol).then_some(symbol)
    }

    /// Whether `name` is a class template (its opaque bodies are all
    /// recorded in `opaque_bodies`).
    pub(in crate::mir::lower) fn is_template_class(&self, name: &str) -> bool {
        self.instances.classes.contains_key(name)
    }

    /// Whether `def` is the body of a shell-method or method template.
    pub(in crate::mir::lower) fn is_template_body(&self, def: DefId) -> bool {
        self.instances.template_defs.contains(&def)
    }

    /// Remove every function that calls an abstract callee (see
    /// [`abstract_callee`]) when no reachable function calls it, and fail
    /// when one does. Such a body is a template's opaque body whose callers
    /// all reach instances, so it is dead; a live one would have no concrete
    /// target to call.
    ///
    /// Roots are the entry point, the vtable slots and every function except
    /// the opaque bodies and the abstract-dispatching ones; reachability
    /// follows `Call` callees, `FuncAddr` targets and closure drop functions.
    pub(in crate::mir::lower) fn prune_unmonomorphised_bodies(
        &self,
        mir: &mut MirProgram,
    ) -> Result<(), String> {
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
        let opaque = &self.instances.opaque_bodies;
        if poisoned.is_empty() && opaque.is_empty() {
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
            .filter(|n| !poisoned.contains_key(*n) && !opaque.contains(*n))
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
        // An opaque body nothing live reaches goes too: it may call a
        // poisoned body this pass removes.
        let dead: HashSet<String> = mir
            .functions
            .iter()
            .filter(|f| {
                poisoned.contains_key(&f.name)
                    || (opaque.contains(&f.name) && !live.contains(f.name.as_str()))
            })
            .map(|f| f.name.clone())
            .collect();
        mir.functions.retain(|f| !dead.contains(&f.name));
        Ok(())
    }
}
