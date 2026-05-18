use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn inline_fold(
        &mut self,
        expr: &HirExpr,
        vec_id: LocalId,
        args: &[HirExpr],
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<LocalId, String> {
        // Lower `init` first; that yields the seed value for the accumulator.
        let init_arg = args
            .first()
            .ok_or_else(|| "fold requires an init argument".to_string())?;
        let init_local = self
            .lower_expr(init_arg)?
            .ok_or_else(|| "fold init argument has no value".to_string())?;

        // The accumulator local takes its name from the closure's first
        // parameter so the closure body's `acc` reference resolves to it.
        // Without a named local, the body's reference would have nowhere
        // to bind. Type comes from the closure-param annotation if
        // present, else falls back to `expr.ty` (the fold result type).
        let acc_ty = closure_params
            .first()
            .map(|p| p.ty.clone())
            .unwrap_or_else(|| expr.ty.clone());
        let acc_local = if let Some(param) = closure_params.first() {
            let l = self.new_local_named(&param.name, acc_ty.clone(), true);
            self.def_to_local.insert(param.def_id, l);
            l
        } else {
            self.new_temp(acc_ty.clone())
        };
        // Seed accumulator with init.
        self.emit_transfer(acc_local, init_local, &acc_ty, MoveSemantics::Copy);

        // Loop counters.
        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });
        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

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

        // Body: bind the per-iteration item (closure param 1), invoke
        // the closure body, store the result back into `acc`.
        self.current_block = body_block;
        if let Some(param) = closure_params.get(1) {
            let item_local = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item_local);
            self.emit(MirInst::Call {
                dest: Some(item_local),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
        }
        let body_result = self.lower_expr(closure_body)?;
        if let Some(body_id) = body_result {
            // acc = closure_body(acc, item)
            self.emit(MirInst::Assign {
                dest: acc_local,
                value: MirValue::Use(body_id),
            });
        }

        // idx += 1; back to header.
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
        if matches!(self.get_terminator(), Terminator::Unreachable) {
            self.set_terminator(Terminator::Goto(header_block));
        }

        self.current_block = exit_block;
        Ok(acc_local)
    }
}
