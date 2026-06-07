use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_binops(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Binary operations ───────────────────────────────────
            HirExprKind::BinaryOp { op, left, right } => {
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

                // ── Operator → method desugar (Task OP, Step 3) ──
                // A NOMINAL receiver (user class / struct / enum, and
                // stdlib classes like Duration) with a MIGRATED op
                // (`+ - * / %`, `& | ^ << >>`) lowers as a real method
                // call `left.OP(right)` to its `.rx` `def OP`. This is the
                // generic mechanism that REPLACED the hardcoded
                // Duration/Instant arithmetic arms (Duration now declares
                // `def +`/`def -` in `.rx`). Machine primitives, String, and
                // the collection heads fall through to the instruction /
                // special-case lowering below — the machine floor, no
                // method-call overhead on the hot path.
                //
                // We synthesize the same `MethodCall` HIR node the explicit
                // `a.+(b)` surface produces and delegate to
                // `lower_method_call`, so there is ONE method-call lowering
                // path (overload selection, FFI alias rewrite, receiver
                // prepend). This MUST run before the operands are lowered
                // below, or they'd be lowered twice.
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

                let lhs_local = self.lower_expr(left)?;
                let rhs_local = self.lower_expr(right)?;
                let lhs_val = local_to_value(lhs_local);
                let rhs_val = local_to_value(rhs_local);

                // ── Phase 2 stdlib batch 2 (#02): String + String ──
                // The default `MirInst::BinOp { op: Add, ... }` would
                // treat both operands as integers and codegen would
                // emit an integer-add over heap pointers — undefined
                // behaviour. Route through `ruxen_string_concat`
                // instead. Matches the existing string-interpolation
                // lowering, which already calls the same runtime fn.
                if matches!(op, BinOp::Add)
                    && matches!(left.ty, Ty::String | Ty::Str)
                    && matches!(right.ty, Ty::String | Ty::Str)
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
