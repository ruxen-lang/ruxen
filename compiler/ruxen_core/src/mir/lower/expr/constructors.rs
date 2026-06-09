use super::super::*;

impl<'a> Lowerer<'a> {
    /// Re-materialise `val_local` at `field_ty`'s width when the two are a
    /// numeric pair that differ — e.g. a `Float` (f64) value flowing into a
    /// `Float32` field. The constructor stores each field with a width-blind
    /// `SetField` (it stores at the value's SSA width), and `GetField` loads
    /// at the field-binding's declared width; if a value's width and the
    /// slot's width disagree the slot reads garbage (an f64 store into a 4-byte
    /// f32 slot reads 0). Routing the value through a target-typed `Assign`
    /// invokes codegen's `coerce_value` (the `fdemote`/`fpromote`/`fcvt_*`
    /// path — the same one a `let`-bound `as Float32` cast and a `Float32`
    /// fn-param boundary already use), so the SSA value is the field's width
    /// BEFORE the store. A no-op when the types already match (the inline
    /// `120.5f32` literal path, which `Assign`-coerces to the temp upstream).
    /// Q28.
    pub(in crate::mir::lower) fn coerce_to_field_ty(
        &mut self,
        val_local: Option<LocalId>,
        field_ty: &Ty,
    ) -> Option<LocalId> {
        let Some(val_local) = val_local else {
            return None;
        };
        let is_numeric = |ty: &Ty| {
            matches!(
                ty,
                Ty::Int
                    | Ty::Int8
                    | Ty::Int16
                    | Ty::Int32
                    | Ty::Int64
                    | Ty::UInt
                    | Ty::UInt8
                    | Ty::UInt16
                    | Ty::UInt32
                    | Ty::UInt64
                    | Ty::ISize
                    | Ty::USize
                    | Ty::Float
                    | Ty::Float32
                    | Ty::Float64
            )
        };
        let Some(val_ty) = self
            .fn_ref()
            .locals
            .get(val_local as usize)
            .map(|l| l.ty.clone())
        else {
            return Some(val_local);
        };
        // Only re-materialise when both ends are numeric and the widths/kinds
        // actually differ. Non-numeric fields (heap pointers, nested structs)
        // are stored/loaded as raw 8-byte slots and must pass through unchanged.
        if val_ty == *field_ty || !is_numeric(&val_ty) || !is_numeric(field_ty) {
            return Some(val_local);
        }
        let dest = self.new_temp(field_ty.clone());
        self.emit(MirInst::Assign {
            dest,
            value: MirValue::Use(val_local),
        });
        Some(dest)
    }

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
                // Phase B-5: write the class_info_ptr header at slot 0
                // for classes that include any `dispatch runtime`
                // mixin. No-op for static-only-mixin classes and for
                // structs. See spec §B2/§B8.
                self.emit_class_info_init(&expr.ty, dest);
                // Phase B-4: shift declared field indices by the
                // class's header slot count so SetField writes land
                // past the class_info_ptr header. Returns 0 for
                // structs/static-only classes — existing flat layout.
                let shift = self.class_field_shift_for_ty(&expr.ty);
                // Declared field types in layout order, so a value can be
                // coerced to its slot's width before the width-blind SetField
                // store (Q28). Empty when the type_def isn't a known
                // struct/class — the get(idx) below then leaves values as-is.
                let field_tys = self.lookup_construct_field_types(&expr.ty);
                for (idx, (_name, field_expr)) in fields.iter().enumerate() {
                    let mut val_local = self.lower_expr(field_expr)?;
                    if let Some(field_ty) = field_tys.get(idx).cloned() {
                        val_local = self.coerce_to_field_ty(val_local, &field_ty);
                    }
                    let val = local_to_value(val_local);
                    self.emit(MirInst::SetField {
                        base: dest,
                        field_index: idx + shift,
                        value: val,
                    });
                }
                Ok(Some(dest))
            }

            // ── Enum variant construction ───────────────────────────
            HirExprKind::EnumVariant {
                type_def,
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
                    // Note: string-literal payloads are already promoted
                    // from `&str` to heap `String` by `lower_expr` ->
                    // `emit_owned_string_literal` (mir/lower/emit.rs:90),
                    // so the typeck-side payload coercion for
                    // `Option[String]` / `Result[String, _]` is
                    // sufficient at MIR time — no explicit wrap here.
                    // Pin: `docs/rondo_v1_blockers.md` B14.
                    // Coerce each payload value to the variant field's
                    // declared width before the width-blind SetField store, so
                    // a bare `Float`/`Float` local placed into a `Float32`
                    // payload is narrowed to f32 first (Q28).
                    let field_tys = self.lookup_variant_field_types(*type_def, *variant_idx);
                    for (idx, (_name, field_expr)) in fields.iter().enumerate() {
                        let mut val_local = self.lower_expr(field_expr)?;
                        if let Some(field_ty) = field_tys.get(idx).cloned() {
                            val_local = self.coerce_to_field_ty(val_local, &field_ty);
                        }
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
                // Element slot types from the tuple's own type, so an f64
                // element flowing into an f32 tuple slot is narrowed before
                // the width-blind store (Q28).
                let elem_tys: Vec<Ty> = match &expr.ty {
                    Ty::Tuple(tys) => tys.clone(),
                    _ => Vec::new(),
                };
                for (idx, elem) in elems.iter().enumerate() {
                    let mut val_local = self.lower_expr(elem)?;
                    if let Some(elem_ty) = elem_tys.get(idx).cloned() {
                        val_local = self.coerce_to_field_ty(val_local, &elem_ty);
                    }
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
                    let new_name = self
                        .resolve_ffi_alias_callee(format!("{}_new", type_name_from_ty(&arr_ty)));
                    self.emit(MirInst::Call {
                        dest: Some(dest),
                        callee: new_name,
                        args: vec![],
                    });
                    let push_name = self
                        .resolve_ffi_alias_callee(format!("{}_push", type_name_from_ty(&arr_ty)));
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
                // #06.8 T#15: route `_new` and `_insert` callees
                // through the FFI alias map (same as ArrayLiteral).
                // The mangled forms carry the surface generic
                // (`Map[&str, Int]_new`); `resolve_ffi_alias_callee`
                // strips the balanced `[...]` segment to match the
                // parent-name key `Map_new` registered by the
                // bootstrap `class Map` shell.
                let new_name =
                    self.resolve_ffi_alias_callee(format!("{}_new", type_name_from_ty(&map_ty)));
                self.emit(MirInst::Call {
                    dest: Some(dest),
                    callee: new_name,
                    args: vec![],
                });
                let insert_name =
                    self.resolve_ffi_alias_callee(format!("{}_insert", type_name_from_ty(&map_ty)));
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
