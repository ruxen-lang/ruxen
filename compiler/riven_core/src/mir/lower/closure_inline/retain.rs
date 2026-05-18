use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn inline_retain(
        &mut self,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<(), String> {
        // read = 0; write = 0
        let read = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: read,
            value: MirValue::Literal(Literal::Int(0)),
        });
        let write = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: write,
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
        let keep_block = self.new_block();
        let inc_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        let cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(read),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(cond),
            then_block: body_block,
            else_block: exit_block,
        });

        // Body: bind item, evaluate predicate.
        self.current_block = body_block;

        let item_local = if let Some(param) = closure_params.first() {
            let item = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item);
            self.emit(MirInst::Call {
                dest: Some(item),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(read)],
            });
            item
        } else {
            self.new_temp(Ty::Int)
        };

        let pred_result = self.lower_expr(closure_body)?;
        let pred_val = local_to_value(pred_result);

        self.set_terminator(Terminator::Branch {
            cond: pred_val,
            then_block: keep_block,
            else_block: inc_block,
        });

        // Keep: write the slot at `write`, then write++.
        self.current_block = keep_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_set".to_string(),
            args: vec![
                MirValue::Use(vec_id),
                MirValue::Use(write),
                MirValue::Use(item_local),
            ],
        });
        let next_write = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_write,
            op: BinOp::Add,
            lhs: MirValue::Use(write),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: write,
            value: MirValue::Use(next_write),
        });
        self.set_terminator(Terminator::Goto(inc_block));

        // Increment read.
        self.current_block = inc_block;
        let next_read = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_read,
            op: BinOp::Add,
            lhs: MirValue::Use(read),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: read,
            value: MirValue::Use(next_read),
        });
        self.set_terminator(Terminator::Goto(header_block));

        // Exit: truncate to `write`.
        self.current_block = exit_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_truncate".to_string(),
            args: vec![MirValue::Use(vec_id), MirValue::Use(write)],
        });
        Ok(())
    }
}
