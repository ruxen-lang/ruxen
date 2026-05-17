use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_index(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Index ───────────────────────────────────────────────
            HirExprKind::Index { object, index } => {
                // Fixed-size arrays `[T; N]` are laid out as N consecutive
                // 8-byte slots (the layout used by Alloc + SetField above).
                // When the index is a compile-time integer literal we can
                // lower `a[i]` to a direct `GetField { field_index: i }`.
                if matches!(object.ty, Ty::FixedArray(_, _)) {
                    if let HirExprKind::IntLiteral(n) = &index.kind {
                        let base_local = self.lower_expr(object)?;
                        if let Some(base) = base_local {
                            let dest = self.new_temp(expr.ty.clone());
                            self.emit(MirInst::GetField {
                                dest,
                                base,
                                field_index: *n as usize,
                            });
                            return Ok(Some(dest));
                        }
                    }
                }
                // ── Phase 2 stdlib batch 1 (#03): Vec[i] ──
                // Indexing a Vec at runtime panics on OOB with a
                // descriptive message ("index N out of range, len M").
                // The runtime fn returns the raw 64-bit slot; the
                // typeck-emitted result type pulls out the element T.
                if matches!(object.ty, Ty::Array(_))
                    || matches!(
                        &object.ty,
                        Ty::Ref(inner) | Ty::RefMut(inner)
                            if matches!(inner.as_ref(), Ty::Array(_))
                    )
                {
                    let base_local = self.lower_expr(object)?;
                    let idx_local = self.lower_expr(index)?;
                    let base_val = local_to_value(base_local);
                    let idx_val = local_to_value(idx_local);
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_vec_get_or_panic".to_string(),
                        args: vec![base_val, idx_val],
                    });
                    return Ok(Some(dest));
                }
                // ── Phase 2 stdlib batch 3 (#04): HashMap[&K] ──
                // `m[k]` panics on missing keys via `riven_hash_index`
                // (mirrors `riven_vec_get_or_panic` for Vec). The
                // surface type is V (set in typeck::infer_index_ty);
                // runtime returns the raw 64-bit value slot.
                if matches!(object.ty, Ty::Map(_, _))
                    || matches!(
                        &object.ty,
                        Ty::Ref(inner) | Ty::RefMut(inner)
                            if matches!(inner.as_ref(), Ty::Map(_, _))
                    )
                {
                    let base_local = self.lower_expr(object)?;
                    let idx_local = self.lower_expr(index)?;
                    let base_val = local_to_value(base_local);
                    let idx_val = local_to_value(idx_local);
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_hash_index".to_string(),
                        args: vec![base_val, idx_val],
                    });
                    return Ok(Some(dest));
                }
                // Dynamic index / other collection kinds still need runtime
                // support; fall through as a no-op.
                let _ = (object, index);
                Ok(None)
            }

            // ── Cast ────────────────────────────────────────────────
            _ => unreachable!("lower_index: dispatched to wrong helper"),
        }
    }
}
