use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn inline_partition(
        &mut self,
        expr: &HirExpr,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<LocalId, String> {
        // Allocate a tuple (true_vec, false_vec) — 16 bytes, 2 pointers
        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Alloc {
            dest: result,
            ty: expr.ty.clone(),
            size: 16,
        });

        // true_vec = Vec.new()
        let true_vec = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(true_vec),
            callee: "riven_vec_new".to_string(),
            args: vec![],
        });

        // false_vec = Vec.new()
        let false_vec = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(false_vec),
            callee: "riven_vec_new".to_string(),
            args: vec![],
        });

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
        let true_block = self.new_block();
        let false_block = self.new_block();
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

        let pred_result = self.lower_expr(closure_body)?;
        let pred_val = local_to_value(pred_result);

        self.set_terminator(Terminator::Branch {
            cond: pred_val,
            then_block: true_block,
            else_block: false_block,
        });

        // True block: true_vec.push(item)
        self.current_block = true_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_push".to_string(),
            args: vec![MirValue::Use(true_vec), MirValue::Use(item_local)],
        });
        self.set_terminator(Terminator::Goto(inc_block));

        // False block: false_vec.push(item)
        self.current_block = false_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "riven_vec_push".to_string(),
            args: vec![MirValue::Use(false_vec), MirValue::Use(item_local)],
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

        // Exit: store true_vec and false_vec into the result tuple
        self.current_block = exit_block;
        self.emit(MirInst::SetField {
            base: result,
            field_index: 0,
            value: MirValue::Use(true_vec),
        });
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: MirValue::Use(false_vec),
        });
        Ok(result)
    }
}
