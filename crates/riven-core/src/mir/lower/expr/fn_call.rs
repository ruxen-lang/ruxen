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
                    // arguments.  Riven allows calling a function without
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

                self.emit(MirInst::Call {
                    dest,
                    callee: callee_name.clone(),
                    args: arg_values,
                });
                Ok(dest)
            }

            // ── Method call ─────────────────────────────────────────
            _ => unreachable!("lower_fn_call: dispatched to wrong helper"),
        }
    }
}
