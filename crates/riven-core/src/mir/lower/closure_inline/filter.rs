use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn inline_filter(
        &mut self,
        expr: &HirExpr,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<LocalId, String> {
        // result = riven_vec_new()
        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Call {
            dest: Some(result),
            callee: "riven_vec_new".to_string(),
            args: vec![],
        });

        // idx = 0
        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        // len = riven_vec_len(vec)
        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "riven_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let push_block = self.new_block();
        let inc_block = self.new_block();
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

        // Body: bind item, evaluate predicate
        self.current_block = body_block;

        let item_local = if let Some(param) = closure_params.first() {
            let item = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item);
            self.emit(MirInst::Call {
                dest: Some(item),
                callee: "riven_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
            item
        } else {
            self.new_temp(Ty::Int)
        };

        // Evaluate predicate
        let pred_result = self.lower_expr(closure_body)?;
        let pred_val = local_to_value(pred_result);

        self.set_terminator(Terminator::Branch {
            cond: pred_val,
            then_block: push_block,
            else_block: inc_block,
        });

        // Push block: result.push(item)
        self.current_block = push_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_push".to_string(),
            args: vec![MirValue::Use(result), MirValue::Use(item_local)],
        });
        self.set_terminator(Terminator::Goto(inc_block));

        // Increment
        self.current_block = inc_block;
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
        self.set_terminator(Terminator::Goto(header_block));

        self.current_block = exit_block;
        Ok(result)
    }
}
