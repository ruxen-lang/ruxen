use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_control(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── If / else ───────────────────────────────────────────
            HirExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_local = self.lower_expr(cond)?;
                let cond_val = local_to_value(cond_local);

                let then_block = self.new_block();
                let else_block = self.new_block();
                let merge_block = self.new_block();

                self.set_terminator(Terminator::Branch {
                    cond: cond_val,
                    then_block,
                    else_block,
                });

                // Then branch
                self.current_block = then_block;
                let then_result = self.lower_expr(then_branch)?;
                let then_exit_block = self.current_block;

                // Else branch
                self.current_block = else_block;
                let else_result = if let Some(else_expr) = else_branch {
                    self.lower_expr(else_expr)?
                } else {
                    None
                };
                let else_exit_block = self.current_block;

                // If the expression has a non-unit type, create a phi-like merge.
                let result = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                    let result_local = self.new_temp(expr.ty.clone());

                    // Assign from then-branch
                    self.current_block = then_exit_block;
                    if matches!(self.get_terminator(), Terminator::Unreachable) {
                        let val = local_to_value(then_result);
                        self.emit(MirInst::Assign {
                            dest: result_local,
                            value: val,
                        });
                        self.set_terminator(Terminator::Goto(merge_block));
                    }

                    // Assign from else-branch
                    self.current_block = else_exit_block;
                    if matches!(self.get_terminator(), Terminator::Unreachable) {
                        let val = local_to_value(else_result);
                        self.emit(MirInst::Assign {
                            dest: result_local,
                            value: val,
                        });
                        self.set_terminator(Terminator::Goto(merge_block));
                    }

                    Some(result_local)
                } else {
                    // Unit-typed: just jump to merge.
                    self.current_block = then_exit_block;
                    if matches!(self.get_terminator(), Terminator::Unreachable) {
                        self.set_terminator(Terminator::Goto(merge_block));
                    }
                    self.current_block = else_exit_block;
                    if matches!(self.get_terminator(), Terminator::Unreachable) {
                        self.set_terminator(Terminator::Goto(merge_block));
                    }
                    None
                };

                self.current_block = merge_block;
                Ok(result)
            }

            // ── While loop ──────────────────────────────────────────
            HirExprKind::While { condition, body } => {
                let header_block = self.new_block();
                let body_block = self.new_block();
                let exit_block = self.new_block();

                // Jump from current block to header.
                self.set_terminator(Terminator::Goto(header_block));

                // Header: evaluate condition, branch.
                self.current_block = header_block;
                let cond_local = self.lower_expr(condition)?;
                let cond_val = local_to_value(cond_local);
                self.set_terminator(Terminator::Branch {
                    cond: cond_val,
                    then_block: body_block,
                    else_block: exit_block,
                });

                // Body: execute, then jump back to header.
                // `continue` inside the body jumps to the header (re-check
                // the condition); `break` jumps to the exit block.
                self.current_block = body_block;
                self.loop_stack.push(LoopFrame {
                    continue_target: header_block,
                    break_target: exit_block,
                    result_local: None,
                    body_entry_block: body_block,
                    body_locals: Vec::new(),
                });
                let _ = self.lower_expr(body)?;
                let frame = self.loop_stack.pop().expect("loop frame");
                if matches!(self.get_terminator(), Terminator::Unreachable) {
                    self.emit_dealloc_loop_locals(&frame.body_locals);
                    self.set_terminator(Terminator::Goto(header_block));
                }
                self.prepend_zero_init_for_body_locals(&frame);

                self.current_block = exit_block;
                Ok(None) // while loops produce Unit
            }

            // ── Loop (infinite) ─────────────────────────────────────
            HirExprKind::Loop { body } => {
                let loop_block = self.new_block();
                let exit_block = self.new_block();

                // If the loop expression yields a value (via `break VALUE`),
                // allocate a result local that every `break` writes into
                // before jumping to the exit block.
                let result_local = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
                    Some(self.new_temp(expr.ty.clone()))
                } else {
                    None
                };

                self.set_terminator(Terminator::Goto(loop_block));

                self.current_block = loop_block;
                self.loop_stack.push(LoopFrame {
                    continue_target: loop_block,
                    break_target: exit_block,
                    result_local,
                    body_entry_block: loop_block,
                    body_locals: Vec::new(),
                });
                let _ = self.lower_expr(body)?;
                let frame = self.loop_stack.pop().expect("loop frame");
                if matches!(self.get_terminator(), Terminator::Unreachable) {
                    self.emit_dealloc_loop_locals(&frame.body_locals);
                    self.set_terminator(Terminator::Goto(loop_block));
                }
                self.prepend_zero_init_for_body_locals(&frame);

                // exit_block is only reachable via break (which we handle below)
                self.current_block = exit_block;
                Ok(result_local)
            }

            // ── Return ──────────────────────────────────────────────
            HirExprKind::Return(value) => {
                let val = if let Some(expr) = value {
                    let local = self.lower_expr(expr)?;
                    Some(local_to_value(local))
                } else {
                    None
                };
                self.set_terminator(Terminator::Return(val));
                // Create a dead block for any code after the return.
                let dead = self.new_block();
                self.current_block = dead;
                Ok(None)
            }

            // ── Function call ───────────────────────────────────────

            // ── Break / Continue ────────────────────────────────────
            HirExprKind::Break(value) => {
                // Look up the innermost loop. If there is no enclosing
                // loop, treat as a no-op (earlier passes should reject).
                if let Some(frame) = self.loop_stack.last().cloned() {
                    // If a value is provided, lower it and assign into
                    // the loop's result local so the loop expression
                    // evaluates to that value at the exit block.
                    if let Some(val_expr) = value {
                        let val_local = self.lower_expr(val_expr)?;
                        if let Some(dest) = frame.result_local {
                            self.emit(MirInst::Assign {
                                dest,
                                value: local_to_value(val_local),
                            });
                        }
                    }
                    // Free heap-owned locals declared in the loop body
                    // before exiting. (P0.2)
                    self.emit_dealloc_loop_locals(&frame.body_locals);
                    self.set_terminator(Terminator::Goto(frame.break_target));
                    // Any code after `break` in this source block is
                    // unreachable — lower it into a fresh dead block so
                    // subsequent emits don't clobber the terminator we
                    // just set.
                    let dead = self.new_block();
                    self.current_block = dead;
                }
                Ok(None)
            }
            HirExprKind::Continue => {
                if let Some(frame) = self.loop_stack.last().cloned() {
                    self.emit_dealloc_loop_locals(&frame.body_locals);
                    self.set_terminator(Terminator::Goto(frame.continue_target));
                    let dead = self.new_block();
                    self.current_block = dead;
                }
                Ok(None)
            }

            // ── For loop ────────────────────────────────────────────
            _ => unreachable!("lower_control: dispatched to wrong helper"),
        }
    }
}
