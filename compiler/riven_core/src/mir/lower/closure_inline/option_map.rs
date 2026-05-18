use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn inline_option_map(
        &mut self,
        expr: &HirExpr,
        option_expr: &HirExpr,
        closure_params: &[HirClosureParam],
        closure_body: &HirExpr,
    ) -> Result<Option<Option<LocalId>>, String> {
        let opt_local = self.lower_expr(option_expr)?;
        let opt_id = opt_local.unwrap_or_else(|| self.new_temp(Ty::Int));

        // Allocate the result Option (16 bytes: tag + payload)
        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Alloc {
            dest: result,
            ty: expr.ty.clone(),
            size: 16,
        });

        // Get the tag of the input Option
        let tag = self.new_temp(Ty::Int32);
        self.emit(MirInst::GetTag {
            dest: tag,
            src: opt_id,
        });

        // Check if Some (tag == 1)
        let is_some = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: is_some,
            op: CmpOp::Eq,
            lhs: MirValue::Use(tag),
            rhs: MirValue::Literal(Literal::Int(1)),
        });

        let some_block = self.new_block();
        let none_block = self.new_block();
        let merge_block = self.new_block();

        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(is_some),
            then_block: some_block,
            else_block: none_block,
        });

        // Some block: extract payload, apply closure, wrap in new Some
        self.current_block = some_block;

        // Get the payload from the input Option
        let payload = self.new_temp(Ty::Int);
        self.emit(MirInst::GetField {
            dest: payload,
            base: opt_id,
            field_index: 1, // payload is at offset 8
        });

        // Bind the closure parameter to the payload.
        // If the parameter type is Infer, refine it using the inner type
        // of the Option being mapped, so that string interpolation and
        // other type-sensitive lowering works correctly.
        if let Some(param) = closure_params.first() {
            let param_ty = if matches!(param.ty, Ty::Infer(_)) {
                match &option_expr.ty {
                    Ty::Option(inner) => inner.as_ref().clone(),
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

        // Evaluate the closure body to get the transformed value
        let mapped_result = self.lower_expr(closure_body)?;
        let mapped_val = local_to_value(mapped_result);

        // Set result to Some(mapped_value)
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 1,
        });
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: mapped_val,
        });
        self.set_terminator(Terminator::Goto(merge_block));

        // None block: set result to None
        self.current_block = none_block;
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 0,
        });
        self.set_terminator(Terminator::Goto(merge_block));

        self.current_block = merge_block;
        Ok(Some(Some(result)))
    }
}
