use super::super::*;

impl<'a> Lowerer<'a> {
    /// Store an already-lowered value `val` into an assignment target place
    /// (a plain/captured/cell-promoted variable, a struct field, or a
    /// fixed-array / backing-Vec index). Shared by single `Assign` lowering and
    /// `MultiAssign` so both use one code path for every target kind.
    fn store_value_into_target(&mut self, target: &HirExpr, val: MirValue) -> Result<(), String> {
        match &target.kind {
            HirExprKind::VarRef(def_id) => {
                // Captured variable inside a closure body is ByRef (mutation
                // requires cell-shared storage).
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
                    return Ok(());
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
                        // Re-binding a heap-owned local: free the prior
                        // allocation before the new pointer overwrites it. (P0.2)
                        if self.initialized_heap_locals.contains(&dest) {
                            self.emit(MirInst::Call {
                                dest: None,
                                callee: "ruxen_dealloc".to_string(),
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
                    // Phase B-4: shift past class_info_ptr header for
                    // runtime-dispatch classes.
                    let shift = self.class_field_shift_for_ty(&object.ty);
                    self.emit(MirInst::SetField {
                        base,
                        field_index: *field_idx + shift,
                        value: val,
                    });
                }
            }
            // `xs[i] = v` index assignment (Q9). Mirrors the read path in
            // `lower_index`: a fixed-array with a literal index stores a slot
            // directly; a backing-Vec receiver calls the bounds-checked
            // `ruxen_vec_set`. Map (`m[k] = v`) still goes through `.insert`.
            HirExprKind::Index { object, index } => {
                if matches!(object.ty, Ty::FixedArray(_, _)) {
                    if let HirExprKind::IntLiteral(n) = &index.kind {
                        if let Some(base) = self.lower_expr(object)? {
                            self.emit(MirInst::SetField {
                                base,
                                field_index: *n as usize,
                                value: val,
                            });
                        }
                        return Ok(());
                    }
                }
                if super::index::is_indexable_vec_ty(&object.ty) {
                    let base_local = self.lower_expr(object)?;
                    let idx_local = self.lower_expr(index)?;
                    if let (Some(base), Some(idx)) = (base_local, idx_local) {
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: "ruxen_vec_set".to_string(),
                            args: vec![MirValue::Use(base), MirValue::Use(idx), val],
                        });
                    }
                }
            }
            _ => {
                // Other assignment targets are not stored (no-op for now).
            }
        }
        Ok(())
    }

    /// Free the Array currently held by a trailing `*rest` target before the
    /// freshly-built tail replaces it. Typeck restricts this target to a
    /// pre-declared variable and its element type to Copy, so the spine-only
    /// Array free is sufficient.
    fn free_splat_rest_target(&mut self, target: &HirExpr) -> Result<(), String> {
        let HirExprKind::VarRef(def_id) = &target.kind else {
            return Err("assignment splat target is not a variable".to_string());
        };
        let callee = super::super::drops::heap_free_callee(&target.ty)
            .ok_or_else(|| format!("assignment splat target is not heap-owned: {}", target.ty))?
            .to_string();

        let old_value = if let Some(slot) = self.capture_map.get(def_id).copied() {
            let cap_ptr = self.captures_ptr_local.unwrap();
            let cell_ptr = self.new_temp(Ty::Int);
            self.emit(MirInst::GetField {
                dest: cell_ptr,
                base: cap_ptr,
                field_index: slot.slot_index,
            });
            let old = self.new_temp(target.ty.clone());
            self.emit(MirInst::GetField {
                dest: old,
                base: cell_ptr,
                field_index: 0,
            });
            old
        } else if let Some(&dest) = self.def_to_local.get(def_id) {
            if self.cell_promoted.contains(def_id) {
                let old = self.new_temp(target.ty.clone());
                self.emit(MirInst::GetField {
                    dest: old,
                    base: dest,
                    field_index: 0,
                });
                old
            } else {
                dest
            }
        } else {
            return Err("assignment splat target has no local storage".to_string());
        };

        self.emit(MirInst::Call {
            dest: None,
            callee,
            args: vec![MirValue::Use(old_value)],
        });
        Ok(())
    }

    /// Parallel assignment `a, b = e1, e2` and trailing assignment splat
    /// `a, *rest = xs`. Every right-hand value is lowered before any target is
    /// written. A splatted Array RHS is read once, scalar prefix values are
    /// snapshotted, and `rest` receives a fresh tail Array. A comma-list RHS
    /// builds `rest` from its remaining snapshotted values.
    pub(super) fn lower_multiassign(
        &mut self,
        targets: &[HirExpr],
        values: &[HirExpr],
        splat_target: Option<usize>,
    ) -> Result<Option<LocalId>, String> {
        if let Some(rest_index) = splat_target {
            let rest_target = targets
                .get(rest_index)
                .ok_or_else(|| "invalid assignment splat target".to_string())?;

            let array_rhs = values.len() == 1 && matches!(values[0].ty.peel_refs(), Ty::Array(_));
            if array_rhs {
                let source = self
                    .lower_expr(&values[0])?
                    .ok_or_else(|| "assignment splat Array produced no value".to_string())?;

                let elem_ty = match values[0].ty.peel_refs() {
                    Ty::Array(elem) => (**elem).clone(),
                    _ => unreachable!("array_rhs checked above"),
                };
                let mut prefix = Vec::with_capacity(rest_index);
                for index in 0..rest_index {
                    let snapshot = self.new_temp(elem_ty.clone());
                    self.emit(MirInst::Call {
                        dest: Some(snapshot),
                        callee: "ruxen_vec_get_or_panic".to_string(),
                        args: vec![
                            MirValue::Use(source),
                            MirValue::Literal(Literal::Int(index as i64)),
                        ],
                    });
                    prefix.push(MirValue::Use(snapshot));
                }

                let tail = self.new_temp(rest_target.ty.clone());
                self.emit(MirInst::Call {
                    dest: Some(tail),
                    callee: "ruxen_vec_skip".to_string(),
                    args: vec![
                        MirValue::Use(source),
                        MirValue::Literal(Literal::Int(rest_index as i64)),
                    ],
                });

                for (target, value) in targets.iter().take(rest_index).zip(prefix) {
                    self.store_value_into_target(target, value)?;
                }
                self.free_splat_rest_target(rest_target)?;
                self.store_value_into_target(rest_target, MirValue::Use(tail))?;
                return Ok(None);
            }

            if values.len() < rest_index {
                return Err(format!(
                    "assignment splat needs {rest_index} prefix values, found {}",
                    values.len()
                ));
            }

            let mut snapshots = Vec::with_capacity(values.len());
            for value in values {
                let lowered = self.lower_expr(value)?;
                let snapshot = self.new_temp(value.ty.clone());
                self.emit(MirInst::Assign {
                    dest: snapshot,
                    value: local_to_value(lowered),
                });
                snapshots.push(MirValue::Use(snapshot));
            }

            let rest = self.new_temp(rest_target.ty.clone());
            self.emit(MirInst::Call {
                dest: Some(rest),
                callee: "ruxen_vec_new".to_string(),
                args: vec![],
            });
            for value in snapshots.iter().skip(rest_index) {
                self.emit(MirInst::Call {
                    dest: None,
                    callee: "ruxen_vec_push".to_string(),
                    args: vec![MirValue::Use(rest), value.clone()],
                });
            }

            for (target, value) in targets
                .iter()
                .take(rest_index)
                .zip(snapshots.into_iter().take(rest_index))
            {
                self.store_value_into_target(target, value)?;
            }
            self.free_splat_rest_target(rest_target)?;
            self.store_value_into_target(rest_target, MirValue::Use(rest))?;
            return Ok(None);
        }

        // Snapshot every RHS into a FRESH temp before any target is written. A
        // bare `Use(var)` would alias the variable's storage, so on `a, b = b, a`
        // the second store would read the already-overwritten `a`. Copying into a
        // new temp captures the pre-assignment value.
        let mut snapshots = Vec::with_capacity(values.len());
        for value in values {
            let v = self.lower_expr(value)?;
            let temp = self.new_temp(value.ty.clone());
            self.emit(MirInst::Assign {
                dest: temp,
                value: local_to_value(v),
            });
            snapshots.push(MirValue::Use(temp));
        }
        for (target, val) in targets.iter().zip(snapshots) {
            self.store_value_into_target(target, val)?;
        }
        Ok(None)
    }

    pub(super) fn lower_assign(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Assignment ──────────────────────────────────────────
            HirExprKind::Assign { target, value, .. } => {
                // ── Operator → method desugar (Task OP, Step 3) ──
                // `a[i] = v` → `a.[]=(i, v)` when the indexed receiver is
                // NOMINAL (user/stdlib class with `def []=`). Done BEFORE
                // lowering `value` so the synthetic method call lowers its
                // args exactly once. Builtin collection index-assign is not
                // handled here (Array/Map mutation goes through their own
                // methods); only a nominal `def []=` receiver routes.
                if let HirExprKind::Index { object, index } = &target.kind {
                    fn peel(t: &Ty) -> &Ty {
                        match t {
                            Ty::Ref(i)
                            | Ty::RefMut(i)
                            | Ty::RefLifetime(_, i)
                            | Ty::RefMutLifetime(_, i) => peel(i),
                            _ => t,
                        }
                    }
                    if matches!(
                        peel(&object.ty),
                        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
                    ) {
                        let synthetic = HirExpr {
                            kind: HirExprKind::MethodCall {
                                object: Box::new((**object).clone()),
                                method: UNRESOLVED_DEF,
                                method_name: "[]=".to_string(),
                                generic_args: vec![],
                                args: vec![(**index).clone(), (**value).clone()],
                                block: None,
                            },
                            ty: Ty::Unit,
                            span: expr.span.clone(),
                        };
                        return self.lower_method_call(&synthetic);
                    }
                }

                let val_local = self.lower_expr(value)?;
                let val = local_to_value(val_local);
                self.store_value_into_target(target, val)?;
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
                    && matches!(target.ty, Ty::String)
                    && matches!(value.ty, Ty::String)
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
                            // Phase B-4: shift past class_info_ptr
                            // header. Use the same shifted index for
                            // both the load (cur) and the store (back)
                            // — both ops target the same slot.
                            let shift = self.class_field_shift_for_ty(&object.ty);
                            let shifted_idx = *field_idx + shift;
                            // Load the current field value.
                            let cur = self.new_temp(target.ty.clone());
                            self.emit(MirInst::GetField {
                                dest: cur,
                                base,
                                field_index: shifted_idx,
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
                                field_index: shifted_idx,
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
