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
                //
                // The set of defs VISIBLE in the enclosing frame is every
                // def `lower_var_ref` could resolve there: an outer-frame
                // local (`def_to_local`) OR — when this closure literal is
                // itself nested inside another closure's body — a capture of
                // that enclosing closure (`capture_map`). Without the
                // `capture_map` half, a closure nested inside a closure body
                // (e.g. `outer.build({ |c| c.add({ || v + 1 }) })`, where
                // `v` is captured by the OUTER block) would not re-capture
                // `v`; the nested closure would read `v` as slot garbage.
                // (Q26.)
                let param_def_ids: HashSet<DefId> = params.iter().map(|p| p.def_id).collect();
                let visible_defs: HashSet<DefId> = self
                    .def_to_local
                    .keys()
                    .copied()
                    .chain(self.capture_map.keys().copied())
                    .collect();
                let mut captured_def_ids: Vec<DefId> = Vec::new();
                let mut seen: HashSet<DefId> = HashSet::new();
                collect_captures(
                    body,
                    &param_def_ids,
                    &visible_defs,
                    &mut captured_def_ids,
                    &mut seen,
                );

                // Decide storage kind per capture.  Copy-typed values
                // can always be captured by value; moved/Copy values go
                // inline; non-move captures of a mutable local that is
                // assigned inside the closure body go through a cell.
                //
                // A capture resolves through one of two enclosing-frame
                // mechanisms — exactly the two `lower_var_ref` consults:
                //   • `Local`     — an outer-frame local (`def_to_local`).
                //   • `Recapture` — a capture of the ENCLOSING closure
                //                   (`capture_map`), i.e. this closure
                //                   literal is nested inside another
                //                   closure's body and re-captures one of
                //                   the outer block's captures (Q26).
                let mut slots: Vec<(DefId, CaptureSource, Ty, CaptureKind)> =
                    Vec::with_capacity(captured_def_ids.len());
                for def in &captured_def_ids {
                    let mutates = closure_body_mutates(body, *def);
                    let want_byref = !*is_move && mutates;

                    if let Some(&local) = self.def_to_local.get(def) {
                        let ty = self.fn_mut().locals[local as usize].ty.clone();
                        let kind = if want_byref {
                            CaptureKind::ByRef
                        } else {
                            CaptureKind::ByValue
                        };
                        slots.push((*def, CaptureSource::Local(local), ty, kind));
                    } else if let Some(enclosing) = self.capture_map.get(def).copied() {
                        // Re-capture from the enclosing closure's captures.
                        // Every capture slot is an 8-byte word (value, or a
                        // cell pointer for ByRef); the recorded `ty` is unused
                        // downstream, so `Ty::Int` (pointer width) suffices.
                        let ty = Ty::Int;
                        // A nested closure that MUTATES an outer-captured
                        // value needs a shared cell. We can only propagate a
                        // cell that already exists (the enclosing capture was
                        // itself ByRef). Mutating a ByValue outer capture from
                        // a doubly-nested closure would require promoting the
                        // enclosing capture to a cell after the fact, which is
                        // not supported — reject rather than miscompile.
                        let kind = if want_byref {
                            if enclosing.kind == CaptureKind::ByRef {
                                CaptureKind::ByRef
                            } else {
                                return Err(format!(
                                    "closure nested inside another closure mutates an \
                                     outer-captured value `{}` that is captured by value; \
                                     this re-capture-and-mutate shape is not supported",
                                    def_id_name(*def, self.symbols)
                                ));
                            }
                        } else {
                            CaptureKind::ByValue
                        };
                        slots.push((*def, CaptureSource::Recapture(enclosing), ty, kind));
                    } else {
                        return Err(format!(
                            "closure captures `{}` which is not resolvable in the \
                             enclosing frame (neither a local nor an outer capture)",
                            def_id_name(*def, self.symbols)
                        ));
                    }
                }

                // Cell-promote any captured `let mut` that will be shared
                // by-reference: load the current value into a fresh cell,
                // then rewrite the outer local to hold the cell pointer.
                // From this point on, reads/writes to the outer local go
                // through the cell (see `cell_promoted`).  We only do
                // this once per local — if it's already been promoted by
                // a previous closure in the same function, reuse it.
                // (Recapture sources reuse the enclosing closure's existing
                // cell, so they need no promotion here.)
                for (def, source, _ty, kind) in &slots {
                    let outer_local = match source {
                        CaptureSource::Local(l) => *l,
                        CaptureSource::Recapture(_) => continue,
                    };
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
                            value: MirValue::Use(outer_local),
                        });
                        // Rewrite the outer local to hold the cell pointer.
                        self.emit(MirInst::Assign {
                            dest: outer_local,
                            value: MirValue::Use(cell),
                        });
                        // The local now holds an 8-byte cell POINTER, not the
                        // original value — retype it to `Ty::Int` so codegen
                        // treats it as a 64-bit pointer base for the
                        // `GetField`/`SetField` cell accesses. Without this, a
                        // promoted `Bool` (i8) / `Char` (i32) local keeps its
                        // narrow type and the cell deref lowers to a
                        // `load.i8`/`i32` from a non-pointer-width value
                        // (cranelift verifier: "invalid pointer width"). Only
                        // bites when the capturing closure is a REAL value
                        // (e.g. passed to `Set_each`/`Hash_each`); the inlined
                        // combinator path never promotes, which is why it
                        // surfaced only with Set/Hash `include Enumerable`.
                        if let Some(f) = self.current_fn.as_mut() {
                            if let Some(slot) = f.locals.get_mut(outer_local as usize) {
                                slot.ty = Ty::Int;
                            }
                        }
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
                    // Snapshot (source, kind, def) so the borrow of `slots`
                    // ends before we emit (emit takes `&mut self`).
                    let fill: Vec<(usize, DefId, CaptureSource, CaptureKind)> = slots
                        .iter()
                        .enumerate()
                        .map(|(i, (d, src, _ty, k))| (i, *d, *src, *k))
                        .collect();
                    for (slot_idx, def, source, kind) in fill {
                        let src_val = match (source, kind) {
                            // ── Outer-frame local ──────────────────────
                            (CaptureSource::Local(outer_local), CaptureKind::ByValue) => {
                                // For already-cell-promoted defs, the outer
                                // local is a cell pointer — load the value
                                // out of the cell before storing.  (Niche:
                                // a ByValue capture of a local promoted by an
                                // earlier closure in this frame.)
                                if self.cell_promoted.contains(&def) {
                                    let tmp = self.new_temp(Ty::Int);
                                    self.emit(MirInst::GetField {
                                        dest: tmp,
                                        base: outer_local,
                                        field_index: 0,
                                    });
                                    MirValue::Use(tmp)
                                } else {
                                    MirValue::Use(outer_local)
                                }
                            }
                            (CaptureSource::Local(outer_local), CaptureKind::ByRef) => {
                                // Outer local already holds the cell pointer
                                // (promoted above). Copy the pointer through.
                                MirValue::Use(outer_local)
                            }
                            // ── Re-capture from the enclosing closure ───
                            // We are still lowering inside the enclosing
                            // closure, so `self.captures_ptr_local` is the
                            // ENCLOSING captures pointer. (Q26.)
                            (CaptureSource::Recapture(encl), nested_kind) => {
                                let encl_cap = self.captures_ptr_local.expect(
                                    "re-capture implies the enclosing closure has a captures ptr",
                                );
                                match (encl.kind, nested_kind) {
                                    // Enclosing ByValue → its slot holds the
                                    // value directly; copy it forward.
                                    (CaptureKind::ByValue, _) => {
                                        let tmp = self.new_temp(Ty::Int);
                                        self.emit(MirInst::GetField {
                                            dest: tmp,
                                            base: encl_cap,
                                            field_index: encl.slot_index,
                                        });
                                        MirValue::Use(tmp)
                                    }
                                    // Enclosing ByRef, nested ByRef → share
                                    // the same cell: propagate the cell ptr.
                                    (CaptureKind::ByRef, CaptureKind::ByRef) => {
                                        let cell = self.new_temp(Ty::Int);
                                        self.emit(MirInst::GetField {
                                            dest: cell,
                                            base: encl_cap,
                                            field_index: encl.slot_index,
                                        });
                                        MirValue::Use(cell)
                                    }
                                    // Enclosing ByRef, nested ByValue → read
                                    // the current value through the cell.
                                    (CaptureKind::ByRef, CaptureKind::ByValue) => {
                                        let cell = self.new_temp(Ty::Int);
                                        self.emit(MirInst::GetField {
                                            dest: cell,
                                            base: encl_cap,
                                            field_index: encl.slot_index,
                                        });
                                        let tmp = self.new_temp(Ty::Int);
                                        self.emit(MirInst::GetField {
                                            dest: tmp,
                                            base: cell,
                                            field_index: 0,
                                        });
                                        MirValue::Use(tmp)
                                    }
                                }
                            }
                        };
                        self.emit(MirInst::SetField {
                            base: cap,
                            field_index: slot_idx,
                            value: src_val,
                        });
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
