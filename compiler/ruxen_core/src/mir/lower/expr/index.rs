use super::super::*;

/// `vec[i]` lowers to `ruxen_vec_get_or_panic` for any receiver that is
/// a backing Vec — a genuine `Ty::Array`, a reference to one, OR the
/// `Ty::Class { name: "Array" | "Vec" }` shape that `self` carries
/// INSIDE a migrated `class Array[T]` stdlib method body (the FFI-shell
/// class, repr-identical to `RuxenVec*`). Without the latter, indexed
/// reads inside `.rx` combinator bodies (`self[i]`) silently no-op.
/// Mirrors `util::is_vec_or_iterator_type`'s receiver classification.
/// Shared with `assign.rs` (the `xs[i] = v` write path mirrors this read
/// path), so it is `pub(super)`.
pub(super) fn is_indexable_vec_ty(ty: &Ty) -> bool {
    match ty {
        Ty::Array(_) => true,
        Ty::Ref(inner) | Ty::RefMut(inner) => is_indexable_vec_ty(inner),
        Ty::Class { name, .. } => {
            let base = name.split('[').next().unwrap_or(name);
            matches!(base, "Vec" | "Array")
        }
        _ => false,
    }
}

impl<'a> Lowerer<'a> {
    pub(super) fn lower_index(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Index ───────────────────────────────────────────────
            HirExprKind::Index { object, index } => {
                // Fixed-size arrays `[T; N]` are laid out as N consecutive
                // 8-byte slots (the layout used by Alloc + SetField above).
                // When the index is a compile-time integer literal we can
                // lower `a[i]` to a direct `GetField { field_index: i }`.
                if matches!(object.ty, Ty::FixedArray(_, _)) {
                    if let HirExprKind::IntLiteral(n) = &index.kind {
                        let base_local = self.lower_expr(object)?;
                        if let Some(base) = base_local {
                            let dest = self.new_temp(expr.ty.clone());
                            self.emit(MirInst::GetField {
                                dest,
                                base,
                                field_index: *n as usize,
                            });
                            return Ok(Some(dest));
                        }
                    }
                }
                // ── Phase 2 stdlib batch 1 (#03): Vec[i] ──
                // Indexing a Vec at runtime panics on OOB with a
                // descriptive message ("index N out of range, len M").
                // The runtime fn returns the raw 64-bit slot; the
                // typeck-emitted result type pulls out the element T.
                if is_indexable_vec_ty(&object.ty) {
                    let base_local = self.lower_expr(object)?;
                    let idx_local = self.lower_expr(index)?;
                    let base_val = local_to_value(base_local);
                    let idx_val = local_to_value(idx_local);
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "ruxen_vec_get_or_panic".to_string(),
                        args: vec![base_val, idx_val],
                    });
                    return Ok(Some(dest));
                }
                // ── Phase 2 stdlib batch 3 (#04): HashMap[&K] ──
                // `m[k]` panics on missing keys via `ruxen_hash_index`
                // (mirrors `ruxen_vec_get_or_panic` for Vec). The
                // surface type is V (set in typeck::infer_index_ty);
                // runtime returns the raw 64-bit value slot.
                if matches!(object.ty, Ty::Map(_, _))
                    || matches!(
                        &object.ty,
                        Ty::Ref(inner) | Ty::RefMut(inner)
                            if matches!(inner.as_ref(), Ty::Map(_, _))
                    )
                {
                    let base_local = self.lower_expr(object)?;
                    let idx_local = self.lower_expr(index)?;
                    let base_val = local_to_value(base_local);
                    let idx_val = local_to_value(idx_local);
                    let dest = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: "ruxen_hash_index".to_string(),
                        args: vec![base_val, idx_val],
                    });
                    return Ok(Some(dest));
                }
                // ── Operator → method desugar (Task OP, Step 3) ──
                // `a[i]` → `a.[](i)` on a NOMINAL receiver (user/stdlib
                // class that defines `def []`). The builtin collection heads
                // (Array/Vec FFI-shell above, Map above, FixedArray above)
                // are already handled; this catches everything else nominal.
                fn peel(t: &Ty) -> &Ty {
                    match t {
                        Ty::Ref(i)
                        | Ty::RefMut(i)
                        | Ty::RefLifetime(_, i)
                        | Ty::RefMutLifetime(_, i) => peel(i),
                        _ => t,
                    }
                }
                if matches!(
                    peel(&object.ty),
                    Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
                ) {
                    let synthetic = HirExpr {
                        kind: HirExprKind::MethodCall {
                            object: Box::new((**object).clone()),
                            method: UNRESOLVED_DEF,
                            method_name: "[]".to_string(),
                            generic_args: vec![],
                            args: vec![(**index).clone()],
                            block: None,
                        },
                        ty: expr.ty.clone(),
                        span: expr.span.clone(),
                    };
                    return self.lower_method_call(&synthetic);
                }

                // Dynamic index / other collection kinds still need runtime
                // support; fall through as a no-op.
                let _ = (object, index);
                Ok(None)
            }

            // ── Cast ────────────────────────────────────────────────
            _ => unreachable!("lower_index: dispatched to wrong helper"),
        }
    }
}
