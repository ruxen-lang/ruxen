use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_for_loop(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── For loop ────────────────────────────────────────────
            HirExprKind::For {
                binding,
                binding_name,
                iterable,
                body,
                tuple_bindings,
            } => {
                // Special case: `for i in start..end` (and `start..=end`).
                // Desugar to a counter-based while loop: evaluate `start`
                // and `end` once each into hidden temporaries, then loop
                // while `i < end` (or `i <= end` for inclusive) and
                // increment by one at the end of each iteration.
                if let HirExprKind::Range {
                    start,
                    end,
                    inclusive,
                } = &iterable.kind
                {
                    let start_expr = start
                        .as_ref()
                        .ok_or_else(|| "for-range requires a start bound".to_string())?;
                    let end_expr = end
                        .as_ref()
                        .ok_or_else(|| "for-range requires an end bound".to_string())?;

                    // Evaluate start and end exactly once.
                    let start_local = self.lower_expr(start_expr)?;
                    let start_val = local_to_value(start_local);
                    let end_local = self.lower_expr(end_expr)?;
                    let end_val = local_to_value(end_local);

                    // Stash end in a hidden temp so we re-use it each header
                    // iteration without re-evaluating the expression.
                    let end_tmp = self.new_temp(Ty::Int);
                    self.emit(MirInst::Assign {
                        dest: end_tmp,
                        value: end_val,
                    });

                    // Create the user-visible loop binding `i` as a mutable
                    // Int local and initialise it with `start`.
                    let binding_local = {
                        let func = self.fn_mut();
                        func.new_local(binding_name.clone(), Ty::Int, true)
                    };
                    self.def_to_local.insert(*binding, binding_local);
                    self.emit(MirInst::Assign {
                        dest: binding_local,
                        value: start_val,
                    });

                    // Blocks: header (cond check), body, step (increment +
                    // back-edge, also the `continue` target), exit.
                    let header_block = self.new_block();
                    let body_block = self.new_block();
                    let step_block = self.new_block();
                    let exit_block = self.new_block();

                    self.set_terminator(Terminator::Goto(header_block));

                    // Header: cond = i < end_tmp (exclusive) or i <= end_tmp.
                    self.current_block = header_block;
                    let cond = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: cond,
                        op: if *inclusive { CmpOp::LtEq } else { CmpOp::Lt },
                        lhs: MirValue::Use(binding_local),
                        rhs: MirValue::Use(end_tmp),
                    });
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(cond),
                        then_block: body_block,
                        else_block: exit_block,
                    });

                    // Body. `continue` jumps to `step_block` so the counter
                    // is still incremented; `break` jumps to `exit_block`.
                    self.current_block = body_block;
                    self.loop_stack.push(LoopFrame {
                        continue_target: step_block,
                        break_target: exit_block,
                        result_local: None,
                        body_entry_block: body_block,
                        body_locals: Vec::new(),
                    });
                    let _ = self.lower_expr(body)?;
                    let frame = self.loop_stack.pop().expect("loop frame");
                    if matches!(self.get_terminator(), Terminator::Unreachable) {
                        self.emit_dealloc_loop_locals(&frame.body_locals);
                        self.set_terminator(Terminator::Goto(step_block));
                    }
                    self.prepend_zero_init_for_body_locals(&frame);

                    // Step: i = i + 1, then jump back to header.
                    self.current_block = step_block;
                    let next = self.new_temp(Ty::Int);
                    self.emit(MirInst::BinOp {
                        dest: next,
                        op: BinOp::Add,
                        lhs: MirValue::Use(binding_local),
                        rhs: MirValue::Literal(Literal::Int(1)),
                    });
                    self.emit(MirInst::Assign {
                        dest: binding_local,
                        value: MirValue::Use(next),
                    });
                    self.set_terminator(Terminator::Goto(header_block));

                    self.current_block = exit_block;
                    return Ok(None);
                }

                // Fallback: iterate over a Vec-like collection.
                //
                // Lower iterable expression (after iterator no-ops, this
                // is typically a Vec pointer).
                let iter_local = self.lower_expr(iterable)?;
                let iter_id = iter_local.unwrap_or_else(|| self.new_temp(Ty::Int));

                // Index counter: _i = 0
                let idx = self.new_temp(Ty::Int);
                self.emit(MirInst::Assign {
                    dest: idx,
                    value: MirValue::Literal(Literal::Int(0)),
                });

                // Length of the collection.
                // All iterator types (VecIter, VecIntoIter, etc.) are
                // runtime no-ops that pass through the underlying Vec
                // pointer, so we always call Vec runtime ops directly.
                let len = self.new_temp(Ty::Int);
                self.emit(MirInst::Call {
                    dest: Some(len),
                    callee: "ruxen_vec_len".to_string(),
                    args: vec![MirValue::Use(iter_id)],
                });

                // Create blocks: header, body, step, exit
                let header_block = self.fn_mut().new_block();
                let body_block = self.fn_mut().new_block();
                let step_block = self.fn_mut().new_block();
                let exit_block = self.fn_mut().new_block();

                // Jump to header from current block
                self.set_terminator(Terminator::Goto(header_block));
                self.current_block = header_block;

                // Header: cond = idx < len
                let cond = self.new_temp(Ty::Bool);
                self.emit(MirInst::Compare {
                    dest: cond,
                    op: CmpOp::Lt,
                    lhs: MirValue::Use(idx),
                    rhs: MirValue::Use(len),
                });
                self.set_terminator(Terminator::Branch {
                    cond: MirValue::Use(cond),
                    then_block: body_block,
                    else_block: exit_block,
                });

                // Body: binding = vec_get(iter_id, idx)
                self.current_block = body_block;

                // Create the binding variable.
                // Determine element type from the iterable's type.
                let binding_ty = element_type_of(&iterable.ty);
                let binding_local = {
                    let func = self.fn_mut();

                    func.new_local(binding_name.clone(), binding_ty, false)
                };
                self.def_to_local.insert(*binding, binding_local);

                self.emit(MirInst::Call {
                    dest: Some(binding_local),
                    callee: "ruxen_vec_get".to_string(),
                    args: vec![MirValue::Use(iter_id), MirValue::Use(idx)],
                });

                // For tuple destructuring patterns like (i, result) from
                // .enumerate(), wire up the sub-bindings.
                if !tuple_bindings.is_empty() {
                    for (tb_idx, (tb_def_id, tb_name)) in tuple_bindings.iter().enumerate() {
                        let tb_local = {
                            let func = self.fn_mut();
                            func.new_local(tb_name.clone(), Ty::Int, false)
                        };
                        self.def_to_local.insert(*tb_def_id, tb_local);

                        if tb_idx == 0 {
                            // First element of enumerate tuple = index
                            self.emit(MirInst::Assign {
                                dest: tb_local,
                                value: MirValue::Use(idx),
                            });
                        } else {
                            // Second element = the actual Vec element
                            self.emit(MirInst::Assign {
                                dest: tb_local,
                                value: MirValue::Use(binding_local),
                            });
                        }
                    }
                }

                // Lower body. `continue` jumps to `step_block` so the
                // index is still incremented; `break` jumps to `exit_block`.
                self.loop_stack.push(LoopFrame {
                    continue_target: step_block,
                    break_target: exit_block,
                    result_local: None,
                    body_entry_block: body_block,
                    body_locals: Vec::new(),
                });
                self.lower_expr(body)?;
                let frame = self.loop_stack.pop().expect("loop frame");

                if matches!(self.get_terminator(), Terminator::Unreachable) {
                    self.emit_dealloc_loop_locals(&frame.body_locals);
                    self.set_terminator(Terminator::Goto(step_block));
                }
                self.prepend_zero_init_for_body_locals(&frame);

                // Step: increment index and jump back to header.
                self.current_block = step_block;
                let next_idx = self.new_temp(Ty::Int);
                self.emit(MirInst::BinOp {
                    dest: next_idx,
                    op: BinOp::Add,
                    lhs: MirValue::Use(idx),
                    rhs: MirValue::Literal(Literal::Int(1)),
                });
                self.emit(MirInst::Assign {
                    dest: idx,
                    value: MirValue::Use(next_idx),
                });

                // Jump back to header
                self.set_terminator(Terminator::Goto(header_block));

                // Continue in exit block
                self.current_block = exit_block;

                Ok(None)
            }

            // ── Closure ─────────────────────────────────────────────
            _ => unreachable!("lower_for_loop: dispatched to wrong helper"),
        }
    }
}
