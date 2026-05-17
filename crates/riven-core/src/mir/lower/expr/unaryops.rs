use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_unaryops(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Unary operations ────────────────────────────────────
            HirExprKind::UnaryOp { op, operand } => {
                let src = self.lower_expr(operand)?;
                let val = local_to_value(src);
                let dest = self.new_temp(expr.ty.clone());
                match op {
                    UnaryOp::Neg => self.emit(MirInst::Negate { dest, operand: val }),
                    UnaryOp::Not => self.emit(MirInst::Not { dest, operand: val }),
                    UnaryOp::Deref => {
                        // `*x` — strip one reference level. In Riven's value
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
