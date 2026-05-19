use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_constructors(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Construct (struct/class instantiation) ──────────────
            HirExprKind::Construct { fields, .. } => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest,
                    ty: expr.ty.clone(),
                    size: self.alloc_size(&expr.ty),
                });
                for (idx, (_name, field_expr)) in fields.iter().enumerate() {
                    let val_local = self.lower_expr(field_expr)?;
                    let val = local_to_value(val_local);
                    self.emit(MirInst::SetField {
                        base: dest,
                        field_index: idx,
                        value: val,
                    });
                }
                Ok(Some(dest))
            }

            // ── Enum variant construction ───────────────────────────
            HirExprKind::EnumVariant {
                variant_idx,
                fields,
                ..
            } => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest,
                    ty: expr.ty.clone(),
                    size: self.alloc_size(&expr.ty),
                });
                self.emit(MirInst::SetTag {
                    dest,
                    tag: *variant_idx as u32,
                });
                // For variants with data, get a pointer to the payload area
                // (offset 8 after the 4-byte tag + 4 bytes padding), then
                // store fields relative to the payload pointer.
                if !fields.is_empty() {
                    let payload_ptr = self.new_temp(expr.ty.clone());
                    self.emit(MirInst::GetPayload {
                        dest: payload_ptr,
                        src: dest,
                        ty: expr.ty.clone(),
                    });
                    for (idx, (_name, field_expr)) in fields.iter().enumerate() {
                        let val_local = self.lower_expr(field_expr)?;
                        let val = local_to_value(val_local);
                        self.emit(MirInst::SetField {
                            base: payload_ptr,
                            field_index: idx,
                            value: val,
                        });
                    }
                }
                Ok(Some(dest))
            }

            // ── Match ───────────────────────────────────────────────

            // ── Tuple ───────────────────────────────────────────────
            HirExprKind::Tuple(elems) => {
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest,
                    ty: expr.ty.clone(),
                    size: self.alloc_size(&expr.ty),
                });
                for (idx, elem) in elems.iter().enumerate() {
                    let val_local = self.lower_expr(elem)?;
                    let val = local_to_value(val_local);
                    self.emit(MirInst::SetField {
                        base: dest,
                        field_index: idx,
                        value: val,
                    });
                }
                Ok(Some(dest))
            }

            // ── Index ───────────────────────────────────────────────

            // allocated arrays still work.
            HirExprKind::ArrayLiteral(elems) => {
                if matches!(expr.ty, Ty::Array(_)) {
                    let arr_ty = expr.ty.clone();
                    let dest = self.new_temp(arr_ty.clone());
                    // #06.8 T#14: route both `_new` and `_push` callees
                    // through the FFI alias map so the migrated
                    // `class Array do lib ... end end` entries reach
                    // their C symbols. The naive mangled forms here
                    // carry the surface generic args (`Array[Int]_new`,
                    // `Array[Int]_push`); `resolve_ffi_alias_callee`
                    // strips the `[...]` segment and falls back to the
                    // parent-name-keyed alias (`Array_new`,
                    // `Array_push`). Without this rewrite the array
                    // literal emits the raw mangled callee and the
                    // linker fails to find `_Array[Int]_new`.
                    let new_name = self.resolve_ffi_alias_callee(format!(
                        "{}_new",
                        type_name_from_ty(&arr_ty)
                    ));
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: new_name,
                        args: vec![],
                    });
                    let push_name = self.resolve_ffi_alias_callee(format!(
                        "{}_push",
                        type_name_from_ty(&arr_ty)
                    ));
                    for elem in elems {
                        let val_local = self.lower_expr(elem)?;
                        let val = local_to_value(val_local);
                        self.emit(MirInst::Call {
                            dest: None,
                            callee: push_name.clone(),
                            args: vec![MirValue::Use(dest), val],
                        });
                    }
                    return Ok(Some(dest));
                }
                let dest = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest,
                    ty: expr.ty.clone(),
                    size: self.alloc_size(&expr.ty),
                });
                for (idx, elem) in elems.iter().enumerate() {
                    let val_local = self.lower_expr(elem)?;
                    let val = local_to_value(val_local);
                    self.emit(MirInst::SetField {
                        base: dest,
                        field_index: idx,
                        value: val,
                    });
                }
                Ok(Some(dest))
            }

            // ── Map literal ──────────────────────────────────────────
            // `{ k => v, ... }` lowers like `map!{…}` did pre-spec-§10a:
            // construct an empty `Map[K, V]` and emit one `insert` call
            // per entry.
            HirExprKind::MapLiteral(entries) => {
                let map_ty = expr.ty.clone();
                let dest = self.new_temp(map_ty.clone());
                let new_name = format!("{}_new", type_name_from_ty(&map_ty));
                self.emit(MirInst::Call {
                    dest: Some(dest),
                    callee: new_name,
                    args: vec![],
                });
                let insert_name = format!("{}_insert", type_name_from_ty(&map_ty));
                for (k_expr, v_expr) in entries {
                    let k_local = self.lower_expr(k_expr)?;
                    let v_local = self.lower_expr(v_expr)?;
                    let k_val = local_to_value(k_local);
                    let v_val = local_to_value(v_local);
                    self.emit(MirInst::Call {
                        dest: None,
                        callee: insert_name.clone(),
                        args: vec![MirValue::Use(dest), k_val, v_val],
                    });
                }
                Ok(Some(dest))
            }

            // ── Macro calls (panic!, assert!, …) ─────────────────────
            _ => unreachable!("lower_constructors: dispatched to wrong helper"),
        }
    }
}
