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
                let callee = self
                    .lookup_ffi_alias(callee_name)
                    .unwrap_or_else(|| callee_name.clone());
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
