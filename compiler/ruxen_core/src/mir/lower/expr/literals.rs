use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_literals(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Literals ────────────────────────────────────────────
            HirExprKind::IntLiteral(n) => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Assign {
                    dest,
                    value: MirValue::Literal(Literal::Int(*n)),
                });
                Ok(Some(dest))
            }
            HirExprKind::FloatLiteral(n) => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Assign {
                    dest,
                    value: MirValue::Literal(Literal::Float(*n)),
                });
                Ok(Some(dest))
            }
            HirExprKind::BoolLiteral(b) => {
                let dest = self.new_temp(Ty::Bool);
                self.emit(MirInst::Assign {
                    dest,
                    value: MirValue::Literal(Literal::Bool(*b)),
                });
                Ok(Some(dest))
            }
            HirExprKind::CharLiteral(c) => {
                let dest = self.new_temp(Ty::Char);
                self.emit(MirInst::Assign {
                    dest,
                    value: MirValue::Literal(Literal::Char(*c)),
                });
                Ok(Some(dest))
            }
            HirExprKind::StringLiteral(s) => {
                // P0.7: wrap raw .rodata pointer through ruxen_string_from so
                // the local owns a heap-allocated String. Without the wrap,
                // String::drop -> free() would double-free a literal pointer.
                let dest = self.emit_owned_string_literal(s);
                Ok(Some(dest))
            }
            HirExprKind::UnitLiteral => Ok(None),

            // ── Variable reference ──────────────────────────────────

            //     value, matching the legacy `null` semantics.
            HirExprKind::NullLiteral => {
                if let Ty::Option(_) = &expr.ty {
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Alloc {
                        dest,
                        ty: expr.ty.clone(),
                        size: self.alloc_size(&expr.ty),
                    });
                    self.emit(MirInst::SetTag { dest, tag: 0 });
                    Ok(Some(dest))
                } else {
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Assign {
                        dest,
                        value: MirValue::Literal(Literal::Int(0)),
                    });
                    Ok(Some(dest))
                }
            }

            // ── Catch-all for unhandled expressions ─────────────────
            _ => unreachable!("lower_literals: dispatched to wrong helper"),
        }
    }
}
