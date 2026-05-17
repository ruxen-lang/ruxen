use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn inline_result_map(
        &mut self,
        expr: &HirExpr,
        result_expr: &HirExpr,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
        on_ok: bool,
    ) -> Result<Option<Option<LocalId>>, String> {
        let res_local = self.lower_expr(result_expr)?;
        let res_id = res_local.unwrap_or_else(|| self.new_temp(Ty::Int));

        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Alloc {
            dest: result,
            ty: expr.ty.clone(),
            size: 16,
        });

        let tag = self.new_temp(Ty::Int32);
        self.emit(MirInst::GetTag {
            dest: tag,
            src: res_id,
        });

        // Result: Ok=0, Err=1.
        let match_tag = if on_ok { 0 } else { 1 };
        let match_block = self.new_block();
        let other_block = self.new_block();
        let merge_block = self.new_block();

        let take_branch = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: take_branch,
            op: CmpOp::Eq,
            lhs: MirValue::Use(tag),
            rhs: MirValue::Literal(Literal::Int(match_tag)),
        });
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(take_branch),
            then_block: match_block,
            else_block: other_block,
        });

        // Matching arm: payload → closure → repackage with same tag.
        self.current_block = match_block;
        let payload = self.new_temp(Ty::Int);
        self.emit(MirInst::GetField {
            dest: payload,
            base: res_id,
            field_index: 1,
        });

        if let Some(param) = closure_params.first() {
            let param_ty = if matches!(param.ty, Ty::Infer(_)) {
                match &result_expr.ty {
                    Ty::Result(ok, err) => {
                        if on_ok {
                            ok.as_ref().clone()
                        } else {
                            err.as_ref().clone()
                        }
                    }
                    _ => param.ty.clone(),
                }
            } else {
                param.ty.clone()
            };
            let param_local = self.new_local_named(&param.name, param_ty, false);
            self.def_to_local.insert(param.def_id, param_local);
            self.emit(MirInst::Assign {
                dest: param_local,
                value: MirValue::Use(payload),
            });
        }

        let mapped = self.lower_expr(closure_body)?;
        let mapped_val = local_to_value(mapped);

        self.emit(MirInst::SetTag {
            dest: result,
            tag: match_tag as u32,
        });
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: mapped_val,
        });
        self.set_terminator(Terminator::Goto(merge_block));

        // Other arm: passthrough — same tag, same payload.
        self.current_block = other_block;
        let other_payload = self.new_temp(Ty::Int);
        self.emit(MirInst::GetField {
            dest: other_payload,
            base: res_id,
            field_index: 1,
        });
        let other_tag = if on_ok { 1 } else { 0 };
        self.emit(MirInst::SetTag {
            dest: result,
            tag: other_tag as u32,
        });
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: MirValue::Use(other_payload),
        });
        self.set_terminator(Terminator::Goto(merge_block));

        self.current_block = merge_block;
        Ok(Some(Some(result)))
    }
}
