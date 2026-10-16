use super::super::*;

/// The `DefId` the present value of a safe-navigation receiver is bound to
/// while its member lowers. The resolver allocates ids upward from 0, so it
/// never hands this one out; `UNRESOLVED_DEF` is the only id above it.
const SAFE_NAV_RECEIVER: DefId = UNRESOLVED_DEF - 1;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_field_access(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        let HirExprKind::FieldAccess {
            object,
            field_name,
            field_idx,
            safe_nav,
        } = &expr.kind
        else {
            unreachable!("lower_field_access: dispatched to wrong helper")
        };
        if *safe_nav && is_option_type(&object.ty) {
            return self.lower_safe_nav(expr);
        }

        // Typeck rewrites every member access that names no field into a
        // `MethodCall`. One whose receiver type was still unknown during
        // typeck reaches here; decide it now and lower a method through the
        // one method-call lowering.
        let receiver_ty = self.mir_refined_expr_ty(object);
        let receiver_name = self
            .receiver_type_name(object)
            .unwrap_or_else(|| type_name_from_ty(&receiver_ty));
        let is_field = match receiver_ty.peel_refs() {
            Ty::Class { name, .. } | Ty::Struct { name, .. } => {
                self.is_real_field(name, field_name)
            }
            // The typechecker has already validated a tuple index.
            Ty::Tuple(_) => field_name.parse::<usize>().is_ok(),
            // Newtype wrappers expose the inner value via `.0`.
            Ty::Newtype { .. } => field_name == "0",
            _ => false,
        };
        if !is_field && !receiver_name.is_empty() {
            let call = HirExpr {
                kind: HirExprKind::MethodCall {
                    object: object.clone(),
                    method: UNRESOLVED_DEF,
                    resolved: UNRESOLVED_DEF,
                    method_name: field_name.clone(),
                    generic_args: Vec::new(),
                    args: Vec::new(),
                    block: None,
                    safe_nav: false,
                    parens: false,
                },
                ty: expr.ty.clone(),
                span: expr.span.clone(),
            };
            return self.lower_method_call(&call);
        }

        let Some(base) = self.lower_expr(object)? else {
            return Ok(None);
        };
        let dest = self.new_temp(expr.ty.clone());
        // Phase B-4: shift field index past class_info_ptr header for
        // runtime-dispatch classes. Returns 0 for structs / static-only-mixin
        // classes.
        let shift = self.class_field_shift_for_ty(&object.ty);
        self.emit(MirInst::GetField {
            dest,
            base,
            field_index: *field_idx + shift,
        });
        Ok(Some(dest))
    }

    /// `x&.member` (a field access or a method call with `safe_nav` set) on
    /// a `T?` receiver: branch on the receiver's tag, lower the member on the
    /// present value and wrap its result into `T?`; an absent receiver gives
    /// an absent result without evaluating the member or its arguments.
    pub(super) fn lower_safe_nav(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        let mut member = expr.clone();
        let (object, safe_nav) = match &mut member.kind {
            HirExprKind::FieldAccess {
                object, safe_nav, ..
            }
            | HirExprKind::MethodCall {
                object, safe_nav, ..
            } => (object, safe_nav),
            _ => unreachable!("lower_safe_nav: not a member access"),
        };
        *safe_nav = false;
        let present_ty = match object.ty.builtin_head() {
            Some(("Option", args)) => args.into_iter().next().unwrap_or(Ty::Int),
            _ => unreachable!("lower_safe_nav: receiver is not optional"),
        };
        let span = object.span.clone();
        let receiver = std::mem::replace(
            object,
            Box::new(HirExpr {
                kind: HirExprKind::VarRef(SAFE_NAV_RECEIVER),
                ty: present_ty,
                span,
            }),
        );
        member.ty = match &expr.ty {
            Ty::Option(inner) => inner.as_ref().clone(),
            _ => Ty::Int,
        };

        let opt_local = self.lower_expr(&receiver)?;
        let opt_id = opt_local.unwrap_or_else(|| self.new_temp(Ty::Int));

        let result = self.new_temp(expr.ty.clone());
        self.emit(MirInst::Alloc {
            dest: result,
            ty: expr.ty.clone(),
            size: 16,
        });
        let tag = self.new_temp(Ty::Int32);
        self.emit(MirInst::GetTag {
            dest: tag,
            src: opt_id,
        });
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

        // Present: bind the payload as the member's receiver, lower the
        // member, wrap its value.
        self.current_block = some_block;
        let payload = self.new_temp(Ty::Int);
        self.emit(MirInst::GetField {
            dest: payload,
            base: opt_id,
            field_index: 1,
        });
        let outer_binding = self.def_to_local.insert(SAFE_NAV_RECEIVER, payload);
        let lowered = self.lower_expr(&member);
        match outer_binding {
            Some(local) => self.def_to_local.insert(SAFE_NAV_RECEIVER, local),
            None => self.def_to_local.remove(&SAFE_NAV_RECEIVER),
        };
        let member_value = lowered?;
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 1,
        });
        self.emit(MirInst::SetField {
            base: result,
            field_index: 1,
            value: local_to_value(member_value),
        });
        self.set_terminator(Terminator::Goto(merge_block));

        // Absent: the result is absent too.
        self.current_block = none_block;
        self.emit(MirInst::SetTag {
            dest: result,
            tag: 0,
        });
        self.set_terminator(Terminator::Goto(merge_block));

        self.current_block = merge_block;
        Ok(Some(result))
    }
}
