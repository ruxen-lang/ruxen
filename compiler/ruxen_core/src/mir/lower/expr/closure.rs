use super::super::*;

impl<'a> Lowerer<'a> {
    pub(super) fn lower_closure(&mut self, expr: &HirExpr) -> Result<Option<LocalId>, String> {
        match &expr.kind {
            // ── Closure ─────────────────────────────────────────────
            HirExprKind::Closure {
                params,
                body,
                is_move,
                kind: closure_kind,
                ..
            } => {
                // Closure layout: the four-word pair documented on
                // `drops::CLOSURE_PAIR_SIZE` — fn_ptr, captures_ptr (one
                // 8-byte slot per capture, NULL when nothing is captured),
                // this literal's drop glue, and the extra-owner count.
                //
                // Each capture slot holds either the value directly
                // (ByValue — move or Copy) or a pointer to a single-slot
                // heap cell shared with the enclosing frame (ByRef —
                // used for `let mut` variables the closure mutates).
                let closure_name = format!("__closure_{}", self.closure_counter);
                self.closure_counter += 1;

                // Collect captured def_ids by walking the body.  A def is
                // captured when it is referenced but not defined inside
                // the closure body or declared as a closure parameter.
                //
                // The set of defs VISIBLE in the enclosing frame is every
                // def `lower_var_ref` could resolve there: an outer-frame
                // local (`def_to_local`) OR — when this closure literal is
                // itself nested inside another closure's body — a capture of
                // that enclosing closure (`capture_map`). Without the
                // `capture_map` half, a closure nested inside a closure body
                // (e.g. `outer.build({ |c| c.add({ || v + 1 }) })`, where
                // `v` is captured by the OUTER block) would not re-capture
                // `v`; the nested closure would read `v` as slot garbage.
                // (Q26.)
                let param_def_ids: HashSet<DefId> = params.iter().map(|p| p.def_id).collect();
                let visible_defs: HashSet<DefId> = self
                    .def_to_local
                    .keys()
                    .copied()
                    .chain(self.capture_map.keys().copied())
                    .collect();
                let mut captured_def_ids: Vec<DefId> = Vec::new();
                let mut seen: HashSet<DefId> = HashSet::new();
                collect_captures(
                    body,
                    &param_def_ids,
                    &visible_defs,
                    &mut captured_def_ids,
                    &mut seen,
                );

                // Decide storage kind per capture.  Copy-typed values
                // can always be captured by value; moved/Copy values go
                // inline; non-move captures of a mutable local that is
                // assigned inside the closure body go through a cell.
                //
                // A capture resolves through one of two enclosing-frame
                // mechanisms — exactly the two `lower_var_ref` consults:
                //   • `Local`     — an outer-frame local (`def_to_local`).
                //   • `Recapture` — a capture of the ENCLOSING closure
                //                   (`capture_map`), i.e. this closure
                //                   literal is nested inside another
                //                   closure's body and re-captures one of
                //                   the outer block's captures (Q26).
                //
                // A block borrows each String or scalar it captures by its
                // address (`ByRef` onto the enclosing variable's own slot,
                // no heap cell): a write, a `&var` or a `def var` call in the
                // body then reaches the variable itself, and the enclosing
                // scope keeps the one owner (drop.spec B12).
                //
                // A proc or lambda owns a String it captures (the enclosing
                // variable moved in): a write, a `&var` or a `def var` call
                // in the body replaces the value in the closure's own slot,
                // which the closure's drop frees (no shared cell).
                let owns_captures =
                    *is_move || *closure_kind != crate::parser::ast::ClosureKind::Block;
                let mut slots: Vec<(DefId, CaptureSource, Ty, CaptureKind)> =
                    Vec::with_capacity(captured_def_ids.len());
                let mut address_slots: HashSet<DefId> = HashSet::new();
                let mut own_slots: HashSet<DefId> = HashSet::new();
                for def in &captured_def_ids {
                    let mutates = closure_body_mutates(body, *def);
                    let live_ty = self.symbols.def_ty(*def);
                    let slot_addressed = live_ty
                        .as_ref()
                        .is_some_and(crate::mir::refmut_abi::refmut_is_slot_address);
                    let copy = live_ty.as_ref().is_some_and(|ty| {
                        crate::resolve::symbols::ty_is_effectively_copy(ty, self.symbols)
                    });
                    let by_address = !owns_captures && slot_addressed;
                    let in_own_slot = owns_captures && slot_addressed && !copy;
                    if by_address {
                        address_slots.insert(*def);
                    }
                    if in_own_slot {
                        own_slots.insert(*def);
                    }
                    let want_byref = !*is_move && ((mutates && !in_own_slot) || by_address);

                    if let Some(&local) = self.def_to_local.get(def) {
                        let ty = self.fn_mut().locals[local as usize].ty.clone();
                        let kind = if want_byref {
                            CaptureKind::ByRef
                        } else {
                            CaptureKind::ByValue
                        };
                        slots.push((*def, CaptureSource::Local(local), ty, kind));
                    } else if let Some(enclosing) = self.capture_map.get(def).copied() {
                        // Re-capture from the enclosing closure's captures.
                        // Every capture slot is an 8-byte word (value, or a
                        // cell pointer for ByRef); the recorded `ty` is unused
                        // downstream, so `Ty::Int` (pointer width) suffices.
                        let ty = Ty::Int;
                        // A nested closure that MUTATES an outer-captured
                        // value needs a shared cell. We can only propagate a
                        // cell that already exists (the enclosing capture was
                        // itself ByRef). Mutating a ByValue outer capture from
                        // a doubly-nested closure would require promoting the
                        // enclosing capture to a cell after the fact, which is
                        // not supported — reject rather than miscompile.
                        let kind = if want_byref {
                            if enclosing.kind == CaptureKind::ByRef || by_address {
                                CaptureKind::ByRef
                            } else {
                                return Err(format!(
                                    "closure nested inside another closure mutates an \
                                     outer-captured value `{}` that is captured by value; \
                                     this re-capture-and-mutate shape is not supported",
                                    def_id_name(*def, self.symbols)
                                ));
                            }
                        } else {
                            CaptureKind::ByValue
                        };
                        slots.push((*def, CaptureSource::Recapture(enclosing), ty, kind));
                    } else {
                        return Err(format!(
                            "closure captures `{}` which is not resolvable in the \
                             enclosing frame (neither a local nor an outer capture)",
                            def_id_name(*def, self.symbols)
                        ));
                    }
                }

                // Cell-promote any captured `let mut` that will be shared
                // by-reference: load the current value into a fresh cell,
                // then rewrite the outer local to hold the cell pointer.
                // From this point on, reads/writes to the outer local go
                // through the cell (see `cell_promoted`).  We only do
                // this once per local — if it's already been promoted by
                // a previous closure in the same function, reuse it.
                // (Recapture sources reuse the enclosing closure's existing
                // cell, so they need no promotion here.)
                for (def, source, _ty, kind) in &slots {
                    let outer_local = match source {
                        CaptureSource::Local(l) => *l,
                        CaptureSource::Recapture(_) => continue,
                    };
                    if *kind == CaptureKind::ByRef
                        && !self.cell_promoted.contains(def)
                        && !address_slots.contains(def)
                    {
                        let cell = self.new_temp(Ty::Int);
                        self.emit(MirInst::Alloc {
                            dest: cell,
                            ty: Ty::Int,
                            size: 8,
                        });
                        // Store the current value of the local into the cell.
                        self.emit(MirInst::SetField {
                            base: cell,
                            field_index: 0,
                            value: MirValue::Use(outer_local),
                        });
                        // Rewrite the outer local to hold the cell pointer.
                        self.emit(MirInst::Assign {
                            dest: outer_local,
                            value: MirValue::Use(cell),
                        });
                        // The local now holds an 8-byte cell POINTER, not the
                        // original value — retype it to `Ty::Int` so codegen
                        // treats it as a 64-bit pointer base for the
                        // `GetField`/`SetField` cell accesses. Without this, a
                        // promoted `Bool` (i8) / `Char` (i32) local keeps its
                        // narrow type and the cell deref lowers to a
                        // `load.i8`/`i32` from a non-pointer-width value
                        // (cranelift verifier: "invalid pointer width"). Only
                        // bites when the capturing closure is a REAL value
                        // (e.g. passed to `Set_each`/`Hash_each`); the inlined
                        // combinator path never promotes, which is why it
                        // surfaced only with Set/Hash `include Enumerable`.
                        if let Some(f) = self.current_fn.as_mut() {
                            if let Some(slot) = f.locals.get_mut(outer_local as usize) {
                                slot.ty = Ty::Int;
                            }
                        }
                        self.cell_promoted.insert(*def);
                    }
                }

                // Allocate the captures struct (or NULL if no captures).
                let captures_ptr = if slots.is_empty() {
                    None
                } else {
                    let cap = self.new_temp(Ty::Int);
                    let size = (slots.len() * 8).max(8);
                    self.emit(MirInst::Alloc {
                        dest: cap,
                        ty: Ty::Int,
                        size,
                    });
                    // Snapshot (source, kind, def) so the borrow of `slots`
                    // ends before we emit (emit takes `&mut self`).
                    let fill: Vec<(usize, DefId, CaptureSource, CaptureKind)> = slots
                        .iter()
                        .enumerate()
                        .map(|(i, (d, src, _ty, k))| (i, *d, *src, *k))
                        .collect();
                    for (slot_idx, def, source, kind) in fill {
                        let src_val = match (source, kind) {
                            // ── Outer-frame local ──────────────────────
                            (CaptureSource::Local(outer_local), CaptureKind::ByValue) => {
                                // For already-cell-promoted defs, the outer
                                // local is a cell pointer — load the value
                                // out of the cell before storing.  (Niche:
                                // a ByValue capture of a local promoted by an
                                // earlier closure in this frame.)
                                if self.refmut_slots.contains(&def) {
                                    // A slot-addressed `&var T`: load exactly
                                    // a `T` (a `Bool` slot is one byte) and
                                    // widen it into the capture word.
                                    let pointee = self.fn_ref().locals[outer_local as usize]
                                        .ty
                                        .peel_refs()
                                        .clone();
                                    MirValue::Use(self.load_through_refmut(
                                        outer_local,
                                        &pointee,
                                        &Ty::Int,
                                    ))
                                } else if self.cell_promoted.contains(&def) {
                                    let tmp = self.new_temp(Ty::Int);
                                    self.emit(MirInst::GetField {
                                        dest: tmp,
                                        base: outer_local,
                                        field_index: 0,
                                    });
                                    MirValue::Use(tmp)
                                } else if *closure_kind == crate::parser::ast::ClosureKind::Block {
                                    // A block BORROWS its captures: store
                                    // through a `Ref` so the drop pass does
                                    // not see the outer local stored into an
                                    // aggregate. The enclosing scope keeps
                                    // ownership and frees it once; the
                                    // block's drop frees no capture.
                                    let ty = self.fn_mut().locals[outer_local as usize].ty.clone();
                                    let borrow = self.new_temp(Ty::Ref(Box::new(ty)));
                                    self.emit(MirInst::Ref {
                                        dest: borrow,
                                        src: outer_local,
                                    });
                                    MirValue::Use(borrow)
                                } else {
                                    MirValue::Use(outer_local)
                                }
                            }
                            // A block's String or scalar: the address of
                            // the enclosing variable's storage (a plain
                            // local's slot, or the address a cell or `&var`
                            // already holds).
                            (CaptureSource::Local(_), CaptureKind::ByRef)
                                if address_slots.contains(&def) =>
                            {
                                local_to_value(self.var_storage_addr(def)?)
                            }
                            (CaptureSource::Local(outer_local), CaptureKind::ByRef) => {
                                // Outer local already holds the cell pointer
                                // (promoted above). Copy the pointer through.
                                MirValue::Use(outer_local)
                            }
                            // ── Re-capture from the enclosing closure ───
                            // We are still lowering inside the enclosing
                            // closure, so `self.captures_ptr_local` is the
                            // ENCLOSING captures pointer. (Q26.)
                            (CaptureSource::Recapture(encl), nested_kind) => {
                                let encl_cap = self.captures_ptr_local.expect(
                                    "re-capture implies the enclosing closure has a captures ptr",
                                );
                                match (encl.kind, nested_kind) {
                                    // Enclosing ByValue → its slot holds the
                                    // value directly. The nested closure
                                    // BORROWS it (its drop frees no
                                    // re-capture): store it through a `Ref`
                                    // so the enclosing closure's slot stays
                                    // owned and its drop frees the value
                                    // once. A nested pair that escapes the
                                    // enclosing body gives the slot up
                                    // instead (`NestedRecapture`).
                                    (CaptureKind::ByValue, CaptureKind::ByValue) => {
                                        let tmp = self.new_temp(Ty::Int);
                                        self.emit(MirInst::GetField {
                                            dest: tmp,
                                            base: encl_cap,
                                            field_index: encl.slot_index,
                                        });
                                        let borrow = self.new_temp(Ty::Ref(Box::new(Ty::Int)));
                                        self.emit(MirInst::Ref {
                                            dest: borrow,
                                            src: tmp,
                                        });
                                        MirValue::Use(borrow)
                                    }
                                    // A block nested in an owning closure
                                    // borrows that closure's own slot by
                                    // address.
                                    (CaptureKind::ByValue, CaptureKind::ByRef) => {
                                        let addr = self.new_temp(Ty::RefMut(Box::new(Ty::Int)));
                                        self.emit(MirInst::FieldAddr {
                                            dest: addr,
                                            base: encl_cap,
                                            field_index: encl.slot_index,
                                        });
                                        MirValue::Use(addr)
                                    }
                                    // Enclosing ByRef, nested ByRef → share
                                    // the same cell: propagate the cell ptr.
                                    (CaptureKind::ByRef, CaptureKind::ByRef) => {
                                        let cell = self.new_temp(Ty::Int);
                                        self.emit(MirInst::GetField {
                                            dest: cell,
                                            base: encl_cap,
                                            field_index: encl.slot_index,
                                        });
                                        MirValue::Use(cell)
                                    }
                                    // Enclosing ByRef, nested ByValue → read
                                    // the current value through the cell.
                                    (CaptureKind::ByRef, CaptureKind::ByValue) => {
                                        let cell = self.new_temp(Ty::Int);
                                        self.emit(MirInst::GetField {
                                            dest: cell,
                                            base: encl_cap,
                                            field_index: encl.slot_index,
                                        });
                                        let tmp = self.new_temp(Ty::Int);
                                        self.emit(MirInst::GetField {
                                            dest: tmp,
                                            base: cell,
                                            field_index: 0,
                                        });
                                        MirValue::Use(tmp)
                                    }
                                }
                            }
                        };
                        self.emit(MirInst::SetField {
                            base: cap,
                            field_index: slot_idx,
                            value: src_val,
                        });
                    }
                    Some(cap)
                };

                // Build the synthesized closure function.  First parameter
                // is the captures pointer (may be NULL for no captures).
                //
                // The MIR return type comes from the closure's `Ty::Fn` (set by
                // typeck), NOT from `body.ty`: when the body diverges on every
                // path (e.g. all branches exit via `next` — Block Tier-2), its
                // tail type is `Never`, yet the closure genuinely returns the
                // `next` values' type (which typeck folded into `expr.ty`).
                // Using `body.ty` there would declare a `Never`-returning
                // function that a `next expr` then `Return`s a real value from,
                // tripping the codegen verifier.
                let ret_ty = match &expr.ty {
                    Ty::Fn { ret, .. } => (**ret).clone(),
                    _ => body.ty.clone(),
                };

                // ── The closure word-ABI (B01) ───────────────────────────
                //
                // A closure is reached ONLY through the erased `Fn` box:
                // `{fn_ptr, captures_ptr}`, invoked by `CallIndirect`. The
                // generic body that invokes it (`Array_map`, `Hash_each`, …)
                // was lowered ONCE with the element type abstract, so its
                // argument and result locals are `TypeParam` — 64-bit
                // uniform WORDS. It has no way to know a particular closure
                // takes an `f64`.
                //
                // Every non-float type survives that: `Bool`/`Char`/narrow
                // ints share the integer register class with the word, so a
                // width mismatch is harmless. A FLOAT does not — it is
                // passed in a vector register, so the callee read a
                // register the caller never wrote, which is why
                // `[2.5].map { |v| v + 0.5 }` produced `0`.
                //
                // So a closure's float parameters and float return use the
                // uniform word, and the body reinterprets at the edges:
                // `BitsToFloat` on entry, `FloatToBits` on the way out. The
                // matching bitcasts on the CALLER's side live in
                // `method_call.rs`, at the `Fn`-box `CallIndirect`. Both
                // sides must move together — one alone is worse than
                // neither.
                let ret_is_float = crate::mir::abi::is_float_ty(&ret_ty);
                let abi_ret_ty = if ret_is_float {
                    Ty::Int
                } else {
                    ret_ty.clone()
                };
                let mut closure_fn = MirFunction::new(&closure_name, abi_ret_ty);
                closure_fn.role = MirFnRole::ClosureBody;
                let cap_param = closure_fn.new_local("__captures".to_string(), Ty::Int, false);
                closure_fn.params.push(cap_param);
                // `closure_param_ids` are what the BODY sees (always the
                // declared type). `float_param_words` records the
                // (word_param, float_local) pairs that need a `BitsToFloat`
                // emitted at the top of the body.
                let mut closure_param_ids: Vec<LocalId> = Vec::with_capacity(params.len());
                let mut float_param_words: Vec<(LocalId, LocalId)> = Vec::new();
                for param in params {
                    if crate::mir::abi::is_float_ty(&param.ty) {
                        let word_id =
                            closure_fn.new_local(format!("{}__word", param.name), Ty::Int, false);
                        closure_fn.params.push(word_id);
                        let float_id =
                            closure_fn.new_local(param.name.clone(), param.ty.clone(), false);
                        float_param_words.push((word_id, float_id));
                        closure_param_ids.push(float_id);
                    } else {
                        let local_id =
                            closure_fn.new_local(param.name.clone(), param.ty.clone(), false);
                        closure_fn.params.push(local_id);
                        closure_param_ids.push(local_id);
                    }
                }
                // The captures box is the pair's, lent to the body; each
                // parameter is taken as its declared type says.
                closure_fn.param_modes =
                    std::iter::once(ParamMode::Read)
                        .chain(params.iter().map(|param| {
                            crate::mir::param_modes::param_mode(&param.ty, self.symbols)
                        }))
                        .collect();

                // Save the current lowerer state, lower the closure body
                // in the context of the new function, then restore.
                let saved_fn = self.current_fn.take();
                let saved_block = self.current_block;
                let saved_defs = self.def_to_local.clone();
                let saved_capture_map = std::mem::take(&mut self.capture_map);
                let saved_captures_ptr = self.captures_ptr_local.take();
                let saved_cell_promoted = std::mem::take(&mut self.cell_promoted);
                let saved_nested_recaptures = std::mem::take(&mut self.nested_recaptures);
                // A slot-addressed `&var T` captured by reference stays one
                // inside the body: its cell pointer is the caller's address.
                // So does a String or scalar a block borrows by address, and
                // a String a proc or lambda owns in its own slot (its
                // address is the slot, `var_storage_addr`).
                let body_refmut_slots: HashSet<DefId> = slots
                    .iter()
                    .filter(|(def, _, _, kind)| {
                        (*kind == CaptureKind::ByRef
                            && (self.refmut_slots.contains(def) || address_slots.contains(def)))
                            || (*kind == CaptureKind::ByValue && own_slots.contains(def))
                    })
                    .map(|(def, ..)| *def)
                    .collect();
                let saved_refmut_slots =
                    std::mem::replace(&mut self.refmut_slots, body_refmut_slots);
                let saved_stmt_log = std::mem::take(&mut self.stmt_log);

                self.current_fn = Some(closure_fn);
                self.current_block = 0;
                self.captures_ptr_local = if slots.is_empty() {
                    None
                } else {
                    Some(cap_param)
                };

                // Clear def_to_local: only closure params (and captures
                // via the capture map) should be visible inside the body.
                self.def_to_local.clear();
                // The body tracks its own heap locals; the enclosing
                // function's set comes back after it, or every rebind after
                // the closure literal would skip freeing the old object.
                let saved_heap_locals = std::mem::take(&mut self.initialized_heap_locals);
                for (i, param) in params.iter().enumerate() {
                    self.def_to_local.insert(param.def_id, closure_param_ids[i]);
                }
                // Populate the capture map.  ByRef captures are visible
                // as cell-promoted defs inside the closure body too — any
                // read/write on them goes through the cell.
                for (slot_idx, (def, _outer, _ty, kind)) in slots.iter().enumerate() {
                    self.capture_map.insert(
                        *def,
                        CaptureSlot {
                            slot_index: slot_idx,
                            kind: *kind,
                            owns: owns_captures,
                        },
                    );
                    if *kind == CaptureKind::ByRef {
                        self.cell_promoted.insert(*def);
                    }
                }

                // Closure word-ABI, INBOUND edge: recover each float
                // parameter from the uniform word it arrived in, before any
                // body instruction can read it.
                for &(word_id, float_id) in &float_param_words {
                    self.emit(MirInst::BitsToFloat {
                        dest: float_id,
                        src: word_id,
                    });
                }

                // Lower the closure body.
                let body_result = self.lower_expr(body)?;
                if matches!(body.ty, Ty::Never) {
                    // The body diverges on every path (all branches exit via
                    // `next` / `return`), so this fall-through is unreachable.
                    // Emitting `Return(None)` here would mismatch a non-`Unit`
                    // MIR return type (inferred from the `next` values). Leave
                    // the block unreachable instead. (Block Tier-2.)
                    self.set_terminator(Terminator::Unreachable);
                } else if body_result.is_some() && body.ty != Ty::Unit {
                    let body_val = local_to_value(body_result);
                    self.set_terminator(Terminator::Return(Some(body_val)));
                } else {
                    self.set_terminator(Terminator::Return(None));
                }

                // Extract the completed closure function.
                let mut completed_fn = self.current_fn.take().unwrap();
                let body_scopes = std::mem::replace(&mut self.stmt_log, saved_stmt_log).scopes;
                self.drop_pending
                    .insert(completed_fn.name.clone(), body_scopes);

                // Closure word-ABI, OUTBOUND edge: rewrite EVERY value
                // return to hand back the float's bit pattern in the
                // uniform word the signature now declares.
                //
                // Done as a post-pass over the finished body rather than at
                // the tail expression, deliberately: a closure can also
                // return from an early `return`, from a `next expr`, and
                // from each arm of a diverging body. Patching only the tail
                // would leave those paths returning a raw `f64` against an
                // integer-word signature — a silent register-class
                // mismatch, i.e. exactly the bug being fixed.
                if ret_is_float {
                    erase_float_returns_to_word(&mut completed_fn, &ret_ty);
                }
                // A block's captures are borrowed (see the capture fill
                // above), so its drop frees only the captures box and pair.
                let owning_slots: &[_] = if *closure_kind == crate::parser::ast::ClosureKind::Block
                {
                    &[]
                } else {
                    &slots
                };
                // The slots a nested closure borrowed and carried out of this
                // body: this closure no longer owns them.
                let nested =
                    std::mem::replace(&mut self.nested_recaptures, saved_nested_recaptures);
                let given_up: HashSet<usize> = nested
                    .iter()
                    .filter(|n| {
                        n.escapes
                            || closure_pair_escapes(
                                &completed_fn,
                                n.pair,
                                self.symbols,
                                &self.ffi_by_value,
                            )
                    })
                    .map(|n| n.slot)
                    .collect();
                let (closure_drop, owned_capture_slots) = self.synthesize_closure_drop(
                    &closure_name,
                    owning_slots,
                    &completed_fn,
                    cap_param,
                    &given_up,
                );
                let closure_drop_name = closure_drop.name.clone();
                self.pending_closures.push(completed_fn);
                self.pending_closures.push(closure_drop);

                // Restore the parent function state.
                self.current_fn = saved_fn;
                self.current_block = saved_block;
                self.def_to_local = saved_defs;
                self.capture_map = saved_capture_map;
                self.captures_ptr_local = saved_captures_ptr;
                self.cell_promoted = saved_cell_promoted;
                self.refmut_slots = saved_refmut_slots;
                self.initialized_heap_locals = saved_heap_locals;

                // Build the closure pair {fn_ptr, captures_ptr, drop_fn,
                // shares}. The drop slot lets a field or collection that
                // owns the pair free it without knowing this literal.
                let fn_ptr = self.new_temp(Ty::Int);
                self.emit(MirInst::FuncAddr {
                    dest: fn_ptr,
                    func_name: closure_name.clone(),
                });
                let drop_ptr = self.new_temp(Ty::Int);
                self.emit(MirInst::FuncAddr {
                    dest: drop_ptr,
                    func_name: closure_drop_name.clone(),
                });
                let pair = self.new_temp(expr.ty.clone());
                self.emit(MirInst::Alloc {
                    dest: pair,
                    ty: expr.ty.clone(),
                    size: CLOSURE_PAIR_SIZE,
                });
                let cap_val = match captures_ptr {
                    Some(cp) => MirValue::Use(cp),
                    None => MirValue::Literal(Literal::Int(0)),
                };
                for (field_index, value) in [
                    (CLOSURE_PAIR_FN, MirValue::Use(fn_ptr)),
                    (CLOSURE_PAIR_CAPTURES, cap_val),
                    (CLOSURE_PAIR_DROP, MirValue::Use(drop_ptr)),
                    (CLOSURE_PAIR_SHARES, MirValue::Literal(Literal::Int(0))),
                ] {
                    self.emit(MirInst::SetField {
                        base: pair,
                        field_index,
                        value,
                    });
                }
                self.fn_mut().record_view(
                    pair,
                    View::Closure {
                        body: closure_name,
                        captures: captures_ptr,
                    },
                );
                self.fn_mut().closure_drops.push(MirClosureDrop {
                    local: pair,
                    drop_fn: closure_drop_name,
                    owned_capture_slots,
                });
                // Every by-value re-capture is a borrow of the enclosing
                // closure's slot (see the capture fill); tell the enclosing
                // closure which pair holds it.
                let carries_nested_escape = !given_up.is_empty();
                for (_def, source, _ty, kind) in &slots {
                    if let CaptureSource::Recapture(encl) = source {
                        if encl.kind == CaptureKind::ByValue && *kind == CaptureKind::ByValue {
                            self.nested_recaptures.push(NestedRecapture {
                                pair,
                                slot: encl.slot_index,
                                escapes: carries_nested_escape,
                            });
                        }
                    }
                }
                Ok(Some(pair))
            }

            // ── Tuple ───────────────────────────────────────────────
            _ => unreachable!("lower_closure: dispatched to wrong helper"),
        }
    }

    fn synthesize_closure_drop(
        &self,
        closure_name: &str,
        slots: &[(DefId, CaptureSource, Ty, CaptureKind)],
        closure_fn: &MirFunction,
        captures_param: LocalId,
        given_up: &HashSet<usize>,
    ) -> (MirFunction, Vec<(usize, LocalId)>) {
        let mut owned_fields: Vec<(usize, Ty, LocalId)> = slots
            .iter()
            .enumerate()
            .filter_map(|(slot, (_def, source, ty, kind))| {
                let CaptureSource::Local(source_local) = source else {
                    return None;
                };
                (*kind == CaptureKind::ByValue
                    && !given_up.contains(&slot)
                    && heap_free_callee(ty).is_some()
                    && capture_slot_still_owned(
                        closure_fn,
                        captures_param,
                        slot,
                        self.symbols,
                        &self.ffi_by_value,
                    ))
                .then(|| (slot, ty.clone(), *source_local))
            })
            .collect();
        let owned_capture_slots = owned_fields
            .iter()
            .map(|(slot, _, source)| (*slot, *source))
            .collect();

        let drop_name = format!("{}_drop", closure_name);
        let mut drop_fn = MirFunction::new(&drop_name, Ty::Unit);
        let pair = drop_fn.new_local("closure", Ty::Int, false);
        drop_fn.params.push(pair);
        let has_pair = drop_fn.new_temp(Ty::Bool);
        drop_fn.blocks[drop_fn.entry_block]
            .instructions
            .push(MirInst::Compare {
                dest: has_pair,
                op: CmpOp::NotEq,
                lhs: MirValue::Use(pair),
                rhs: MirValue::Literal(Literal::Int(0)),
            });
        let shares_block = drop_fn.new_block();
        let return_block = drop_fn.new_block();
        drop_fn.blocks[drop_fn.entry_block].terminator = Terminator::Branch {
            cond: MirValue::Use(has_pair),
            then_block: shares_block,
            else_block: return_block,
        };
        // A shared pair (`.clone` of a Proc) loses one owner and stays
        // live; only the last owner frees it.
        let shares = drop_fn.new_temp(Ty::Int);
        let is_shared = drop_fn.new_temp(Ty::Bool);
        drop_fn.blocks[shares_block].instructions.extend([
            MirInst::GetField {
                dest: shares,
                base: pair,
                field_index: CLOSURE_PAIR_SHARES,
            },
            MirInst::Compare {
                dest: is_shared,
                op: CmpOp::NotEq,
                lhs: MirValue::Use(shares),
                rhs: MirValue::Literal(Literal::Int(0)),
            },
        ]);
        let unshare_block = drop_fn.new_block();
        let pair_live_block = drop_fn.new_block();
        drop_fn.blocks[shares_block].terminator = Terminator::Branch {
            cond: MirValue::Use(is_shared),
            then_block: unshare_block,
            else_block: pair_live_block,
        };
        let fewer = drop_fn.new_temp(Ty::Int);
        drop_fn.blocks[unshare_block].instructions.extend([
            MirInst::BinOp {
                dest: fewer,
                op: BinOp::Sub,
                lhs: MirValue::Use(shares),
                rhs: MirValue::Literal(Literal::Int(1)),
            },
            MirInst::SetField {
                base: pair,
                field_index: CLOSURE_PAIR_SHARES,
                value: MirValue::Use(fewer),
            },
        ]);
        drop_fn.blocks[unshare_block].terminator = Terminator::Goto(return_block);
        let captures = drop_fn.new_temp(Ty::Int);
        drop_fn.blocks[pair_live_block]
            .instructions
            .push(MirInst::GetField {
                dest: captures,
                base: pair,
                field_index: CLOSURE_PAIR_CAPTURES,
            });
        let has_captures = drop_fn.new_temp(Ty::Bool);
        drop_fn.blocks[pair_live_block]
            .instructions
            .push(MirInst::Compare {
                dest: has_captures,
                op: CmpOp::NotEq,
                lhs: MirValue::Use(captures),
                rhs: MirValue::Literal(Literal::Int(0)),
            });
        let captures_block = drop_fn.new_block();
        let pair_block = drop_fn.new_block();
        drop_fn.blocks[pair_live_block].terminator = Terminator::Branch {
            cond: MirValue::Use(has_captures),
            then_block: captures_block,
            else_block: pair_block,
        };

        let mut current = captures_block;
        owned_fields.reverse();
        for (slot, ty, _) in owned_fields {
            let captured = drop_fn.new_temp(ty.clone());
            drop_fn.blocks[current]
                .instructions
                .push(MirInst::GetField {
                    dest: captured,
                    base: captures,
                    field_index: slot,
                });
            let is_live = drop_fn.new_temp(Ty::Bool);
            drop_fn.blocks[current].instructions.push(MirInst::Compare {
                dest: is_live,
                op: CmpOp::NotEq,
                lhs: MirValue::Use(captured),
                rhs: MirValue::Literal(Literal::Int(0)),
            });
            let field_drop = drop_fn.new_block();
            let next = drop_fn.new_block();
            drop_fn.blocks[current].terminator = Terminator::Branch {
                cond: MirValue::Use(is_live),
                then_block: field_drop,
                else_block: next,
            };

            let mut user_drop_freed_storage = false;
            if let Ty::Class { name, .. } = &ty {
                if crate::mir::lower::drops::is_drop_class(
                    self.symbols,
                    &self.user_drop_classes,
                    name,
                ) {
                    let callee = self.resolve_ffi_alias_callee(format!("{}_drop", name));
                    user_drop_freed_storage = self.symbols.drop_consumes_self(name);
                    drop_fn.blocks[field_drop].instructions.push(MirInst::Call {
                        dest: None,
                        callee,
                        args: vec![MirValue::Use(captured)],
                    });
                }
            }
            if !user_drop_freed_storage {
                let free = heap_free_call(&mut drop_fn, self.symbols, &ty, captured)
                    .expect("owned closure capture must have a type-directed free helper");
                drop_fn.blocks[field_drop].instructions.extend(free);
            }
            drop_fn.blocks[field_drop].terminator = Terminator::Goto(next);
            current = next;
        }

        drop_fn.blocks[current].instructions.push(MirInst::Call {
            dest: None,
            callee: "ruxen_dealloc".to_string(),
            args: vec![MirValue::Use(captures)],
        });
        drop_fn.blocks[current].terminator = Terminator::Goto(pair_block);
        drop_fn.blocks[pair_block].instructions.push(MirInst::Call {
            dest: None,
            callee: "ruxen_dealloc".to_string(),
            args: vec![MirValue::Use(pair)],
        });
        drop_fn.blocks[pair_block].terminator = Terminator::Goto(return_block);
        drop_fn.blocks[return_block].terminator = Terminator::Return(None);
        (drop_fn, owned_capture_slots)
    }
}

/// Closure word-ABI (B01), outbound edge.
///
/// Rewrite every `Return(Some(v))` in a float-returning closure so the value
/// leaves as the IEEE-754 bit pattern in a 64-bit uniform word, matching the
/// signature `lower_closure` declared. `float_ty` is the closure's ORIGINAL
/// declared return type, used to give the intermediate temp the right float
/// width so `FloatToBits` reinterprets at that width.
///
/// The caller side of this contract is the `Fn`-box `CallIndirect` in
/// `method_call.rs`, which reads the word back with `BitsToFloat`. The two
/// must always ship together.
fn erase_float_returns_to_word(func: &mut MirFunction, float_ty: &Ty) {
    for block_idx in 0..func.blocks.len() {
        let ret_val = match &func.blocks[block_idx].terminator {
            Terminator::Return(Some(v)) => v.clone(),
            _ => continue,
        };

        // `FloatToBits` reads a LOCAL, so a literal (or any non-`Use`
        // operand) is materialised into a float temp first. Routing a
        // non-float operand through the same temp is deliberate: the
        // `Assign` numeric path promotes it, which is what `-> Float`
        // means for a body whose tail is an integer expression.
        let src_local = match ret_val {
            MirValue::Use(id) if func.locals[id as usize].ty == *float_ty => id,
            other => {
                let tmp = func.new_local("__ret_f".to_string(), float_ty.clone(), false);
                func.blocks[block_idx].instructions.push(MirInst::Assign {
                    dest: tmp,
                    value: other,
                });
                tmp
            }
        };

        let word = func.new_local("__ret_word".to_string(), Ty::Int, false);
        func.blocks[block_idx]
            .instructions
            .push(MirInst::FloatToBits {
                dest: word,
                src: src_local,
            });
        func.blocks[block_idx].terminator = Terminator::Return(Some(MirValue::Use(word)));
    }
}
