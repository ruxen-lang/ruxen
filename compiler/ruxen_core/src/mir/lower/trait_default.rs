use super::*;
use crate::hir::visit::VisitMut;

impl<'a> Lowerer<'a> {
    /// Synthesize the universal-`Kernel` default bodies (`{Type}_tap`,
    /// `{Type}_then`) for every concrete receiver that called one at a site
    /// with no own/`include`-provided body. `mixin Kernel` is implicitly
    /// carried by every Ruxen value (Ruby's `Kernel`/`Object`), but a
    /// primitive / builtin / non-`include`-ing receiver has no `impl Kernel
    /// for Type` item to drive the per-implementor monomorphization the way
    /// `lower_impl_block_with_outer_methods` does for user classes. Call sites
    /// were recorded into `universal_kernel_needed` during lowering; here we
    /// clone the Kernel default, rewrite `Self` → the concrete receiver, and
    /// lower it under the recorded mangled name — skipping any name a real
    /// body already emitted (an own `def tap` / explicit `include Kernel`), so
    /// a user override always wins.
    pub(super) fn emit_universal_kernel_bodies(
        &mut self,
        mir: &mut MirProgram,
    ) -> Result<(), String> {
        let Some(kernel_defaults) = self.trait_default_methods.get("Kernel").cloned() else {
            return Ok(());
        };
        let already_emitted: HashSet<String> =
            mir.functions.iter().map(|f| f.name.clone()).collect();
        // Deterministic order: sort by mangled name so emission (and any
        // resulting diagnostics / symbol order) is stable across runs.
        let mut needed: Vec<(String, (Ty, String))> =
            std::mem::take(&mut self.universal_kernel_needed)
                .into_iter()
                .collect();
        needed.sort_by(|a, b| a.0.cmp(&b.0));
        for (mangled, (self_ty, method_name)) in needed {
            if already_emitted.contains(&mangled) {
                continue;
            }
            let Some(default_fn) = kernel_defaults.get(&method_name) else {
                continue;
            };
            let mut cloned = default_fn.clone();
            rewrite_self_in_func(&mut cloned, &self_ty);
            // Pin the implicit `self` type: a synthesized name like
            // `Array[Int]_tap` names no symbol-table class, so the mangled-name
            // derivation in `lower_method` would produce `Ty::Unit`.
            self.self_ty_override = Some(self_ty.clone());
            let mir_fn = self.lower_method(&mangled, &cloned);
            self.self_ty_override = None;
            mir.functions.push(mir_fn?);
        }
        Ok(())
    }
}

/// Rewrite every occurrence of `Ty::TypeParam { name == "Self" }` inside a
/// cloned trait default method's body/params/return type to point at the
/// concrete `impl` target. This is how we monomorphise a default method for
/// each implementor so that `self.field` / `self.other_method` dispatch
/// resolves through the normal `{ConcreteType}_{method}` path.
pub(super) fn rewrite_self_in_func(func: &mut HirFuncDef, concrete: &Ty) {
    let subst = std::collections::HashMap::from([("Self".to_string(), concrete.clone())]);
    subst_type_params_in_func(func, &subst);
}

/// Generic-class monomorphization (option 1): substitute every
/// `Ty::TypeParam { name }` whose `name` is a key in `subst` with the
/// concrete type bound to it, through `Ty::subst_params`. This is the
/// generalization of `rewrite_self_in_func` from the single `Self` rewrite
/// to an arbitrary `{ param_name → concrete Ty }` map, so a generic class
/// method body (`Box[T].eq`) can be specialized to `T = String` before MIR
/// binop / interpolation lowering runs — at which point the existing
/// `Ty::String` special-cases in `binops.rs` / `interpolation.rs` fire
/// naturally.
pub(super) fn subst_type_params_in_func(
    func: &mut HirFuncDef,
    subst: &std::collections::HashMap<String, Ty>,
) {
    TypeParamSubst { subst }.visit_func_mut(func);
}

/// Applies `Ty::subst_params` to every type the shared HIR walk reaches.
struct TypeParamSubst<'a> {
    subst: &'a std::collections::HashMap<String, Ty>,
}

impl VisitMut for TypeParamSubst<'_> {
    fn visit_ty_mut(&mut self, ty: &mut Ty) {
        *ty = ty.subst_params(self.subst);
    }
}
