use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn inline_sort_by(
        &mut self,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<(), String> {
        // i = 0
        let i_idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: i_idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        // len = ruxen_vec_len(vec)
        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "ruxen_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let outer_header = self.new_block();
        let outer_body = self.new_block();
        let inner_header = self.new_block();
        let inner_body = self.new_block();
        let swap_block = self.new_block();
        let inner_inc = self.new_block();
        let outer_inc = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(outer_header));
        self.current_block = outer_header;

        // outer cond: i < len
        let outer_cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: outer_cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(i_idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(outer_cond),
            then_block: outer_body,
            else_block: exit_block,
        });

        // outer body: j = i + 1
        self.current_block = outer_body;
        let j_idx = self.new_temp(Ty::Int);
        let i_plus_1 = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: i_plus_1,
            op: BinOp::Add,
            lhs: MirValue::Use(i_idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: j_idx,
            value: MirValue::Use(i_plus_1),
        });
        self.set_terminator(Terminator::Goto(inner_header));

        // inner cond: j < len
        self.current_block = inner_header;
        let inner_cond = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: inner_cond,
            op: CmpOp::Lt,
            lhs: MirValue::Use(j_idx),
            rhs: MirValue::Use(len),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(inner_cond),
            then_block: inner_body,
            else_block: outer_inc,
        });

        // inner body: bind closure params a = vec[i], b = vec[j];
        // result = closure(a, b); if result > 0: swap(i, j).
        self.current_block = inner_body;
        let elem_ty = element_type_of(&self.fn_local_ty(vec_id));
        let a_local = if let Some(param) = closure_params.first() {
            let l = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, l);
            self.emit(MirInst::Call {
                dest: Some(l),
                callee: "ruxen_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(i_idx)],
            });
            l
        } else {
            let l = self.new_temp(elem_ty.clone());
            self.emit(MirInst::Call {
                dest: Some(l),
                callee: "ruxen_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(i_idx)],
            });
            l
        };
        let _b_local = if let Some(param) = closure_params.get(1) {
            let l = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, l);
            self.emit(MirInst::Call {
                dest: Some(l),
                callee: "ruxen_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(j_idx)],
            });
            l
        } else {
            let l = self.new_temp(elem_ty.clone());
            self.emit(MirInst::Call {
                dest: Some(l),
                callee: "ruxen_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(j_idx)],
            });
            l
        };
        let _ = a_local;

        let cmp_result = self.lower_expr(closure_body)?;
        let cmp_val = local_to_value(cmp_result);
        let zero = MirValue::Literal(Literal::Int(0));
        let need_swap = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: need_swap,
            op: CmpOp::Gt,
            lhs: cmp_val,
            rhs: zero,
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(need_swap),
            then_block: swap_block,
            else_block: inner_inc,
        });

        // swap_block: ruxen_vec_swap(vec, i, j)
        self.current_block = swap_block;
        self.emit(MirInst::Call {
            dest: None,
            callee: "ruxen_vec_swap".to_string(),
            args: vec![
                MirValue::Use(vec_id),
                MirValue::Use(i_idx),
                MirValue::Use(j_idx),
            ],
        });
        self.set_terminator(Terminator::Goto(inner_inc));

        // inner_inc: j += 1
        self.current_block = inner_inc;
        let next_j = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_j,
            op: BinOp::Add,
            lhs: MirValue::Use(j_idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: j_idx,
            value: MirValue::Use(next_j),
        });
        self.set_terminator(Terminator::Goto(inner_header));

        // outer_inc: i += 1
        self.current_block = outer_inc;
        let next_i = self.new_temp(Ty::Int);
        self.emit(MirInst::BinOp {
            dest: next_i,
            op: BinOp::Add,
            lhs: MirValue::Use(i_idx),
            rhs: MirValue::Literal(Literal::Int(1)),
        });
        self.emit(MirInst::Assign {
            dest: i_idx,
            value: MirValue::Use(next_i),
        });
        self.set_terminator(Terminator::Goto(outer_header));

        self.current_block = exit_block;
        Ok(())
    }
}
