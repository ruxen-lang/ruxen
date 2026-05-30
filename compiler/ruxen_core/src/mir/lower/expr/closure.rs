use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_closure(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Closure ─────────────────────────────────────────────
            HirExprKind::Closure {
                params,
                body,
                is_move,
                ..
            } => {
                // Closure layout (heap-allocated, 16 bytes):
                //   [0] fn_ptr       — address of the synthesized function
                //   [8] captures_ptr — heap block holding captured values
                //                      (one 8-byte slot per capture). May
                //                      be NULL when the closure captures
                //                      nothing.
                //
                // Each capture slot holds either the value directly
                // (ByValue — move or Copy) or a pointer to a single-slot
                // heap cell shared with the enclosing frame (ByRef —
                // used for `let mut` variables the closure mutates).
                let closure_name = format!("__closure_{}", self.closure_counter);
                self.closure_counter += 1;

                // Collect captured def_ids by walking the body.  A def is
                // captured when it is referenced but not defined inside
                // the closure body or declared as a closure parameter.
                let param_def_ids: HashSet<DefId> = params.iter().map(|p| p.def_id).collect();
                let mut captured_def_ids: Vec<DefId> = Vec::new();
                let mut seen: HashSet<DefId> = HashSet::new();
                collect_captures(
                    body,
                    &param_def_ids,
                    &self.def_to_local,
                    &mut captured_def_ids,
                    &mut seen,
                );

                // Decide storage kind per capture.  Copy-typed values
                // can always be captured by value; moved/Copy values go
                // inline; non-move captures of a mutable local that is
                // assigned inside the closure body go through a cell.
                let mut slots: Vec<(DefId, LocalId, Ty, CaptureKind)> =
                    Vec::with_capacity(captured_def_ids.len());
                for def in &captured_def_ids {
                    let outer_local = *self.def_to_local.get(def).unwrap();
                    let ty = self.fn_mut().locals[outer_local as usize].ty.clone();
                    let mutates = closure_body_mutates(body, *def);
                    let kind = if *is_move || !mutates {
                        CaptureKind::ByValue
                    } else {
                        CaptureKind::ByRef
                    };
                    slots.push((*def, outer_local, ty, kind));
                }

                // Cell-promote any captured `let mut` that will be shared
                // by-reference: load the current value into a fresh cell,
                // then rewrite the outer local to hold the cell pointer.
                // From this point on, reads/writes to the outer local go
                // through the cell (see `cell_promoted`).  We only do
                // this once per local — if it's already been promoted by
                // a previous closure in the same function, reuse it.
                for (def, outer_local, _ty, kind) in &slots {
                    if *kind == CaptureKind::ByRef && !self.cell_promoted.contains(def) {
                        let cell = self.new_temp(Ty::Int);
                        self.emit(MirInst::Alloc {
                            dest: cell,
                            ty: Ty::Int,
                            size: 8,
                        });
                        // Store the current value of the local into the cell.
                        self.emit(MirInst::SetField {
                            base: cell,
                            field_index: 0,
                            value: MirValue::Use(*outer_local),
                        });
                        // Rewrite the outer local to hold the cell pointer.
                        self.emit(MirInst::Assign {
                            dest: *outer_local,
                            value: MirValue::Use(cell),
                        });
                        self.cell_promoted.insert(*def);
                    }
                }

                // Allocate the captures struct (or NULL if no captures).
                let captures_ptr = if slots.is_empty() {
                    None
                } else {
                    let cap = self.new_temp(Ty::Int);
                    let size = (slots.len() * 8).max(8);
                    self.emit(MirInst::Alloc {
                        dest: cap,
                        ty: Ty::Int,
                        size,
                    });
                    for (slot_idx, (_def, outer_local, _ty, kind)) in slots.iter().enumerate() {
                        match kind {
                            CaptureKind::ByValue => {
                                // For already-cell-promoted defs, the outer
                                // local is a cell pointer — load the value
                                // out of the cell before storing.  (This
                                // covers the niche case of a ByValue capture
                                // of a local promoted by an earlier closure.)
                                let src_val = if self.cell_promoted.contains(&slots[slot_idx].0) {
                                    let tmp = self.new_temp(Ty::Int);
                                    self.emit(MirInst::GetField {
                                        dest: tmp,
                                        base: *outer_local,
                                        field_index: 0,
                                    });
                                    MirValue::Use(tmp)
                                } else {
                                    MirValue::Use(*outer_local)
                                };
                                self.emit(MirInst::SetField {
                                    base: cap,
                                    field_index: slot_idx,
                                    value: src_val,
                                });
                            }
                            CaptureKind::ByRef => {
                                // Outer local already holds the cell pointer
                                // (we promoted it above).  Just copy the
                                // pointer into the captures slot.
                                self.emit(MirInst::SetField {
                                    base: cap,
                                    field_index: slot_idx,
                                    value: MirValue::Use(*outer_local),
                                });
                            }
                        }
                    }
                    Some(cap)
                };

                // Build the synthesized closure function.  First parameter
                // is the captures pointer (may be NULL for no captures).
                let ret_ty = body.ty.clone();
                let mut closure_fn = MirFunction::new(&closure_name, ret_ty);
                let cap_param = closure_fn.new_local("__captures".to_string(), Ty::Int, false);
                closure_fn.params.push(cap_param);
                let mut closure_param_ids: Vec<LocalId> = Vec::with_capacity(params.len());
                for param in params {
                    let local_id =
                        closure_fn.new_local(param.name.clone(), param.ty.clone(), false);
                    closure_fn.params.push(local_id);
                    closure_param_ids.push(local_id);
                }

                // Save the current lowerer state, lower the closure body
                // in the context of the new function, then restore.
                let saved_fn = self.current_fn.take();
                let saved_block = self.current_block;
                let saved_defs = self.def_to_local.clone();
                let saved_capture_map = std::mem::take(&mut self.capture_map);
                let saved_captures_ptr = self.captures_ptr_local.take();
                let saved_cell_promoted = std::mem::take(&mut self.cell_promoted);

                self.current_fn = Some(closure_fn);
                self.current_block = 0;
                self.captures_ptr_local = if slots.is_empty() {
                    None
                } else {
                    Some(cap_param)
                };

                // Clear def_to_local: only closure params (and captures
                // via the capture map) should be visible inside the body.
                self.def_to_local.clear();
                self.initialized_heap_locals.clear();
                for (i, param) in params.iter().enumerate() {
                    self.def_to_local.insert(param.def_id, closure_param_ids[i]);
                }
                // Populate the capture map.  ByRef captures are visible
                // as cell-promoted defs inside the closure body too — any
                // read/write on them goes through the cell.
                for (slot_idx, (def, _outer, _ty, kind)) in slots.iter().enumerate() {
                    self.capture_map.insert(
                        *def,
                        CaptureSlot {
                            slot_index: slot_idx,
                            kind: *kind,
                        },
                    );
                    if *kind == CaptureKind::ByRef {
                        self.cell_promoted.insert(*def);
                    }
                }

                // Lower the closure body.
                let body_result = self.lower_expr(body)?;
                let ret_is_unit = matches!(body.ty, Ty::Unit | Ty::Never);
                if body_result.is_some() && !ret_is_unit {
                    let body_val = local_to_value(body_result);
                    self.set_terminator(Terminator::Return(Some(body_val)));
                } else {
                    self.set_terminator(Terminator::Return(None));
                }

                // Extract the completed closure function.
                let completed_fn = self.current_fn.take().unwrap();
                self.pending_closures.push(completed_fn);

                // Restore the parent function state.
                self.current_fn = saved_fn;
                self.current_block = saved_block;
                self.def_to_local = saved_defs;
                self.capture_map = saved_capture_map;
                self.captures_ptr_local = saved_captures_ptr;
                self.cell_promoted = saved_cell_promoted;

                // Build the closure pair {fn_ptr, captures_ptr}.
                let fn_ptr = self.new_temp(Ty::Int);
                self.emit(MirInst::FuncAddr {
                    dest: fn_ptr,
                    func_name: closure_name,
                });
                let pair = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest: pair,
                    ty: expr.ty.clone(),
                    size: 16,
                });
                self.emit(MirInst::SetField {
                    base: pair,
                    field_index: 0,
                    value: MirValue::Use(fn_ptr),
                });
                let cap_val = match captures_ptr {
                    Some(cp) => MirValue::Use(cp),
                    None => MirValue::Literal(Literal::Int(0)),
                };
                self.emit(MirInst::SetField {
                    base: pair,
                    field_index: 1,
                    value: cap_val,
                });
                Ok(Some(pair))
            }

            // ── Tuple ───────────────────────────────────────────────
            _ => unreachable!("lower_closure: dispatched to wrong helper"),
        }
    }
}
