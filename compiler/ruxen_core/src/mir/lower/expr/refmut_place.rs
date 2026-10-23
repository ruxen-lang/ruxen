//! Slot-addressed `&var T` places (`mir::refmut_abi`).
//!
//! A `&var T` of a String or a scalar is the ADDRESS of the caller's storage.
//! This module owns the three MIR shapes that rule needs:
//!
//! - the address of a place, which an argument passes for a `&var T`
//!   parameter and a `r = &var place` binding holds
//!   ([`Lowerer::lower_refmut_addr`]): always the place's own storage (a
//!   local's slot, a cell, a field slot, an Array element slot, a
//!   closure's own capture slot), never a copy stored back afterwards;
//! - the load of a `T` through such an address where its value is read
//!   ([`Lowerer::load_through_refmut`]);
//! - the store of a `T` through it ([`Lowerer::store_through_refmut`]).
//!
//! A loaded value is handed on as the same `&var T`-typed word every other
//! borrowed value uses (a float's word is its bit pattern), so the rest of
//! lowering reads it exactly as it reads a `&T`.

use super::super::*;

impl<'a> Lowerer<'a> {
    /// The address to pass for a slot-addressed `&var T` argument `arg`
    /// (or to hold in a `r = &var place` binding): the place's own storage,
    /// so a write through it (which frees the value it replaces) reaches
    /// the place itself.
    ///
    /// - `&var x` and a bare `x` (auto-borrow): the variable's storage
    ///   ([`Self::var_storage_addr`]).
    /// - A call whose result is a slot-addressed `&var T`
    ///   (`g.deref_mut`) passes that address unchanged.
    /// - A field: the address of its slot in the object (`FieldAddr`).
    /// - An Array element: the address of its own slot
    ///   (`ruxen_vec_slot_addr`).
    /// - A temporary (a call result, a literal) is evaluated into a fresh
    ///   slot whose address is passed; nothing else names it.
    pub(in crate::mir::lower) fn lower_refmut_addr(
        &mut self,
        arg: &HirExpr,
    ) -> Result<Option<LocalId>, String> {
        match &arg.kind {
            HirExprKind::Borrow {
                mutable: true,
                expr: inner,
            } => return self.lower_refmut_addr(inner),
            HirExprKind::VarRef(def_id) => {
                if let Some(addr) = self.var_storage_addr(*def_id)? {
                    return Ok(Some(addr));
                }
            }
            HirExprKind::FnCall { .. } | HirExprKind::MethodCall { .. }
                if crate::mir::refmut_abi::is_slot_refmut(&arg.ty) =>
            {
                self.want_refmut_addr = true;
                return self.lower_expr(arg);
            }
            // borrow_check rejects these (E1021) before lowering.
            _ if arg.is_unaddressable_var_place() => {
                return Err(format!(
                    "error[E1021]: `{}` has no address to pass to a `&var` parameter",
                    arg.ty
                ))
            }
            // A field passes the address of its slot in the object: the
            // callee's store replaces the field in place.
            HirExprKind::FieldAccess {
                object, field_idx, ..
            } => {
                let Some(base) = self.lower_expr(object)? else {
                    return Ok(None);
                };
                let field_index = *field_idx + self.class_field_shift_for_ty(&object.ty);
                let addr = self.new_temp(Ty::RefMut(Box::new(arg.ty.peel_refs().clone())));
                self.emit(MirInst::FieldAddr {
                    dest: addr,
                    base,
                    field_index,
                });
                return Ok(Some(addr));
            }
            // An Array element passes its own slot: the callee's store
            // replaces the element in place (`ruxen_vec_slot_addr`).
            HirExprKind::Index { object, index } => {
                return self.element_slot_addr(object, index, &arg.ty);
            }
            _ => {}
        }

        let Some(value) = self.lower_expr(arg)? else {
            return Ok(None);
        };
        let pointee = arg.ty.peel_refs().clone();
        let slot = self.new_temp(pointee.clone());
        self.emit(MirInst::Copy {
            dest: slot,
            src: value,
        });
        let addr = self.new_temp(Ty::RefMut(Box::new(pointee)));
        self.emit(MirInst::RefMut {
            dest: addr,
            src: slot,
        });
        Ok(Some(addr))
    }

    /// The address of the variable `def_id`'s own storage, or `None` when
    /// it has none in this frame:
    ///
    /// - a plain local is borrowed in place (`RefMut`, which gives it a
    ///   stack slot in codegen);
    /// - a `&var` parameter or binding, and a cell-promoted local, already
    ///   hold the address;
    /// - inside a closure, a capture held by reference holds the address
    ///   (a cell, or the enclosing variable's slot a block borrows); a
    ///   capture a proc or lambda owns lives in its own slot of the
    ///   captures box (`FieldAddr`).
    pub(in crate::mir::lower) fn var_storage_addr(
        &mut self,
        def_id: DefId,
    ) -> Result<Option<LocalId>, String> {
        if let Some(slot) = self.capture_map.get(&def_id).copied() {
            let cap_ptr = self
                .captures_ptr_local
                .expect("capture_map non-empty implies captures_ptr_local is set");
            let dest = match slot.kind {
                CaptureKind::ByRef => {
                    let cell = self.new_temp(Ty::Int);
                    self.emit(MirInst::GetField {
                        dest: cell,
                        base: cap_ptr,
                        field_index: slot.slot_index,
                    });
                    cell
                }
                CaptureKind::ByValue if slot.owns => {
                    let addr = self.new_temp(Ty::RefMut(Box::new(Ty::Int)));
                    self.emit(MirInst::FieldAddr {
                        dest: addr,
                        base: cap_ptr,
                        field_index: slot.slot_index,
                    });
                    addr
                }
                // A block borrows every String or scalar it captures by
                // address (`lower_closure`), so its by-value slots hold
                // handles, which a `&var` never takes the address of.
                CaptureKind::ByValue => {
                    return Err(format!(
                        "a block holds `{}` by value and cannot pass its address to a \
                         `&var` parameter",
                        def_id_name(def_id, self.symbols)
                    ))
                }
            };
            return Ok(Some(dest));
        }
        let Some(&local) = self.def_to_local.get(&def_id) else {
            return Ok(None);
        };
        if self.cell_promoted.contains(&def_id) {
            return Ok(Some(local));
        }
        let pointee = self.fn_ref().locals[local as usize].ty.clone();
        let addr = self.new_temp(Ty::RefMut(Box::new(pointee)));
        self.emit(MirInst::RefMut {
            dest: addr,
            src: local,
        });
        Ok(Some(addr))
    }

    /// The address of Array element `object[index]`'s own slot
    /// (`ruxen_vec_slot_addr`), as a `&var elem_ty`.
    pub(in crate::mir::lower) fn element_slot_addr(
        &mut self,
        object: &HirExpr,
        index: &HirExpr,
        elem_ty: &Ty,
    ) -> Result<Option<LocalId>, String> {
        let (Some(base), Some(index)) = (self.lower_expr(object)?, self.lower_expr(index)?) else {
            return Ok(None);
        };
        let addr = self.new_temp(Ty::RefMut(Box::new(elem_ty.peel_refs().clone())));
        self.emit(MirInst::Call {
            dest: Some(addr),
            callee: "ruxen_vec_slot_addr".to_string(),
            args: vec![MirValue::Use(base), MirValue::Use(index)],
        });
        Ok(Some(addr))
    }

    /// Load the `pointee` stored at the slot address `addr` and return it as
    /// a `word_ty` word (the `&var T`-typed value the rest of lowering reads).
    /// The load is `pointee` wide: a `Bool` slot is one byte.
    pub(in crate::mir::lower) fn load_through_refmut(
        &mut self,
        addr: LocalId,
        pointee: &Ty,
        word_ty: &Ty,
    ) -> LocalId {
        let value = self.new_temp(pointee.clone());
        self.emit(MirInst::GetField {
            dest: value,
            base: addr,
            field_index: 0,
        });
        if word_ty == pointee {
            return value;
        }
        let word = self.new_temp(word_ty.clone());
        self.emit(MirInst::Copy {
            dest: word,
            src: value,
        });
        word
    }

    /// Store `val` as a `pointee` at the slot address `addr`, narrowing or
    /// reinterpreting a borrowed word first so the store is exactly `pointee`
    /// wide and never writes past a one-byte `Bool` slot.
    pub(in crate::mir::lower) fn store_through_refmut(
        &mut self,
        addr: LocalId,
        pointee: &Ty,
        val: MirValue,
    ) {
        let value = match val {
            MirValue::Use(src) if self.fn_ref().locals[src as usize].ty != *pointee => {
                let narrowed = self.new_temp(pointee.clone());
                self.emit(MirInst::Copy {
                    dest: narrowed,
                    src,
                });
                MirValue::Use(narrowed)
            }
            other => other,
        };
        self.emit(MirInst::SetField {
            base: addr,
            field_index: 0,
            value,
        });
    }

    /// A call result typed as a slot-addressed `&var T` is an address; where
    /// its value is read, load the `T` through it.
    pub(in crate::mir::lower) fn load_slot_refmut_result(
        &mut self,
        result: Option<LocalId>,
    ) -> Option<LocalId> {
        let addr = result?;
        let word_ty = self.fn_ref().locals[addr as usize].ty.clone();
        match crate::mir::refmut_abi::slot_refmut_pointee(&word_ty) {
            Some(pointee) => {
                let pointee = pointee.clone();
                Some(self.load_through_refmut(addr, &pointee, &word_ty))
            }
            None => Some(addr),
        }
    }
}
