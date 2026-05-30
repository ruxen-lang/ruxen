use super::*;

impl<'a> Lowerer<'a> {
    // ── Match lowering ──────────────────────────────────────────────────

    pub(super) fn lower_match(
        &mut self,
        expr: &HirExpr,
        scrutinee: &HirExpr,
        arms: &[HirMatchArm],
    ) -> Result<Option<LocalId>, String> {
        let scrut_local = self.lower_expr(scrutinee)?;

        // For enum-like types (Enum, Result, Option), use tag-based
        // switch. Also treat unresolved Infer types as enum if any arm
        // uses an Enum pattern (e.g., Ok/Err, Some/None).
        let is_enum = matches!(
            scrutinee.ty,
            Ty::Enum { .. } | Ty::Result(_, _) | Ty::Option(_)
        ) || arms
            .iter()
            .any(|arm| matches!(arm.pattern, HirPattern::Enum { .. }));

        let merge_block = self.new_block();
        let result_local = if expr.ty != Ty::Unit && expr.ty != Ty::Never {
            Some(self.new_temp(expr.ty.clone()))
        } else {
            None
        };

        if is_enum {
            // Get the discriminant tag.
            let scrut = scrut_local.unwrap_or_else(|| {
                // Scrutinee didn't produce a local (e.g. Unit expression).
                // Create a zero-valued temporary as a fallback.
                let tmp = self.new_temp(scrutinee.ty.clone());
                self.emit(MirInst::Assign {
                    dest: tmp,
                    value: MirValue::Literal(Literal::Int(0)),
                });
                tmp
            });
            let tag_local = self.new_temp(Ty::Int32);
            self.emit(MirInst::GetTag {
                dest: tag_local,
                src: scrut,
            });

            // Build switch targets. Every arm gets its own entry block
            // so arms with guards can fall through to the next arm on a
            // failed guard, and multiple arms targeting the same
            // variant can be chained in source order (first matching-
            // and-guard-true arm wins).
            let mut targets: Vec<(i64, BlockId)> = Vec::new();
            let otherwise = self.new_block(); // fallback / wildcard
            let mut seen_variants: HashMap<i64, BlockId> = HashMap::new();

            // Pre-allocate an entry block for every arm. The first
            // wildcard / binding arm lives directly in `otherwise` so
            // the switch can land there without an extra hop.
            let mut arm_entry_blocks: Vec<BlockId> = Vec::with_capacity(arms.len());
            let mut first_wildcard_placed = false;
            for arm in arms.iter() {
                let is_wild = !matches!(arm.pattern, HirPattern::Enum { .. });
                let block = if is_wild && !first_wildcard_placed {
                    first_wildcard_placed = true;
                    otherwise
                } else {
                    self.new_block()
                };
                arm_entry_blocks.push(block);
            }

            // Compute each arm's fallthrough target: where control
            // transfers when the arm's pattern or guard fails. For an
            // enum arm, fallthrough is the next arm whose pattern could
            // still match this variant (same variant index, or a
            // wildcard / binding arm that matches anything). Falling
            // off the end lands on `otherwise`.
            let mut arm_fallthroughs: Vec<BlockId> = Vec::with_capacity(arms.len());
            for (i, arm) in arms.iter().enumerate() {
                let this_variant = match &arm.pattern {
                    HirPattern::Enum { variant_idx, .. } => Some(*variant_idx as i64),
                    _ => None,
                };
                let mut target = otherwise;
                for (j, other) in arms.iter().enumerate().skip(i + 1) {
                    match &other.pattern {
                        HirPattern::Enum { variant_idx, .. } => {
                            if Some(*variant_idx as i64) == this_variant {
                                target = arm_entry_blocks[j];
                                break;
                            }
                        }
                        _ => {
                            // Wildcard/binding — matches any variant.
                            target = arm_entry_blocks[j];
                            break;
                        }
                    }
                }
                arm_fallthroughs.push(target);
            }

            let mut arm_blocks: Vec<(BlockId, &HirMatchArm)> = Vec::new();
            let mut wildcard_arm: Option<(BlockId, usize)> = None;

            for (arm_idx, arm) in arms.iter().enumerate() {
                let arm_block = arm_entry_blocks[arm_idx];
                if let HirPattern::Enum { variant_idx, .. } = &arm.pattern {
                    let disc = *variant_idx as i64;
                    seen_variants.entry(disc).or_insert_with(|| {
                        targets.push((disc, arm_block));
                        arm_block
                    });
                    arm_blocks.push((arm_block, arm));
                } else {
                    // Wildcard / binding — first one lives at
                    // `otherwise`; later ones are reached only via
                    // fallthrough from a preceding arm's failed guard.
                    wildcard_arm = Some((otherwise, arm_idx));
                    arm_blocks.push((arm_block, arm));
                }
            }

            self.set_terminator(Terminator::Switch {
                value: MirValue::Use(tag_local),
                targets,
                otherwise,
            });

            // Lower each arm body.
            for (arm_idx, (arm_block, arm)) in arm_blocks.iter().enumerate() {
                self.current_block = *arm_block;

                // Bind pattern variables if it's an Enum pattern with field bindings.
                if let HirPattern::Enum {
                    type_def,
                    variant_idx,
                    fields,
                    ..
                } = &arm.pattern
                {
                    if !fields.is_empty() {
                        // For Option/Result, derive field types from the
                        // scrutinee type since the variant definitions use
                        // TypeParam placeholders.
                        //
                        // Variant indices (defined in resolve/stdlib/{option,result}.rs):
                        //   Option: None=0, Some=1
                        //   Result: Ok=0,   Err=1
                        let variant_field_types = match &scrutinee.ty {
                            Ty::Option(inner) if *variant_idx == 1 => {
                                // Some(T) — the field type is the inner type
                                vec![*inner.clone()]
                            }
                            Ty::Result(ok, _err) if *variant_idx == 0 => {
                                // Ok(T) — the field type is the ok type
                                vec![*ok.clone()]
                            }
                            Ty::Result(_ok, err) if *variant_idx == 1 => {
                                // Err(E) — the field type is the error type
                                vec![*err.clone()]
                            }
                            _ => self.lookup_variant_field_types(*type_def, *variant_idx),
                        };

                        // Get the payload pointer (offset 8 from enum base).
                        let payload_ptr = self.new_temp(scrutinee.ty.clone());
                        self.emit(MirInst::GetPayload {
                            dest: payload_ptr,
                            src: scrut,
                            ty: scrutinee.ty.clone(),
                        });

                        for (idx, field_pat) in fields.iter().enumerate() {
                            let binding_info = match field_pat {
                                HirPattern::Binding {
                                    def_id,
                                    name,
                                    mutable,
                                    ..
                                } => Some((*def_id, name.as_str(), *mutable)),
                                HirPattern::Ref {
                                    def_id,
                                    name,
                                    mutable,
                                    ..
                                } => {
                                    // `ref` pattern: bind a reference to
                                    // the field. At runtime references are
                                    // the same representation as values for
                                    // heap types, so treat identically to
                                    // Binding for code generation purposes.
                                    Some((*def_id, name.as_str(), *mutable))
                                }
                                _ => None,
                            };
                            if let Some((def_id, name, mutable)) = binding_info {
                                let field_ty =
                                    variant_field_types.get(idx).cloned().unwrap_or(Ty::Int);
                                let local = self.new_local_named(name, field_ty, mutable);
                                self.def_to_local.insert(def_id, local);
                                self.emit(MirInst::GetField {
                                    dest: local,
                                    base: payload_ptr,
                                    field_index: idx,
                                });
                            } else if matches!(field_pat, HirPattern::Wildcard { .. }) {
                                // A `_`-discarded payload field that owns heap
                                // (e.g. the File in `match File.open(p) {
                                // Ok(_) => .. }`) is never bound to a local, so
                                // nothing frees it and the resource leaks —
                                // fixture 518_file_drop_closes leaks an fd per
                                // iteration. Materialise it into a temp via
                                // GetField so scope-exit drop elaboration can
                                // run its destructor: the GetField-from-payload
                                // rule in `compute_dealloc_safe_locals` marks
                                // the temp owned, and the drop filter emits
                                // `{Class}_drop` + dealloc. Only heap-owning
                                // payload types need this; Int/Bool/etc. and
                                // enum payloads (which the container drop
                                // handles) are left alone.
                                let field_ty =
                                    variant_field_types.get(idx).cloned().unwrap_or(Ty::Int);
                                if matches!(
                                    field_ty,
                                    Ty::Class { .. }
                                        | Ty::Struct { .. }
                                        | Ty::String
                                        | Ty::Array(_)
                                        | Ty::Map(_, _)
                                        | Ty::Set(_)
                                ) {
                                    let tmp = self.new_temp(field_ty);
                                    self.emit(MirInst::GetField {
                                        dest: tmp,
                                        base: payload_ptr,
                                        field_index: idx,
                                    });
                                }
                            }

                            // Handle nested Enum patterns: e.g.
                            // Err(TaskError.NotFound(id)) — the field
                            // pattern itself is an Enum whose fields need
                            // to be bound.
                            if let HirPattern::Enum {
                                type_def: inner_type_def,
                                variant_idx: inner_variant_idx,
                                fields: inner_fields,
                                ..
                            } = field_pat
                            {
                                // Extract the outer field (the inner enum
                                // value) from the payload.
                                let inner_enum_ty =
                                    variant_field_types.get(idx).cloned().unwrap_or(Ty::Int);
                                let inner_enum_local = self.new_temp(inner_enum_ty.clone());
                                self.emit(MirInst::GetField {
                                    dest: inner_enum_local,
                                    base: payload_ptr,
                                    field_index: idx,
                                });

                                if !inner_fields.is_empty() {
                                    let inner_variant_field_types = self
                                        .lookup_variant_field_types(
                                            *inner_type_def,
                                            *inner_variant_idx,
                                        );

                                    // Get the inner payload pointer.
                                    let inner_payload = self.new_temp(inner_enum_ty.clone());
                                    self.emit(MirInst::GetPayload {
                                        dest: inner_payload,
                                        src: inner_enum_local,
                                        ty: inner_enum_ty,
                                    });

                                    for (inner_idx, inner_field_pat) in
                                        inner_fields.iter().enumerate()
                                    {
                                        let inner_binding = match inner_field_pat {
                                            HirPattern::Binding {
                                                def_id,
                                                name,
                                                mutable,
                                                ..
                                            } => Some((*def_id, name.as_str(), *mutable)),
                                            HirPattern::Ref {
                                                def_id,
                                                name,
                                                mutable,
                                                ..
                                            } => Some((*def_id, name.as_str(), *mutable)),
                                            _ => None,
                                        };
                                        if let Some((inner_def_id, inner_name, inner_mutable)) =
                                            inner_binding
                                        {
                                            let inner_field_ty = inner_variant_field_types
                                                .get(inner_idx)
                                                .cloned()
                                                .unwrap_or(Ty::Int);
                                            let inner_local = self.new_local_named(
                                                inner_name,
                                                inner_field_ty,
                                                inner_mutable,
                                            );
                                            self.def_to_local.insert(inner_def_id, inner_local);
                                            self.emit(MirInst::GetField {
                                                dest: inner_local,
                                                base: inner_payload,
                                                field_index: inner_idx,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else if let HirPattern::Binding {
                    def_id,
                    name,
                    mutable,
                    ..
                } = &arm.pattern
                {
                    // Bind the scrutinee value to the variable — use the scrutinee's type.
                    let binding_ty = scrutinee.ty.clone();
                    let local = self.new_local_named(name, binding_ty, *mutable);
                    self.def_to_local.insert(*def_id, local);
                    self.emit(MirInst::Assign {
                        dest: local,
                        value: MirValue::Use(scrut),
                    });
                }

                // Evaluate the guard (if any) after pattern bindings.
                // Pattern bindings are already registered in
                // `def_to_local`, so the guard expression can reference
                // them. On guard failure, control falls through to the
                // next arm that could match (same variant or wildcard).
                if let Some(guard_expr) = &arm.guard {
                    let guard_local = self.lower_expr(guard_expr)?;
                    let guard_val = local_to_value(guard_local);
                    let body_block = self.new_block();
                    self.set_terminator(Terminator::Branch {
                        cond: guard_val,
                        then_block: body_block,
                        else_block: arm_fallthroughs[arm_idx],
                    });
                    self.current_block = body_block;
                }

                let body_result = self.lower_expr(&arm.body)?;
                if matches!(self.get_terminator(), Terminator::Unreachable) {
                    if let Some(dest) = result_local {
                        let val = local_to_value(body_result);
                        self.emit(MirInst::Assign { dest, value: val });
                    }
                    self.set_terminator(Terminator::Goto(merge_block));
                }
            }

            // If no wildcard arm was found, the otherwise block is unreachable.
            if wildcard_arm.is_none() {
                self.current_block = otherwise;
                self.set_terminator(Terminator::Unreachable);
            }
        } else {
            // Non-enum match: cascading branches (if/else chain).
            self.lower_match_cascading(
                scrut_local,
                &scrutinee.ty,
                arms,
                result_local,
                merge_block,
            )?;
        }

        self.current_block = merge_block;
        Ok(result_local)
    }

    fn lower_match_cascading(
        &mut self,
        scrut_local: Option<LocalId>,
        scrut_ty: &Ty,
        arms: &[HirMatchArm],
        result_local: Option<LocalId>,
        merge_block: BlockId,
    ) -> Result<(), String> {
        if arms.is_empty() {
            self.set_terminator(Terminator::Goto(merge_block));
            return Ok(());
        }

        for (i, arm) in arms.iter().enumerate() {
            let is_last = i == arms.len() - 1;
            let arm_body_block = self.new_block();
            let next_block = if is_last {
                merge_block
            } else {
                self.new_block()
            };

            // When a guard is present, the pattern-match target is an
            // intermediate block that evaluates the guard before
            // dispatching to the body or falling through to `next_block`.
            let has_guard = arm.guard.is_some();
            let match_target = if has_guard {
                self.new_block()
            } else {
                arm_body_block
            };

            match &arm.pattern {
                HirPattern::Wildcard { .. }
                | HirPattern::Binding { .. }
                | HirPattern::Ref { .. } => {
                    // Wildcard / binding / ref always matches.
                    let binding_info = match &arm.pattern {
                        HirPattern::Binding {
                            def_id,
                            name,
                            mutable,
                            ..
                        }
                        | HirPattern::Ref {
                            def_id,
                            name,
                            mutable,
                            ..
                        } => Some((*def_id, name.clone(), *mutable)),
                        _ => None,
                    };
                    if let Some((def_id, name, mutable)) = binding_info {
                        if let Some(scrut) = scrut_local {
                            let local = self.new_local_named(&name, scrut_ty.clone(), mutable);
                            self.def_to_local.insert(def_id, local);
                            self.emit(MirInst::Assign {
                                dest: local,
                                value: MirValue::Use(scrut),
                            });
                        }
                    }
                    self.set_terminator(Terminator::Goto(match_target));
                }
                HirPattern::Or { patterns, .. } => {
                    // Or-pattern: matches if any sub-pattern matches. For
                    // v0.1 we restrict or-patterns to literal / wildcard
                    // alternatives (no binding alternatives) — the parser
                    // accepts more, but we guard here.
                    let mut all_literal_or_wild = true;
                    for p in patterns {
                        match p {
                            HirPattern::Literal { .. } | HirPattern::Wildcard { .. } => {}
                            _ => all_literal_or_wild = false,
                        }
                    }
                    if !all_literal_or_wild {
                        // Fall through to the arm body (best-effort) so
                        // typeck/resolve don't crash; emit a diagnostic-
                        // worthy no-op. A future pass can add uniform-
                        // binding validation.
                        self.set_terminator(Terminator::Goto(match_target));
                    } else {
                        // Build a chain of tests across alternatives.
                        self.lower_or_pattern(
                            scrut_local,
                            scrut_ty,
                            patterns,
                            match_target,
                            next_block,
                        )?;
                    }
                }
                HirPattern::Tuple { elements, .. } => {
                    // Tuple pattern: compare each element against the
                    // scrutinee's corresponding field. Literals gate the
                    // match; bindings always accept and introduce a local.
                    if let Some(scrut) = scrut_local {
                        self.lower_tuple_pattern(
                            scrut,
                            scrut_ty,
                            elements,
                            match_target,
                            next_block,
                        )?;
                    } else {
                        self.set_terminator(Terminator::Goto(match_target));
                    }
                }
                HirPattern::Literal { expr: pat_expr, .. } => {
                    // Compare scrutinee to literal.
                    if let Some(scrut) = scrut_local {
                        let lit_local = self.lower_expr(pat_expr)?;
                        let cmp_dest = self.new_temp(Ty::Bool);
                        self.emit(MirInst::Compare {
                            dest: cmp_dest,
                            op: CmpOp::Eq,
                            lhs: MirValue::Use(scrut),
                            rhs: local_to_value(lit_local),
                        });
                        self.set_terminator(Terminator::Branch {
                            cond: MirValue::Use(cmp_dest),
                            then_block: match_target,
                            else_block: next_block,
                        });
                    } else {
                        self.set_terminator(Terminator::Goto(match_target));
                    }
                }
                _ => {
                    // Other patterns — fallthrough to body for now.
                    self.set_terminator(Terminator::Goto(match_target));
                }
            }

            // Evaluate the guard, if any, in the intermediate block.
            // Pattern bindings introduced above are already registered
            // in `def_to_local`, so the guard can reference them.
            if let Some(guard_expr) = &arm.guard {
                self.current_block = match_target;
                let guard_local = self.lower_expr(guard_expr)?;
                let guard_val = local_to_value(guard_local);
                self.set_terminator(Terminator::Branch {
                    cond: guard_val,
                    then_block: arm_body_block,
                    else_block: next_block,
                });
            }

            // Lower arm body.
            self.current_block = arm_body_block;
            let body_result = self.lower_expr(&arm.body)?;
            if matches!(self.get_terminator(), Terminator::Unreachable) {
                if let Some(dest) = result_local {
                    let val = local_to_value(body_result);
                    self.emit(MirInst::Assign { dest, value: val });
                }
                self.set_terminator(Terminator::Goto(merge_block));
            }

            if !is_last {
                self.current_block = next_block;
            }
        }
        Ok(())
    }

    fn lower_or_pattern(
        &mut self,
        scrut_local: Option<LocalId>,
        _scrut_ty: &Ty,
        patterns: &[HirPattern],
        match_target: BlockId,
        next_block: BlockId,
    ) -> Result<(), String> {
        let scrut = match scrut_local {
            Some(s) => s,
            None => {
                self.set_terminator(Terminator::Goto(match_target));
                return Ok(());
            }
        };
        for (i, pat) in patterns.iter().enumerate() {
            let is_last = i + 1 == patterns.len();
            let fail_block = if is_last {
                next_block
            } else {
                self.new_block()
            };
            match pat {
                HirPattern::Wildcard { .. } => {
                    self.set_terminator(Terminator::Goto(match_target));
                    return Ok(());
                }
                HirPattern::Literal { expr: pat_expr, .. } => {
                    let lit_local = self.lower_expr(pat_expr)?;
                    let cmp_dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: cmp_dest,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(scrut),
                        rhs: local_to_value(lit_local),
                    });
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(cmp_dest),
                        then_block: match_target,
                        else_block: fail_block,
                    });
                }
                _ => {
                    self.set_terminator(Terminator::Goto(match_target));
                    return Ok(());
                }
            }
            if !is_last {
                self.current_block = fail_block;
            }
        }
        Ok(())
    }

    /// Lower a tuple pattern by comparing literal elements and binding
    /// non-literal elements to the corresponding tuple field.
    fn lower_tuple_pattern(
        &mut self,
        scrut: LocalId,
        scrut_ty: &Ty,
        elements: &[HirPattern],
        match_target: BlockId,
        next_block: BlockId,
    ) -> Result<(), String> {
        let elem_tys: Vec<Ty> = match scrut_ty {
            Ty::Tuple(ts) => ts.clone(),
            _ => {
                self.set_terminator(Terminator::Goto(match_target));
                return Ok(());
            }
        };
        for (idx, pat) in elements.iter().enumerate() {
            let elem_ty = elem_tys.get(idx).cloned().unwrap_or(Ty::Unit);
            let elem_local = self.new_temp(elem_ty.clone());
            self.emit(MirInst::GetField {
                dest: elem_local,
                base: scrut,
                field_index: idx,
            });
            match pat {
                HirPattern::Wildcard { .. } => {}
                HirPattern::Binding {
                    def_id,
                    name,
                    mutable,
                    ..
                } => {
                    let local = self.new_local_named(name, elem_ty, *mutable);
                    self.def_to_local.insert(*def_id, local);
                    self.emit(MirInst::Assign {
                        dest: local,
                        value: MirValue::Use(elem_local),
                    });
                }
                HirPattern::Literal { expr: pat_expr, .. } => {
                    let lit_local = self.lower_expr(pat_expr)?;
                    let cmp_dest = self.new_temp(Ty::Bool);
                    self.emit(MirInst::Compare {
                        dest: cmp_dest,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(elem_local),
                        rhs: local_to_value(lit_local),
                    });
                    let ok_block = self.new_block();
                    self.set_terminator(Terminator::Branch {
                        cond: MirValue::Use(cmp_dest),
                        then_block: ok_block,
                        else_block: next_block,
                    });
                    self.current_block = ok_block;
                }
                _ => {
                    // Unsupported nested patterns: fall through to match.
                }
            }
        }
        self.set_terminator(Terminator::Goto(match_target));
        Ok(())
    }
}
