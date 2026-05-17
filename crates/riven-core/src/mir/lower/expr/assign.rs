use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_assign(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Assignment ──────────────────────────────────────────
            HirExprKind::Assign { target, value, .. } => {
                let val_local = self.lower_expr(value)?;
                let val = local_to_value(val_local);

                match &target.kind {
                    HirExprKind::VarRef(def_id) => {
                        // Captured variable inside a closure body — must be
                        // ByRef (mutation requires cell-shared storage).
                        if let Some(slot) = self.capture_map.get(def_id).copied() {
                            let cap_ptr = self.captures_ptr_local.unwrap();
                            let cell_ptr = self.new_temp(Ty::Int);
                            self.emit(MirInst::GetField {
                                dest: cell_ptr,
                                base: cap_ptr,
                                field_index: slot.slot_index,
                            });
                            self.emit(MirInst::SetField {
                                base: cell_ptr,
                                field_index: 0,
                                value: val,
                            });
                            return Ok(None);
                        }
                        if let Some(&dest) = self.def_to_local.get(def_id) {
                            if self.cell_promoted.contains(def_id) {
                                // Write-through the cell.
                                self.emit(MirInst::SetField {
                                    base: dest,
                                    field_index: 0,
                                    value: val,
                                });
                            } else {
                                // Re-binding a heap-owned local: free the
                                // prior allocation before the new pointer
                                // overwrites it. (P0.2)
                                if self.initialized_heap_locals.contains(&dest) {
                                    self.emit(MirInst::Call {
                                        dest: None,
                                        callee: "riven_dealloc".to_string(),
                                        args: vec![MirValue::Use(dest)],
                                    });
                                }
                                self.emit(MirInst::Assign { dest, value: val });
                                let dest_ty = self
                                    .fn_ref()
                                    .locals
                                    .iter()
                                    .find(|l| l.id == dest)
                                    .map(|l| l.ty.clone());
                                if matches!(
                                    dest_ty,
                                    Some(Ty::Class { .. })
                                        | Some(Ty::Struct { .. })
                                        | Some(Ty::Enum { .. })
                                ) {
                                    self.initialized_heap_locals.insert(dest);
                                }
                            }
                        }
                    }
                    HirExprKind::FieldAccess {
                        object, field_idx, ..
                    } => {
                        let base_local = self.lower_expr(object)?;
                        if let Some(base) = base_local {
                            self.emit(MirInst::SetField {
                                base,
                                field_index: *field_idx,
                                value: val,
                            });
                        }
                    }
                    _ => {
                        // Other assignment targets (index, etc.) — skip for now
                    }
                }
                Ok(None)
            }

            // ── Compound assignment ─────────────────────────────────
            HirExprKind::CompoundAssign { target, op, value } => {
                let rhs_local = self.lower_expr(value)?;
                let rhs_val = local_to_value(rhs_local);

                // ── Phase 2 stdlib batch 2 (#02): String += String ──
                // Lower as `target = String_push_str(target, value)`.
                // The default integer-add path below would treat the
                // heap pointer operands as i64 and corrupt them.
                //
                // Note: we don't emit an explicit free for the prior
                // buffer here. That mirrors the existing `s.push_str(x)`
                // method-call lowering at line ~1546, which also rebinds
                // without freeing. The known temporary leak is shared
                // by both paths and tracked for a future buffer-owning
                // String redesign; closing it here would diverge from
                // push_str semantics and confuse the leak-tracker tests.
                if matches!(op, BinOp::Add)
                    && matches!(target.ty, Ty::String | Ty::Str)
                    && matches!(value.ty, Ty::String | Ty::Str)
                {
                    if let HirExprKind::VarRef(def_id) = &target.kind {
                        if let Some(&dest) = self.def_to_local.get(def_id) {
                            let new_buf = self.new_temp(Ty::String);
                            self.emit(MirInst::Call {
                                dest: Some(new_buf),
                                callee: "String_push_str".to_string(),
                                args: vec![MirValue::Use(dest), rhs_val],
                            });
                            self.emit(MirInst::Assign {
                                dest,
                                value: MirValue::Use(new_buf),
                            });
                            return Ok(None);
                        }
                    }
                }

                match &target.kind {
                    HirExprKind::VarRef(def_id) => {
                        // Captured variable inside a closure body — load
                        // the current value via the cell, apply the op,
                        // store back through the cell.
                        if let Some(slot) = self.capture_map.get(def_id).copied() {
                            let cap_ptr = self.captures_ptr_local.unwrap();
                            let cell_ptr = self.new_temp(Ty::Int);
                            self.emit(MirInst::GetField {
                                dest: cell_ptr,
                                base: cap_ptr,
                                field_index: slot.slot_index,
                            });
                            let cur = self.new_temp(target.ty.clone());
                            self.emit(MirInst::GetField {
                                dest: cur,
                                base: cell_ptr,
                                field_index: 0,
                            });
                            let tmp = self.new_temp(target.ty.clone());
                            if is_comparison(*op) {
                                self.emit(MirInst::Compare {
                                    dest: tmp,
                                    op: binop_to_cmpop(*op),
                                    lhs: MirValue::Use(cur),
                                    rhs: rhs_val,
                                });
                            } else {
                                self.emit(MirInst::BinOp {
                                    dest: tmp,
                                    op: *op,
                                    lhs: MirValue::Use(cur),
                                    rhs: rhs_val,
                                });
                            }
                            self.emit(MirInst::SetField {
                                base: cell_ptr,
                                field_index: 0,
                                value: MirValue::Use(tmp),
                            });
                            return Ok(None);
                        }
                        if let Some(&dest) = self.def_to_local.get(def_id) {
                            // Cell-promoted local: read-modify-write via cell.
                            if self.cell_promoted.contains(def_id) {
                                let cur = self.new_temp(target.ty.clone());
                                self.emit(MirInst::GetField {
                                    dest: cur,
                                    base: dest,
                                    field_index: 0,
                                });
                                let tmp = self.new_temp(target.ty.clone());
                                if is_comparison(*op) {
                                    self.emit(MirInst::Compare {
                                        dest: tmp,
                                        op: binop_to_cmpop(*op),
                                        lhs: MirValue::Use(cur),
                                        rhs: rhs_val,
                                    });
                                } else {
                                    self.emit(MirInst::BinOp {
                                        dest: tmp,
                                        op: *op,
                                        lhs: MirValue::Use(cur),
                                        rhs: rhs_val,
                                    });
                                }
                                self.emit(MirInst::SetField {
                                    base: dest,
                                    field_index: 0,
                                    value: MirValue::Use(tmp),
                                });
                                return Ok(None);
                            }
                            let lhs_val = MirValue::Use(dest);
                            let tmp = self.new_temp(target.ty.clone());
                            if is_comparison(*op) {
                                self.emit(MirInst::Compare {
                                    dest: tmp,
                                    op: binop_to_cmpop(*op),
                                    lhs: lhs_val,
                                    rhs: rhs_val,
                                });
                            } else {
                                self.emit(MirInst::BinOp {
                                    dest: tmp,
                                    op: *op,
                                    lhs: lhs_val,
                                    rhs: rhs_val,
                                });
                            }
                            self.emit(MirInst::Assign {
                                dest,
                                value: MirValue::Use(tmp),
                            });
                        }
                    }
                    HirExprKind::FieldAccess {
                        object, field_idx, ..
                    } => {
                        let base_local = self.lower_expr(object)?;
                        if let Some(base) = base_local {
                            // Load the current field value.
                            let cur = self.new_temp(target.ty.clone());
                            self.emit(MirInst::GetField {
                                dest: cur,
                                base,
                                field_index: *field_idx,
                            });
                            // Perform the operation.
                            let tmp = self.new_temp(target.ty.clone());
                            if is_comparison(*op) {
                                self.emit(MirInst::Compare {
                                    dest: tmp,
                                    op: binop_to_cmpop(*op),
                                    lhs: MirValue::Use(cur),
                                    rhs: rhs_val,
                                });
                            } else {
                                self.emit(MirInst::BinOp {
                                    dest: tmp,
                                    op: *op,
                                    lhs: MirValue::Use(cur),
                                    rhs: rhs_val,
                                });
                            }
                            // Store the result back.
                            self.emit(MirInst::SetField {
                                base,
                                field_index: *field_idx,
                                value: MirValue::Use(tmp),
                            });
                        }
                    }
                    _ => {
                        // Other compound assignment targets (index, etc.) — skip for now
                    }
                }
                Ok(None)
            }

            // ── Construct (struct/class instantiation) ──────────────
            _ => unreachable!("lower_assign: dispatched to wrong helper"),
        }
    }
}
