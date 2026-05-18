use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn inline_unwrap_or_else(
        &mut self,
        expr: &HirExpr,
        receiver_expr: &HirExpr,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
        ok_tag: i64,
    ) -> Result<Option<Option<LocalId>>, String> {
        let recv_local = self.lower_expr(receiver_expr)?;
        let recv_id = recv_local.unwrap_or_else(|| self.new_temp(Ty::Int));

        let result = self.new_temp(expr.ty.clone());

        let tag = self.new_temp(Ty::Int32);
        self.emit(MirInst::GetTag {
            dest: tag,
            src: recv_id,
        });

        let is_ok = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: is_ok,
            op: CmpOp::Eq,
            lhs: MirValue::Use(tag),
            rhs: MirValue::Literal(Literal::Int(ok_tag)),
        });

        let ok_block = self.new_block();
        let err_block = self.new_block();
        let merge_block = self.new_block();

        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(is_ok),
            then_block: ok_block,
            else_block: err_block,
        });

        // Success arm: result = payload.
        self.current_block = ok_block;
        let ok_payload = self.new_temp(expr.ty.clone());
        self.emit(MirInst::GetField {
            dest: ok_payload,
            base: recv_id,
            field_index: 1,
        });
        self.emit(MirInst::Assign {
            dest: result,
            value: MirValue::Use(ok_payload),
        });
        self.set_terminator(Terminator::Goto(merge_block));

        // Error arm: bind closure param to err payload, run body.
        self.current_block = err_block;
        if let Some(param) = closure_params.first() {
            let err_payload = self.new_temp(Ty::Int);
            self.emit(MirInst::GetField {
                dest: err_payload,
                base: recv_id,
                field_index: 1,
            });
            let param_ty = if matches!(param.ty, Ty::Infer(_)) {
                match &receiver_expr.ty {
                    Ty::Result(_, err) => err.as_ref().clone(),
                    _ => param.ty.clone(),
                }
            } else {
                param.ty.clone()
            };
            let param_local = self.new_local_named(&param.name, param_ty, false);
            self.def_to_local.insert(param.def_id, param_local);
            self.emit(MirInst::Assign {
                dest: param_local,
                value: MirValue::Use(err_payload),
            });
        }
        let body_val = self.lower_expr(closure_body)?;
        // If the closure body produces a value, use it as the result;
        // otherwise leave `result` uninitialised (Unit-typed call sites).
        if let Some(v) = body_val {
            self.emit(MirInst::Assign {
                dest: result,
                value: MirValue::Use(v),
            });
        }
        self.set_terminator(Terminator::Goto(merge_block));

        self.current_block = merge_block;
        Ok(Some(Some(result)))
    }

}
