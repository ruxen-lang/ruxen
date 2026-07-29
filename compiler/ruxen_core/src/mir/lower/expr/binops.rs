use super::super::*;

/// Numeric-type classification used by the comparison-operand re-materializer.
fn numeric_kind(ty: &Ty) -> Option<NumKind> {
    match ty {
        Ty::Float | Ty::Float32 | Ty::Float64 => Some(NumKind::Float),
        Ty::Int | Ty::Int8 | Ty::Int16 | Ty::Int32 | Ty::Int64 | Ty::ISize => {
            Some(NumKind::SignedInt)
        }
        Ty::UInt | Ty::UInt8 | Ty::UInt16 | Ty::UInt32 | Ty::UInt64 | Ty::USize => {
            Some(NumKind::UnsignedInt)
        }
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NumKind {
    Float,
    SignedInt,
    UnsignedInt,
}

impl<'a> Lowerer<'a> {
    fn nominal_comparison_method_available(
        &self,
        receiver_ty: &Ty,
        method: &str,
        args: &[HirExpr],
    ) -> bool {
        let class_name = type_name_from_ty(receiver_ty);
        let canonical = self
            .symbols
            .canonical_method_name(&class_name, method)
            .to_string();
        let owner = self.resolve_method_class_with_args(&class_name, &canonical, args);
        if self.class_has_method_accepting_args(&owner, &canonical, args) {
            return true;
        }
        self.trait_impls.iter().any(|(trait_name, implementors)| {
            implementors.iter().any(|name| name == &class_name)
                && self
                    .trait_default_methods
                    .get(trait_name)
                    .is_some_and(|methods| methods.contains_key(&canonical))
        })
    }

    /// True when a builtin-head receiver (`String` / `Array` / `Hash` / `Set`)
    /// declares a `.rx` method for operator `method` (e.g. `String#*`,
    /// `Array#+`). The generic-compiler end-state moves collection/string
    /// operators to `.rx`; when the method exists we route `a op b` to
    /// `a.OP(b)` rather than the machine floor, which cannot express a
    /// heap-collection operator and silently miscompiles (`"ab" * 3` → empty,
    /// `[1,2] + [3,4]` → 0). The receiver's display name carries generic args
    /// (`Array[Int]`); the `.rx` class parent is the bare head (`Array`), so
    /// strip the `[...]` suffix before probing.
    fn builtin_head_has_op_method(&self, head: &Ty, method: &str) -> bool {
        let raw = type_name_from_ty(head);
        let class_name = raw.split('[').next().unwrap_or(&raw);
        // NAME-only existence check (not arg-typed): typeck already selected and
        // type-checked this operator against the `.rx` method before MIR runs,
        // and the `.rx` operator method's generic param (`Array[T]`) would not
        // pass the strict `arg.ty == param.ty` arg check against a concrete
        // `Array[Int]` argument anyway. MIR only needs to agree that the method
        // exists so it dispatches there instead of the machine floor.
        self.symbols.iter().any(|def| {
            let crate::resolve::symbols::DefKind::Method { parent, .. } = &def.kind else {
                return false;
            };
            def.name == method
                && self
                    .symbols
                    .get(*parent)
                    .map(|p| p.name == class_name)
                    .unwrap_or(false)
        })
    }

    /// Q33: re-materialize a mismatched numeric operand pair to a common type
    /// before a `Compare`, so the comparison happens at one width with a
    /// signedness-correct int↔float conversion.
    ///
    /// The `Compare` instruction (like `SetField` in Q28) is width-blind:
    /// codegen coerces the rhs to the lhs's SSA type with the signedness-BLIND
    /// `coerce_value`, which uses `fcvt_from_uint` for an int→float crossing.
    /// That turns a signed `Int(-1)` (i64 `0xFFFF…`) into `1.8e19` instead of
    /// `-1.0`, so `f32_val == -1` is false. Routing the int operand through a
    /// target-typed `Assign` to the float type invokes codegen's Q5
    /// signedness-aware path (`fcvt_from_sint` for a signed source), exactly as
    /// a `let`-bound `as Float32` cast does, so both operands reach `Compare`
    /// at the same float width with the right sign. Mirrors `coerce_to_field_ty`.
    ///
    /// Common-type rule: if exactly one side is float, coerce the other side to
    /// the float side. If both are float but differ in width, coerce the
    /// narrower to the wider. Int-only and equal-type pairs are left untouched
    /// (the existing int Compare path is correct).
    fn coerce_compare_operands(
        &mut self,
        lhs_local: Option<LocalId>,
        rhs_local: Option<LocalId>,
    ) -> (Option<LocalId>, Option<LocalId>) {
        let (Some(l), Some(r)) = (lhs_local, rhs_local) else {
            return (lhs_local, rhs_local);
        };
        let Some(lty) = self.fn_ref().locals.get(l as usize).map(|x| x.ty.clone()) else {
            return (lhs_local, rhs_local);
        };
        let Some(rty) = self.fn_ref().locals.get(r as usize).map(|x| x.ty.clone()) else {
            return (lhs_local, rhs_local);
        };
        let (Some(lk), Some(rk)) = (numeric_kind(&lty), numeric_kind(&rty)) else {
            return (lhs_local, rhs_local);
        };
        if lty == rty {
            return (lhs_local, rhs_local);
        }
        // Pick the common type. Float dominates int; the wider float wins.
        let float_rank = |ty: &Ty| match ty {
            Ty::Float | Ty::Float64 => 2u8,
            Ty::Float32 => 1u8,
            _ => 0u8,
        };
        let common = match (lk, rk) {
            (NumKind::Float, NumKind::Float) => {
                if float_rank(&lty) >= float_rank(&rty) {
                    lty.clone()
                } else {
                    rty.clone()
                }
            }
            (NumKind::Float, _) => lty.clone(),
            (_, NumKind::Float) => rty.clone(),
            // Both integral but differing kinds/widths: leave for the existing
            // int Compare path (codegen coerces rhs→lhs width via ireduce/
            // extend; the Q33 sign hazard only bites the int↔float crossing).
            _ => return (lhs_local, rhs_local),
        };
        let new_l = if lty == common {
            Some(l)
        } else {
            let d = self.new_temp(common.clone());
            self.emit(MirInst::Assign {
                dest: d,
                value: MirValue::Use(l),
            });
            Some(d)
        };
        let new_r = if rty == common {
            Some(r)
        } else {
            let d = self.new_temp(common.clone());
            self.emit(MirInst::Assign {
                dest: d,
                value: MirValue::Use(r),
            });
            Some(d)
        };
        (new_l, new_r)
    }

    /// Coerce an operand local to `f64` for `ruxen_float_pow`. A float-typed
    /// local passes through; an int local is routed through a target-typed
    /// `Assign` to `Ty::Float`, invoking codegen's signedness-aware int→float
    /// conversion (the same Q33 path `coerce_compare_operands` uses).
    fn coerce_local_to_float(&mut self, local: Option<LocalId>) -> Option<LocalId> {
        let Some(l) = local else {
            return local;
        };
        let ty = self.fn_ref().locals.get(l as usize).map(|x| x.ty.clone());
        if matches!(ty, Some(Ty::Float) | Some(Ty::Float32) | Some(Ty::Float64)) {
            return Some(l);
        }
        let d = self.new_temp(Ty::Float);
        self.emit(MirInst::Assign {
            dest: d,
            value: MirValue::Use(l),
        });
        Some(d)
    }

    /// Short-circuit lowering for `&&` / `||`. Builds a branch CFG (mirroring
    /// the `if` lowering in `control.rs`) so the RHS is evaluated ONLY on the
    /// path where the LHS did not already decide the result: `a && b` skips `b`
    /// when `a` is false; `a || b` skips `b` when `a` is true. Without this the
    /// eager `MirInst::BinOp` path evaluates both operands, so
    /// `ptr != nil && ptr.field` would touch `ptr.field` even when `ptr` is nil.
    fn lower_short_circuit(
        &mut self,
        op: BinOp,
        left: &HirExpr,
        right: &HirExpr,
    ) -> Result<LocalId, String> {
        let lhs_local = self.lower_expr(left)?;
        let lhs_val = local_to_value(lhs_local);

        let result = self.new_temp(Ty::Bool);
        let rhs_block = self.new_block();
        let short_block = self.new_block();
        let merge_block = self.new_block();

        // `&&`: LHS true → evaluate RHS, LHS false → short to `false`.
        // `||`: LHS true → short to `true`, LHS false → evaluate RHS.
        let (then_block, else_block) = match op {
            BinOp::And | BinOp::KeywordAnd => (rhs_block, short_block),
            BinOp::Or | BinOp::KeywordOr => (short_block, rhs_block),
            _ => unreachable!("lower_short_circuit only handles And/Or"),
        };
        self.set_terminator(Terminator::Branch {
            cond: lhs_val,
            then_block,
            else_block,
        });

        // Short-circuit block: the LHS already fixed the result
        // (`false` for `&&`, `true` for `||`).
        self.current_block = short_block;
        self.emit(MirInst::Assign {
            dest: result,
            value: MirValue::Literal(Literal::Bool(matches!(op, BinOp::Or | BinOp::KeywordOr))),
        });
        self.set_terminator(Terminator::Goto(merge_block));

        // RHS block: the result is the (Bool) value of the right operand,
        // evaluated here so it only runs on this path. Guard against a diverging
        // RHS (e.g. `a || panic!(…)`) that already set its own terminator: only
        // assign the result and wire the Goto when the block is still open.
        self.current_block = rhs_block;
        let rhs_local = self.lower_expr(right)?;
        if matches!(self.get_terminator(), Terminator::Unreachable) {
            self.emit(MirInst::Assign {
                dest: result,
                value: local_to_value(rhs_local),
            });
            self.set_terminator(Terminator::Goto(merge_block));
        }

        self.current_block = merge_block;
        Ok(result)
    }

    /// Lower `Option[T] ?? fallback` as a branch on the Option tag. The Some
    /// path extracts the payload; only the None path evaluates `fallback`.
    fn lower_nil_coalesce(
        &mut self,
        left: &HirExpr,
        right: &HirExpr,
        result_ty: &Ty,
    ) -> Result<LocalId, String> {
        let option = self
            .lower_expr(left)?
            .ok_or_else(|| "left operand of `??` has no value".to_string())?;
        let tag = self.new_temp(Ty::Int32);
        self.emit(MirInst::GetTag {
            dest: tag,
            src: option,
        });
        let is_some = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest: is_some,
            op: CmpOp::Eq,
            lhs: MirValue::Use(tag),
            rhs: MirValue::Literal(Literal::Int(1)),
        });

        let some_block = self.new_block();
        let fallback_block = self.new_block();
        let merge_block = self.new_block();
        let result = self.new_temp(result_ty.clone());
        self.set_terminator(Terminator::Branch {
            cond: MirValue::Use(is_some),
            then_block: some_block,
            else_block: fallback_block,
        });

        self.current_block = some_block;
        let payload = self.new_temp(Ty::Int);
        self.emit(MirInst::GetPayload {
            dest: payload,
            src: option,
            ty: left.ty.clone(),
        });
        self.emit(MirInst::GetField {
            dest: result,
            base: payload,
            field_index: 0,
        });
        self.set_terminator(Terminator::Goto(merge_block));

        self.current_block = fallback_block;
        let fallback = self.lower_expr(right)?;
        if matches!(self.get_terminator(), Terminator::Unreachable) {
            self.emit(MirInst::Assign {
                dest: result,
                value: local_to_value(fallback),
            });
            self.set_terminator(Terminator::Goto(merge_block));
        }

        self.current_block = merge_block;
        Ok(result)
    }

    pub(super) fn lower_binops(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Binary operations ───────────────────────────────────
            HirExprKind::BinaryOp { op, left, right } => {
                if matches!(op, BinOp::NilCoalesce) {
                    return Ok(Some(self.lower_nil_coalesce(left, right, &expr.ty)?));
                }

                // ── Operator → method desugar ──
                // Prefer an available user method or included mixin default
                // before structural derives. The availability probe is what
                // preserves auto-PartialEq/Ord for nominal types that do not
                // define the corresponding symbol method.
                fn unwrap_ref(t: &Ty) -> &Ty {
                    match t {
                        Ty::Ref(inner)
                        | Ty::RefMut(inner)
                        | Ty::RefLifetime(_, inner)
                        | Ty::RefMutLifetime(_, inner) => unwrap_ref(inner),
                        _ => t,
                    }
                }
                let lhs_nominal = matches!(
                    unwrap_ref(&left.ty),
                    Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
                );
                if lhs_nominal {
                    if let Some(method) = op.method_name() {
                        let args = vec![(**right).clone()];
                        let comparison = matches!(
                            op,
                            BinOp::Eq
                                | BinOp::NotEq
                                | BinOp::Lt
                                | BinOp::Gt
                                | BinOp::LtEq
                                | BinOp::GtEq
                                | BinOp::Cmp
                        );
                        if !comparison
                            || self.nominal_comparison_method_available(&left.ty, method, &args)
                        {
                            let synthetic = HirExpr {
                                kind: HirExprKind::MethodCall {
                                    object: Box::new((**left).clone()),
                                    method: UNRESOLVED_DEF,
                                    method_name: method.to_string(),
                                    generic_args: vec![],
                                    args,
                                    block: None,
                                },
                                ty: expr.ty.clone(),
                                span: expr.span.clone(),
                            };
                            let result = self.lower_method_call(&synthetic)?;
                            if matches!(op, BinOp::NotEq) {
                                if let Some(src) = result {
                                    let dest = self.new_temp(Ty::Bool);
                                    self.emit(MirInst::Not {
                                        dest,
                                        operand: MirValue::Use(src),
                                    });
                                    return Ok(Some(dest));
                                }
                            }
                            return Ok(result);
                        }
                    }
                }

                // ── Builtin-head operator → `.rx` method ──
                // String / Array / Hash / Set carry `.rx` operator methods
                // (`def *`, `def +`, …) for the arithmetic/bitwise/shift
                // operators the machine floor can't express on a heap
                // collection. Route to the method when the `.rx` class declares
                // it — the builtin-head analogue of the nominal route above,
                // and the generic-compiler end-state. Comparison/equality and
                // `~=`/`??` keep their dedicated routes below, and a head with
                // no such method falls through to the existing hardcoded
                // runtime routes / machine floor. Runs before operand lowering,
                // so the synthetic call re-lowers the operands exactly once.
                if !lhs_nominal
                    && !matches!(
                        op,
                        BinOp::Eq
                            | BinOp::NotEq
                            | BinOp::Lt
                            | BinOp::Gt
                            | BinOp::LtEq
                            | BinOp::GtEq
                            | BinOp::Cmp
                            | BinOp::MatchOp
                            | BinOp::NilCoalesce
                    )
                {
                    let head = unwrap_ref(&left.ty);
                    if matches!(
                        head,
                        Ty::String | Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_)
                    ) {
                        if let Some(method) = op.method_name() {
                            if self.builtin_head_has_op_method(head, method) {
                                let synthetic = HirExpr {
                                    kind: HirExprKind::MethodCall {
                                        object: Box::new((**left).clone()),
                                        method: UNRESOLVED_DEF,
                                        method_name: method.to_string(),
                                        generic_args: vec![],
                                        args: vec![(**right).clone()],
                                        block: None,
                                    },
                                    ty: expr.ty.clone(),
                                    span: expr.span.clone(),
                                };
                                return self.lower_method_call(&synthetic);
                            }
                        }
                    }
                }

                // ── derive PartialEq: structural equality on structs ──
                // `a == b` and `a != b` on a struct that derives PartialEq
                // must compare field-by-field. The default `Compare` lowering
                // would compare struct *pointers* (heap addresses), false for
                // two distinct allocations even when their fields match.
                if matches!(op, BinOp::Eq | BinOp::NotEq) {
                    if let Some(struct_name) = struct_name_with_partial_eq(&left.ty, self.symbols) {
                        if let Some(field_info) = struct_field_layout(&struct_name, self.symbols) {
                            return Ok(Some(self.lower_struct_partial_eq(
                                left,
                                right,
                                *op,
                                &field_info,
                            )?));
                        }
                    }
                }

                // ── derive Ord / PartialOrd: route ordering operators to
                // the synthesised `<Type>_cmp` / `<Type>_partial_cmp`
                // helper. The default `Compare` lowering below would
                // compare struct *pointers* (heap addresses), which
                // gives meaningless lex order across allocations. The
                // synthesiser's tuple-style field walk already returns
                // -1 / 0 / +1, so we only need to fold that result
                // through the requested operator.
                if matches!(op, BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq) {
                    if let Some((struct_name, partial)) =
                        struct_name_with_ord(&left.ty, self.symbols)
                    {
                        return Ok(Some(self.lower_struct_ord(
                            left,
                            right,
                            *op,
                            &struct_name,
                            partial,
                        )?));
                    }
                }

                // ── Short-circuit `&&` / `||` ──
                // These MUST NOT evaluate the RHS once the LHS decides the
                // result (`ptr != nil && ptr.field`, `x || default()`). The
                // eager `MirInst::BinOp` path below lowers BOTH operands first,
                // so intercept here and build a branch CFG before either operand
                // is lowered, so only the taken side evaluates its operand.
                if matches!(
                    op,
                    BinOp::And | BinOp::Or | BinOp::KeywordAnd | BinOp::KeywordOr
                ) {
                    return Ok(Some(self.lower_short_circuit(*op, left, right)?));
                }

                let lhs_local = self.lower_expr(left)?;
                let rhs_local = self.lower_expr(right)?;
                let lhs_val = local_to_value(lhs_local);
                let rhs_val = local_to_value(rhs_local);

                // ── Exponentiation `a ** b` (power operator) ──
                // There is no machine `pow` instruction, so route to a runtime
                // call. `Int ** Int` → `ruxen_int_pow(i64,i64)->i64`; any float
                // operand → `ruxen_float_pow(f64,f64)->f64` with the int side
                // coerced to `f64` first (target-typed `Assign`, the Q33 path).
                // typeck (`infer_binop`) guarantees `expr.ty == Ty::Float` here
                // whenever either operand is float.
                if matches!(op, BinOp::Pow) {
                    if matches!(expr.ty, Ty::Float | Ty::Float32 | Ty::Float64) {
                        let l = self.coerce_local_to_float(lhs_local);
                        let r = self.coerce_local_to_float(rhs_local);
                        let dest = self.new_temp(Ty::Float);
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: "ruxen_float_pow".to_string(),
                            args: vec![local_to_value(l), local_to_value(r)],
                        });
                        return Ok(Some(dest));
                    }
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "ruxen_int_pow".to_string(),
                        args: vec![lhs_val, rhs_val],
                    });
                    return Ok(Some(dest));
                }

                // ── Three-way comparison `a <=> b` (spaceship) ──
                // Returns Int (-1 / 0 / 1). Route by operand type: two Strings
                // → `ruxen_str_spaceship`; any float operand → `ruxen_float_cmp`
                // (int side coerced to f64); otherwise `ruxen_int_cmp`.
                if matches!(op, BinOp::Cmp) {
                    let dest = self.new_temp(Ty::Int);
                    if matches!(left.ty, Ty::String) {
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: "ruxen_str_spaceship".to_string(),
                            args: vec![lhs_val, rhs_val],
                        });
                    } else if matches!(left.ty, Ty::Float | Ty::Float32 | Ty::Float64)
                        || matches!(right.ty, Ty::Float | Ty::Float32 | Ty::Float64)
                    {
                        let l = self.coerce_local_to_float(lhs_local);
                        let r = self.coerce_local_to_float(rhs_local);
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: "ruxen_float_cmp".to_string(),
                            args: vec![local_to_value(l), local_to_value(r)],
                        });
                    } else {
                        self.emit(MirInst::Call {
                            dest: Some(dest),
                            callee: "ruxen_int_cmp".to_string(),
                            args: vec![lhs_val, rhs_val],
                        });
                    }
                    return Ok(Some(dest));
                }

                // ── Phase 2 stdlib batch 2 (#02): String + String ──
                // The default `MirInst::BinOp { op: Add, ... }` would
                // treat both operands as integers and codegen would
                // emit an integer-add over heap pointers — undefined
                // behaviour. Route through `ruxen_string_concat`
                // instead. Matches the existing string-interpolation
                // lowering, which already calls the same runtime fn.
                if matches!(op, BinOp::Add)
                    && matches!(left.ty, Ty::String)
                    && matches!(right.ty, Ty::String)
                {
                    let dest = self.new_temp(Ty::String);
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "ruxen_string_concat".to_string(),
                        args: vec![lhs_val, rhs_val],
                    });
                    return Ok(Some(dest));
                }

                // ── Phase 2 stdlib batch 1 (#03): Vec[T] == Vec[T] ──
                // The default integer Compare would compare heap
                // pointers, returning false for any two distinct
                // allocations even when their elements match. Route
                // through `ruxen_vec_eq` for both `==` and `!=`.
                if matches!(op, BinOp::Eq | BinOp::NotEq)
                    && matches!(left.ty, Ty::Array(_))
                    && matches!(right.ty, Ty::Array(_))
                {
                    let cmp = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Call {
                        dest: Some(cmp),
                        callee: "ruxen_vec_eq".to_string(),
                        args: vec![lhs_val, rhs_val],
                    });
                    if matches!(op, BinOp::Eq) {
                        return Ok(Some(cmp));
                    }
                    let dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Not {
                        dest,
                        operand: MirValue::Use(cmp),
                    });
                    return Ok(Some(dest));
                }

                // ── Phase 2 stdlib (#04): HashMap == HashMap ──
                // Same justification as Vec equality above — default
                // integer compare on the spine pointers is meaningless
                // across allocations. Route through `ruxen_hash_eq`.
                if matches!(op, BinOp::Eq | BinOp::NotEq)
                    && matches!(left.ty, Ty::Map(_, _))
                    && matches!(right.ty, Ty::Map(_, _))
                {
                    let cmp = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Call {
                        dest: Some(cmp),
                        callee: "ruxen_hash_eq".to_string(),
                        args: vec![lhs_val, rhs_val],
                    });
                    if matches!(op, BinOp::Eq) {
                        return Ok(Some(cmp));
                    }
                    let dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Not {
                        dest,
                        operand: MirValue::Use(cmp),
                    });
                    return Ok(Some(dest));
                }

                // ── Phase 2 stdlib (#04): HashSet == HashSet ──
                if matches!(op, BinOp::Eq | BinOp::NotEq)
                    && matches!(left.ty, Ty::Set(_))
                    && matches!(right.ty, Ty::Set(_))
                {
                    let cmp = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Call {
                        dest: Some(cmp),
                        callee: "ruxen_set_eq".to_string(),
                        args: vec![lhs_val, rhs_val],
                    });
                    if matches!(op, BinOp::Eq) {
                        return Ok(Some(cmp));
                    }
                    let dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Not {
                        dest,
                        operand: MirValue::Use(cmp),
                    });
                    return Ok(Some(dest));
                }

                // Duration / Instant arithmetic special-cases REMOVED
                // (Task OP, Step 3): `Duration + Duration` / `- ` and
                // `Instant - Instant` now flow through the generic
                // nominal-receiver method route above (they declare
                // `def +`/`def -` in time/src/lib.rx aliasing the same
                // `ruxen_duration_add`/`_sub`/`ruxen_instant_sub` symbols).

                // ── std.regex (#06.96): `String ~= Regex` -> `Bool` ──
                // Desugars to `ruxen_regex_is_match(regex_handle, text)`
                // (note arg order: the C runtime takes the regex first,
                // text second). Typeck enforced the operand types
                // already (E1702); on the off-chance the literal hadn't
                // resolved (e.g. an Infer var) we fall through to the
                // default arm and let codegen catch it via the
                // `unreachable!` in `emit_binop`.
                if matches!(op, BinOp::MatchOp) {
                    let dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "ruxen_regex_is_match".to_string(),
                        // C signature: ruxen_regex_is_match(pcre2_code_8 *r, const char *text)
                        args: vec![rhs_val, lhs_val],
                    });
                    return Ok(Some(dest));
                }

                let dest = self.new_temp(expr.ty.clone());

                if is_comparison(*op) {
                    // Q33: re-materialize a mismatched numeric operand pair to a
                    // common float width with a signedness-correct conversion
                    // before the width-blind `Compare` (e.g. `f32 == -1`, where
                    // the `-1` rhs is Int-typed in MIR). Equal-type and int-only
                    // pairs pass through unchanged.
                    let (lhs_local, rhs_local) = self.coerce_compare_operands(lhs_local, rhs_local);
                    let lhs_val = local_to_value(lhs_local);
                    let rhs_val = local_to_value(rhs_local);
                    let cmp_op = binop_to_cmpop(*op);
                    self.emit(MirInst::Compare {
                        dest,
                        op: cmp_op,
                        lhs: lhs_val,
                        rhs: rhs_val,
                    });
                } else {
                    self.emit(MirInst::BinOp {
                        dest,
                        op: *op,
                        lhs: lhs_val,
                        rhs: rhs_val,
                    });
                }
                Ok(Some(dest))
            }

            // ── Unary operations ────────────────────────────────────
            _ => unreachable!("lower_binops: dispatched to wrong helper"),
        }
    }
}
