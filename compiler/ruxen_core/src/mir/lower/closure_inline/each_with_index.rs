use super::super::*;

impl<'a> Lowerer<'a> {
    /// Emit an inlined `vec.each_with_index { |item, idx| <body> }` — the
    /// Ruby two-arg form of `each`. Identical per-element loop to
    /// `inline_each`, but the closure's SECOND parameter is bound to the
    /// running 0-based index (an `Int`), matching Ruby's
    /// `Enumerable#each_with_index` yield order `|element, index|`.
    pub(super) fn inline_each_with_index(
        &mut self,
        vec_id: LocalId,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<(), String> {
        // idx = 0
        let idx = self.new_temp(Ty::Int);
        self.emit(MirInst::Assign {
            dest: idx,
            value: MirValue::Literal(Literal::Int(0)),
        });

        // len = ruxen_vec_len(vec)
        let len = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(len),
            callee: "ruxen_vec_len".to_string(),
            args: vec![MirValue::Use(vec_id)],
        });

        let header_block = self.new_block();
        let body_block = self.new_block();
        let exit_block = self.new_block();

        self.set_terminator(Terminator::Goto(header_block));
        self.current_block = header_block;

        // cond = idx < len
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

        // Bind param 0 (element): item = vec_get(vec, idx)
        if let Some(param) = closure_params.first() {
            let item_local = self.new_local_named(&param.name, param.ty.clone(), false);
            self.def_to_local.insert(param.def_id, item_local);
            self.emit(MirInst::Call {
                dest: Some(item_local),
                callee: "ruxen_vec_get".to_string(),
                args: vec![MirValue::Use(vec_id), MirValue::Use(idx)],
            });
        }

        // Bind param 1 (index): i = idx
        if let Some(param) = closure_params.get(1) {
            let index_local = self.new_local_named(&param.name, Ty::Int, false);
            self.def_to_local.insert(param.def_id, index_local);
            self.emit(MirInst::Assign {
                dest: index_local,
                value: MirValue::Use(idx),
            });
        }

        // Lower the closure body
        let _ = self.lower_expr(closure_body)?;

        // idx = idx + 1
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
        Ok(())
    }
}
