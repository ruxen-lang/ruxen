use super::super::*;

impl<'a> Lowerer<'a> {
    /// True when `o[i] = v` on a receiver of this type desugars to the
    /// receiver's `[]=` method rather than the index-assign machine floor.
    ///
    /// Two receiver shapes qualify: a NOMINAL type (user/stdlib
    /// `class`/`struct`/`enum`), and a BUILTIN head (`String`/`Array`/`Hash`/
    /// `Set`) whose `.rx` class RESOLVES `[]=` — via a `def` or an `alias`,
    /// which is how `Hash` provides it (`alias []= insert`). The builtin-head
    /// half is the write-side twin of the builtin-head OPERATOR route in
    /// `binops.rs`, and it reuses that module's head set and method probe so
    /// the read/binop side and the write side cannot drift apart on which
    /// receivers may dispatch to `.rx`.
    ///
    /// `lower_assign` consults this to take the route; the machine floor
    /// consults it to tell "this receiver has no `[]=` at all" apart from "this
    /// receiver HAS one but the caller reached the floor without desugaring"
    /// (parallel assignment), so its refusal names the real limitation.
    fn index_assign_routes_to_method(&self, object_ty: &Ty) -> bool {
        let head = object_ty.peel_refs();
        matches!(head, Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. })
            || (super::binops::is_builtin_op_head(head)
                && self.builtin_head_has_op_method(head, "[]="))
    }

    /// Store an already-lowered value `val` into an assignment target place
    /// (a plain/captured/cell-promoted variable, a struct field, or a
    /// fixed-array / backing-Vec index). Shared by single `Assign` lowering and
    /// `MultiAssign` so both use one code path for every target kind.
    pub(super) fn store_value_into_target(
        &mut self,
        target: &HirExpr,
        val: MirValue,
    ) -> Result<(), String> {
        match &target.kind {
            HirExprKind::VarRef(def_id) => {
                // Captured variable inside a closure body is ByRef (mutation
                // requires cell-shared storage).
                // A slot-addressed `&var T` stores a `T` through the
                // caller's address (`mir::refmut_abi`).
                if self.refmut_slots.contains(def_id) {
                    let addr = self.refmut_slot_addr(*def_id)?;
                    let pointee = target.ty.peel_refs().clone();
                    self.store_through_refmut(addr, &pointee, val);
                    return Ok(());
                }
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
                        // allocation (and the fields it owns) before the new
                        // pointer overwrites it. (P0.2)
                        if self.initialized_heap_locals.contains(&dest) {
                            let ty = self.fn_ref().locals[dest as usize].ty.clone();
                            let symbols = self.symbols;
                            let free = super::super::drops::heap_free_call(
                                self.fn_mut(),
                                symbols,
                                &ty,
                                dest,
                            )
                            .expect("an initialized heap local has an aggregate type");
                            for inst in free {
                                self.emit(inst);
                            }
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
                    let field_index = *field_idx + shift;
                    let owned = match object.ty.peel_refs() {
                        Ty::Class { name, .. } => {
                            self.symbols.class_owns_fields(name)
                                && super::super::drops::field_owns_value(&target.ty, self.symbols)
                        }
                        _ => false,
                    };
                    if !owned {
                        self.emit(MirInst::SetField {
                            base,
                            field_index,
                            value: val,
                        });
                        return Ok(());
                    }
                    // The field owns its value: the store moves `val` in and
                    // frees the value it replaces, unless that is the same
                    // object (a self-returning update) or NULL (never stored,
                    // e.g. the first store in `init`; `ruxen_alloc` zeroes).
                    // A closure is counted, not compared: `val` is an owner
                    // of its own (a literal, a moved value, a `.clone`
                    // share), so the replaced owner is always released.
                    let counted = super::super::drops::is_owned_closure(&target.ty);
                    let old = self.new_temp(target.ty.clone());
                    self.emit(MirInst::GetField {
                        dest: old,
                        base,
                        field_index,
                    });
                    self.fn_mut().record_view(old, View::Own);
                    let replaced = (!counted).then(|| {
                        let replaced = self.new_temp(Ty::Bool);
                        self.emit(MirInst::Compare {
                            dest: replaced,
                            op: CmpOp::NotEq,
                            lhs: MirValue::Use(old),
                            rhs: val.clone(),
                        });
                        replaced
                    });
                    self.emit(MirInst::SetField {
                        base,
                        field_index,
                        value: val,
                    });
                    let live = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: live,
                        op: CmpOp::NotEq,
                        lhs: MirValue::Use(old),
                        rhs: MirValue::Literal(Literal::Int(0)),
                    });
                    let check_live = self.new_block();
                    let drop_block = self.new_block();
                    let done = self.new_block();
                    match replaced {
                        Some(replaced) => self.set_terminator(Terminator::Branch {
                            cond: MirValue::Use(replaced),
                            then_block: check_live,
                            else_block: done,
                        }),
                        None => self.set_terminator(Terminator::Goto(check_live)),
                    }
                    self.current_block = check_live;
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(live),
                        then_block: drop_block,
                        else_block: done,
                    });
                    self.current_block = drop_block;
                    let user_drop = self.user_drop_callee(&target.ty);
                    let symbols = self.symbols;
                    let free = super::super::drops::owned_value_drop(
                        self.fn_mut(),
                        symbols,
                        user_drop,
                        &target.ty,
                        old,
                    );
                    for inst in free {
                        self.emit(inst);
                    }
                    self.set_terminator(Terminator::Goto(done));
                    self.current_block = done;
                }
            }
            // `xs[i] = v` index assignment (Q9), machine floor. Mirrors the read
            // path in `lower_index`: a fixed-array with a literal index stores a
            // slot directly; a backing-Vec receiver calls the bounds-checked
            // `ruxen_vec_set`. Receivers that dispatch to an `.rx` `[]=`
            // (nominal, or a builtin head that resolves one) are intercepted by
            // `lower_assign` and never reach here.
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
                    return Ok(());
                }
                // No store path for this receiver. Refuse to lower rather than
                // drop the write: a silently discarded `o[i] = v` is exactly the
                // failure mode B72 was (`Ty::Map` matched no arm above and the
                // store vanished), and the repo forbids a silent no-op on any
                // user-callable surface.
                //
                // Three distinct reasons land here; each gets its own message,
                // because a wrong-but-confident "define `[]=`" would misdirect
                // two of the three.
                if matches!(object.ty, Ty::FixedArray(_, _)) {
                    return Err(format!(
                        "index assignment on the fixed-size array `{}` requires a \
                         literal index; use an `Array` for a computed index",
                        object.ty
                    ));
                }
                if self.index_assign_routes_to_method(&object.ty) {
                    // The receiver DOES have `[]=`; the caller reached the floor
                    // without going through the desugar. Only `MultiAssign`
                    // does that today — it stores an already-lowered value, so
                    // it has no HIR argument to hand a synthetic method call.
                    return Err(format!(
                        "parallel assignment cannot write through an index on `{}`; \
                         assign each index in its own statement",
                        object.ty
                    ));
                }
                return Err(format!(
                    "no `[]=` on type `{}`, so `o[i] = v` has no store path; \
                     define one on its class",
                    object.ty
                ));
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

        let symbols = self.symbols;
        let free =
            super::super::drops::heap_free_call(self.fn_mut(), symbols, &target.ty, old_value)
                .ok_or_else(|| {
                    format!("assignment splat target is not heap-owned: {}", target.ty)
                })?;
        for inst in free {
            self.emit(inst);
        }
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
                // `a[i] = v` → `a.[]=(i, v)` for every receiver whose `[]=`
                // resolves (see `index_assign_routes_to_method`). Done BEFORE
                // lowering `value`, so the synthetic method call lowers its args
                // exactly once. Everything else falls through to
                // `store_value_into_target`'s machine floor, which is what keeps
                // `Array`/`FixedArray` index-assign on `ruxen_vec_set` /
                // `SetField`. B72: before the builtin-head half of that
                // predicate existed, `Ty::Map` matched neither this test nor any
                // arm of that floor, so `h[k] = v` type-checked and vanished.
                if let HirExprKind::Index { object, index } = &target.kind {
                    if self.index_assign_routes_to_method(&object.ty) {
                        let synthetic = HirExpr {
                            kind: HirExprKind::MethodCall {
                                object: Box::new((**object).clone()),
                                method: UNRESOLVED_DEF,
                                resolved: UNRESOLVED_DEF,
                                requirement_params: None,
                                method_name: "[]=".to_string(),
                                generic_args: vec![],
                                args: vec![(**index).clone(), (**value).clone()],
                                block: None,
                                safe_nav: false,
                                parens: true,
                            },
                            ty: Ty::Unit,
                            span: expr.span.clone(),
                        };
                        return self.lower_method_call(&synthetic);
                    }
                }

                let val_local = self.lower_expr(value)?;
                let val = local_to_value(val_local);
                self.free_replaced_refmut_string(target, value)?;
                self.store_value_into_target(target, val)?;
                if expr.ty.is_copy() && expr.ty != Ty::Unit {
                    Ok(val_local)
                } else {
                    Ok(None)
                }
            }

            // ── Compound assignment ─────────────────────────────────
            HirExprKind::CompoundAssign { target, op, value } => {
                let rhs_local = self.lower_expr(value)?;
                let rhs_val = local_to_value(rhs_local);

                if let HirExprKind::VarRef(def_id) = &target.kind {
                    if self.refmut_slots.contains(def_id) {
                        let addr = self.refmut_slot_addr(*def_id)?;
                        return self.compound_assign_through(addr, target, *op, value, rhs_val);
                    }
                }

                // `s += t` on Strings appends in place, as `s.push_str(t)`.
                // The default integer-add path below would treat the heap
                // pointer operands as i64 and corrupt them.
                if matches!(target.ty, Ty::String) && matches!(value.ty, Ty::String) {
                    if let BinOp::Add = op {
                        let receiver = self.lower_slot_receiver(target)?;
                        if let Some(receiver) = receiver {
                            self.fn_mut().record_view(receiver, View::Assignee);
                        }
                        let target_val = local_to_value(receiver);
                        let callee = self.string_append_callee();
                        self.emit_rebinding_self_call(
                            target,
                            callee,
                            vec![target_val, rhs_val],
                            None,
                        )?;
                        return Ok(None);
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
                            if op.is_comparison() {
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
                                if op.is_comparison() {
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
                            if op.is_comparison() {
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
                            if op.is_comparison() {
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
                    // An Array element is updated in its own slot.
                    HirExprKind::Index { object, index }
                        if !target.is_unaddressable_var_place() =>
                    {
                        let Some(addr) = self.element_slot_addr(object, index, &target.ty)? else {
                            return Ok(None);
                        };
                        return self.compound_assign_through(addr, target, *op, value, rhs_val);
                    }
                    _ => {
                        return Err(format!(
                            "`{op:?}=` cannot update this place in place (only a variable, a \
                             field or an Array element can be); write `x = x {op:?} v` instead"
                        ));
                    }
                }
                Ok(None)
            }

            // ── Construct (struct/class instantiation) ──────────────
            _ => unreachable!("lower_assign: dispatched to wrong helper"),
        }
    }

    /// The address a slot-addressed `&var T` variable holds: its own local,
    /// or, inside a closure, the capture's storage (`var_storage_addr`: the
    /// cell or caller's address a by-reference capture holds, or the
    /// closure's own slot for a String it owns).
    pub(super) fn refmut_slot_addr(&mut self, def_id: DefId) -> Result<LocalId, String> {
        if self.capture_map.contains_key(&def_id) {
            return self
                .var_storage_addr(def_id)?
                .ok_or_else(|| "a captured `&var` variable has no storage".to_string());
        }
        self.def_to_local
            .get(&def_id)
            .copied()
            .ok_or_else(|| "a `&var` variable has no local storage".to_string())
    }

    /// `s = v` on a `&var String` replaces the caller's String: free the
    /// buffer it held, which nothing else owns. `value` is already lowered;
    /// a self-assignment (`s = s`) keeps the buffer it stores.
    fn free_replaced_refmut_string(
        &mut self,
        target: &HirExpr,
        value: &HirExpr,
    ) -> Result<(), String> {
        let HirExprKind::VarRef(def_id) = &target.kind else {
            return Ok(());
        };
        if !self.refmut_slots.contains(def_id) || !target.ty.is_string_form() {
            return Ok(());
        }
        if matches!(&value.kind, HirExprKind::VarRef(v) if v == def_id) {
            return Ok(());
        }
        let addr = self.refmut_slot_addr(*def_id)?;
        let old = self.load_through_refmut(addr, &Ty::String, &Ty::String);
        let symbols = self.symbols;
        let free = super::super::drops::heap_free_call(self.fn_mut(), symbols, &Ty::String, old)
            .expect("String has a free helper");
        for inst in free {
            self.emit(inst);
        }
        Ok(())
    }

    /// `x op= v` on a place reached through `addr`, the address of its
    /// storage (a slot-addressed `&var T` variable, an Array element): read
    /// the `T` through the address, apply the operator at the pointee's own
    /// type (a float adds as a float, not as its bit pattern) and store the
    /// `T` back. `s += t` on a String appends in place through the address
    /// (`String#push_str`).
    /// The callee `s += t` appends through: the `String#push_str`
    /// declaration, resolved to its C symbol.
    fn string_append_callee(&self) -> String {
        self.resolve_ffi_alias_callee("String_push_str".to_string())
    }

    fn compound_assign_through(
        &mut self,
        addr: LocalId,
        target: &HirExpr,
        op: BinOp,
        value: &HirExpr,
        rhs_val: MirValue,
    ) -> Result<Option<LocalId>, String> {
        let pointee = target.ty.peel_refs().clone();
        if pointee.is_string_form() {
            if !(matches!(op, BinOp::Add) && value.ty.is_string_form()) {
                return Err(format!(
                    "`{op:?}=` is not defined on a `&var String`; only `+=` appends"
                ));
            }
            let callee = self.string_append_callee();
            self.fn_mut().record_view(addr, View::Assignee);
            self.emit(MirInst::Call {
                dest: None,
                callee,
                args: vec![MirValue::Use(addr), rhs_val],
            });
            return Ok(None);
        }
        let cur = self.load_through_refmut(addr, &pointee, &pointee);
        let rhs_val = match rhs_val {
            MirValue::Use(src)
                if self.fn_ref().locals[src as usize].ty.peel_refs() == &pointee
                    && self.fn_ref().locals[src as usize].ty != pointee =>
            {
                let narrowed = self.new_temp(pointee.clone());
                self.emit(MirInst::Copy {
                    dest: narrowed,
                    src,
                });
                MirValue::Use(narrowed)
            }
            other => other,
        };
        let result = self.new_temp(pointee.clone());
        if op.is_comparison() {
            self.emit(MirInst::Compare {
                dest: result,
                op: binop_to_cmpop(op),
                lhs: MirValue::Use(cur),
                rhs: rhs_val,
            });
        } else {
            self.emit(MirInst::BinOp {
                dest: result,
                op,
                lhs: MirValue::Use(cur),
                rhs: rhs_val,
            });
        }
        self.store_through_refmut(addr, &pointee, MirValue::Use(result));
        Ok(None)
    }
}
