use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn inline_find(
        &mut self,
        expr: &HirExpr,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<LocalId, String> {
        // Allocate result as Option (tagged union: 16 bytes)
        // tag=0 -> None, tag=1 -> Some(payload)
        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Alloc {
            dest: result,
            ty: expr.ty.clone(),
            size: 16,
        });
        // Initialize to None (tag=0)
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 0,
        });

        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "ruxen_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let found_block = self.new_block();
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

        // Body
        self.current_block = body_block;

        let item_local = if let Some(param) = closure_params.first() {
            let item = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item);
            self.emit(MirInst::Call {
                dest: Some(item),
                callee: "ruxen_vec_get".to_string(),
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
            then_block: found_block,
            else_block: inc_block,
        });

        // Found: set result to Some(item)
        self.current_block = found_block;
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 1,
        });
        // Store item as payload (offset 8 from base)
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: MirValue::Use(item_local),
        });
        self.set_terminator(Terminator::Goto(exit_block));

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
