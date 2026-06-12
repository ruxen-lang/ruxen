use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_fn_call(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Function call ───────────────────────────────────────
            HirExprKind::FnCall {
                callee_name, args, ..
            } => {
                // `super(...)` inside an `init` of a subclass: dispatch to the
                // parent class's init, forwarding the child's `self` as the
                // receiver so that the parent's `@field` auto-assigns write
                // into the same object.
                if callee_name == "super" {
                    if let Some(parent_name) = self.current_parent_class() {
                        let self_local = self.fn_mut().params.first().copied().unwrap_or(0);
                        let mut arg_values = Vec::with_capacity(args.len() + 1);
                        arg_values.push(MirValue::Use(self_local));
                        for arg in args {
                            let local = self.lower_expr(arg)?;
                            arg_values.push(local_to_value(local));
                        }
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: format!("{}_init", parent_name),
                            args: arg_values,
                        });
                        return Ok(None);
                    }
                }

                let mut arg_values = Vec::with_capacity(args.len());
                for arg in args {
                    // Auto-invoke bare zero-arg function references used as
                    // arguments.  Ruxen allows calling a function without
                    // parentheses (`puts greet` ≡ `puts greet()`), so when an
                    // argument is an identifier that resolves to a zero-arg
                    // function, synthesize the invocation rather than passing
                    // the function address as a value.
                    let local = self.lower_fn_arg(arg)?;
                    arg_values.push(local_to_value(local));
                }

                let dest = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                    Some(self.new_temp(expr.ty.clone()))
                } else {
                    None
                };

                // #06.8 Phase 2: rewrite the callee to the C symbol when
                // this call resolves to an FFI fn declared via
                // `lib "X" def NAME as "<c-symbol>"(...)`. The Ruxen-side
                // call site still spells `NAME`; the linker resolves
                // `<c-symbol>`. Non-FFI calls hit the unwrap_or branch
                // and use the Ruxen name unchanged.
                //
                // §B1 — every FFI-alias lookup routes through the
                // single entry `lookup_ffi_alias` (see
                // `mir/lower/mod.rs::lookup_ffi_alias`).
                // Q17: redirect a call to an eligible generic free function
                // to its monomorphic copy specialized for this call's
                // concrete type arguments (`paint_all` → `paint_all__mono__
                // TallySurface`), so a mixin-bound method inside the body
                // dispatches on the concrete type instead of emitting the
                // bound-placeholder callee (`T: Paintable_fill_rect`). Falls
                // through to the FFI-alias / opaque path when no specialization
                // was emitted for these type args (the single-implementor
                // devirtualize case, or a non-generic callee). Checked before
                // the FFI alias because a monomorphized user fn is never an
                // FFI symbol.
                let arg_tys: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
                let callee = if let Some(mangled) = self.fn_mono_callee(callee_name, &arg_tys) {
                    mangled
                } else if self.generic_fn_unmonomorphizable(callee_name) {
                    // An eligible generic fn whose opaque body is suppressed
                    // (it could only emit bound-placeholder callees) was called
                    // from a concrete context but its type args did not resolve
                    // to a fully-concrete vector — so there is no monomorphic
                    // copy to dispatch to and no opaque body to fall back on.
                    // Surface a clear error rather than emit a dangling callee.
                    return Err(format!(
                        "cannot monomorphize generic function `{callee_name}`: its \
                         mixin-bound type argument did not resolve to a concrete type at \
                         this call site (a generic argument cannot itself be a type \
                         parameter with no concrete leaf — see \
                         docs/decisions/q17-cross-package-monomorphization.md)."
                    ));
                } else {
                    self.lookup_ffi_alias(callee_name)
                        .unwrap_or_else(|| callee_name.clone())
                };
                self.emit(MirInst::Call {
                    dest,
                    callee,
                    args: arg_values,
                });
                Ok(dest)
            }

            // ── Method call ─────────────────────────────────────────
            _ => unreachable!("lower_fn_call: dispatched to wrong helper"),
        }
    }
}
