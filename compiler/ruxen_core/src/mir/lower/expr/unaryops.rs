use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_unaryops(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Unary operations ────────────────────────────────────
            HirExprKind::UnaryOp { op, operand } => {
                // ── Operator → method desugar (Task OP, Step 3) ──
                // `-a` → `a.-@()`, `!a` → `a.!()` when the operand is a
                // NOMINAL receiver (user/stdlib class). Machine primitives
                // (`Int`/`Float`/`Bool`) fall through to the direct
                // `Negate`/`Not` instructions below — the machine floor.
                // `Deref` is never a method. (`+@` has no surface operator.)
                fn peel(t: &Ty) -> &Ty {
                    match t {
                        Ty::Ref(i)
                        | Ty::RefMut(i)
                        | Ty::RefLifetime(_, i)
                        | Ty::RefMutLifetime(_, i) => peel(i),
                        _ => t,
                    }
                }
                let nominal = matches!(
                    peel(&operand.ty),
                    Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
                );
                let unary_method = match op {
                    UnaryOp::Neg => Some("-@"),
                    UnaryOp::Not => Some("!"),
                    UnaryOp::Deref => None,
                };
                if nominal {
                    if let Some(method) = unary_method {
                        let synthetic = HirExpr {
                            kind: HirExprKind::MethodCall {
                                object: Box::new((**operand).clone()),
                                method: UNRESOLVED_DEF,
                                method_name: method.to_string(),
                                generic_args: vec![],
                                args: vec![],
                                block: None,
                            },
                            ty: expr.ty.clone(),
                            span: expr.span.clone(),
                        };
                        return self.lower_method_call(&synthetic);
                    }
                }

                let src = self.lower_expr(operand)?;
                let val = local_to_value(src);
                let dest = self.new_temp(expr.ty.clone());
                match op {
                    UnaryOp::Neg => self.emit(MirInst::Negate { dest, operand: val }),
                    UnaryOp::Not => self.emit(MirInst::Not { dest, operand: val }),
                    UnaryOp::Deref => {
                        // `*x` — strip one reference level. In Ruxen's value
                        // model a reference is represented the same as its
                        // pointee for scalar types, so this is a plain copy
                        // of the underlying value.
                        self.emit(MirInst::Assign { dest, value: val });
                    }
                }
                Ok(Some(dest))
            }

            // ── Block ───────────────────────────────────────────────

            // ── Borrow ──────────────────────────────────────────────
            HirExprKind::Borrow {
                mutable,
                expr: inner,
            } => {
                let src_local = self.lower_expr(inner)?;
                if let Some(src) = src_local {
                    let dest = self.new_temp(expr.ty.clone());
                    if *mutable {
                        self.emit(MirInst::RefMut { dest, src });
                    } else {
                        self.emit(MirInst::Ref { dest, src });
                    }
                    Ok(Some(dest))
                } else {
                    Ok(None)
                }
            }

            // ── String interpolation ────────────────────────────────
            _ => unreachable!("lower_unaryops: dispatched to wrong helper"),
        }
    }
}
