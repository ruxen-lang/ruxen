use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_var_ref(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Variable reference ──────────────────────────────────
            HirExprKind::VarRef(def_id) => {
                // Captured variable inside a closure body: load from the
                // captures pointer.  ByValue → a direct load; ByRef → load
                // the cell pointer and dereference through it.
                if let Some(slot) = self.capture_map.get(def_id).copied() {
                    let cap_ptr = self
                        .captures_ptr_local
                        .expect("capture_map non-empty implies captures_ptr_local is set");
                    match slot.kind {
                        CaptureKind::ByValue => {
                            let dest = self.new_temp(expr.ty.clone());
                            self.emit(MirInst::GetField {
                                dest,
                                base: cap_ptr,
                                field_index: slot.slot_index,
                            });
                            return Ok(Some(dest));
                        }
                        CaptureKind::ByRef => {
                            let cell_ptr = self.new_temp(Ty::Int);
                            self.emit(MirInst::GetField {
                                dest: cell_ptr,
                                base: cap_ptr,
                                field_index: slot.slot_index,
                            });
                            let dest = self.new_temp(expr.ty.clone());
                            self.emit(MirInst::GetField {
                                dest,
                                base: cell_ptr,
                                field_index: 0,
                            });
                            return Ok(Some(dest));
                        }
                    }
                }
                if let Some(&local) = self.def_to_local.get(def_id) {
                    // Cell-promoted locals (mutably captured by a closure
                    // in this frame) hold a pointer to an 8-byte cell;
                    // reads go through the cell.
                    if self.cell_promoted.contains(def_id) {
                        let dest = self.new_temp(expr.ty.clone());
                        self.emit(MirInst::GetField {
                            dest,
                            base: local,
                            field_index: 0,
                        });
                        return Ok(Some(dest));
                    }
                    Ok(Some(local))
                } else if let Some(const_expr) = self.const_values.get(def_id).cloned() {
                    // Reference to a top-level `const` — substitute the
                    // initializer expression inline at this use site.
                    self.lower_expr(&const_expr)
                } else {
                    // Might be a top-level function reference — just return None
                    // for now; calls use the callee_name directly.
                    Ok(None)
                }
            }

            // ── Binary operations ───────────────────────────────────
            _ => unreachable!("lower_var_ref: dispatched to wrong helper"),
        }
    }
}
