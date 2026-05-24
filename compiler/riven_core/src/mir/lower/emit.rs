use super::*;

impl<'a> Lowerer<'a> {
    /// Get a mutable reference to the current MIR function.
    pub(super) fn fn_mut(&mut self) -> &mut MirFunction {
        self.current_fn.as_mut().expect("no current function")
    }

    pub(super) fn fn_ref(&self) -> &MirFunction {
        self.current_fn.as_ref().expect("no current function")
    }

    /// Emit `riven_dealloc(L); L = 0` for each `L`, in the current
    /// block. Used at every loop-exit edge (break, continue, back-edge)
    /// to free heap allocations made inside the body. The zero-store
    /// after dealloc keeps `compute_dealloc_safe_locals` from re-
    /// emitting a function-end dealloc on a stale pointer, and means a
    /// later iteration that bypasses the `let` will dealloc NULL.
    pub(super) fn emit_dealloc_loop_locals(&mut self, locals: &[LocalId]) {
        for &local in locals {
            // If the local is a class with a user-declared `def drop`,
            // call that destructor FIRST so it sees the live allocation
            // (e.g. JoinHandle's drop needs to read tid + closure fields
            // to detach the spawned thread before the memory is freed).
            // Without this, a `let _h = Thread.spawn({...})` inside a
            // while loop body would dealloc the JoinHandle at the
            // back-edge before the spawned thread had a chance to read
            // its closure.fn_ptr — see rondo's docs/riven-issues.md
            // §W15.
            let drop_callee = self
                .fn_ref()
                .locals
                .iter()
                .find(|l| l.id == local)
                .and_then(|l| match &l.ty {
                    Ty::Class { name, .. } => {
                        let bare = self.user_drop_classes.contains(name);
                        let suffix = format!(".{}", name);
                        let qualified = self.user_drop_classes.iter().any(|q| q.ends_with(&suffix));
                        if bare || qualified {
                            let mangled = format!("{}_drop", name);
                            Some(self.resolve_ffi_alias_callee(mangled))
                        } else {
                            None
                        }
                    }
                    _ => None,
                });
            if let Some(callee) = drop_callee {
                self.emit(MirInst::Call {
                    dest: None,
                    callee,
                    args: vec![MirValue::Use(local)],
                });
            }
            self.emit(MirInst::Call {
                dest: None,
                callee: "riven_dealloc".to_string(),
                args: vec![MirValue::Use(local)],
            });
            self.emit(MirInst::Assign {
                dest: local,
                value: MirValue::Literal(Literal::Int(0)),
            });
        }
    }

    /// Insert `Assign L = 0` at the very top of the loop's body-entry
    /// block for every body-local. Runs once before the first iteration
    /// so a path that bypasses a `let` (e.g. inside a nested `if`)
    /// reaches its first dealloc with a NULL value.
    pub(super) fn prepend_zero_init_for_body_locals(&mut self, frame: &LoopFrame) {
        if frame.body_locals.is_empty() {
            return;
        }
        let block = &mut self.fn_mut().blocks[frame.body_entry_block];
        for (i, &local) in frame.body_locals.iter().enumerate() {
            block.instructions.insert(
                i,
                MirInst::Assign {
                    dest: local,
                    value: MirValue::Literal(Literal::Int(0)),
                },
            );
        }
    }

    /// Push an instruction onto the current basic block.
    pub(super) fn emit(&mut self, inst: MirInst) {
        if let MirInst::Call { callee, args, .. } = &inst {
            let moves_args = matches!(
                callee.as_str(),
                "riven_executor_spawn"
                    | "Task_spawn_raw"
                    | "Task.spawn_raw"
                    | "riven_thread_spawn"
                    | "Thread_spawn"
                    | "Thread_spawn_raw"
            );
            let init_moves_non_self_args = callee.ends_with("_init");
            if moves_args || init_moves_non_self_args {
                for (idx, arg) in args.iter().enumerate() {
                    if init_moves_non_self_args && idx == 0 {
                        continue;
                    }
                    if let MirValue::Use(local) = arg {
                        if init_moves_non_self_args {
                            let is_aggregate = self
                                .fn_ref()
                                .locals
                                .iter()
                                .find(|candidate| candidate.id == *local)
                                .map(|candidate| {
                                    matches!(
                                        candidate.ty,
                                        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
                                    )
                                })
                                .unwrap_or(false);
                            if !is_aggregate {
                                continue;
                            }
                        }
                        self.initialized_heap_locals.remove(local);
                        for frame in &mut self.loop_stack {
                            frame.body_locals.retain(|candidate| candidate != local);
                        }
                    }
                }
            }
        }
        let block_id = self.current_block;
        let func = self.current_fn.as_mut().expect("no current function");
        func.blocks[block_id].instructions.push(inst);
    }

    /// Emit a value-transfer instruction selected by the source's move
    /// semantics and the type's effective Copy-ness. Move-bound owned
    /// values become `MirInst::Move` (so drop-elaboration follows the
    /// LIFO order); Copy values become `MirInst::Copy`; everything else
    /// degrades to a plain `Assign`.
    pub(super) fn emit_transfer(
        &mut self,
        dest: LocalId,
        src: LocalId,
        ty: &Ty,
        semantics: MoveSemantics,
    ) {
        let inst = match semantics {
            MoveSemantics::Move if !ty_is_effectively_copy(ty, self.symbols) => {
                MirInst::Move { dest, src }
            }
            _ if ty_is_effectively_copy(ty, self.symbols) => MirInst::Copy { dest, src },
            _ => MirInst::Assign {
                dest,
                value: MirValue::Use(src),
            },
        };
        self.emit(inst);
    }

    /// Lower a string literal as a heap-owned `String`. The raw
    /// `MirInst::StringLiteral` produces a pointer into `.rodata`;
    /// dropping such a pointer would call `free()` on a static address.
    /// `riven_string_from` copies it to the heap so `String::drop` is
    /// safe on the result. (P0.7)
    pub(super) fn emit_owned_string_literal(&mut self, value: &str) -> LocalId {
        let raw = self.new_temp(Ty::Str);
        self.emit(MirInst::StringLiteral {
            dest: raw,
            value: value.to_string(),
        });
        let owned = self.new_temp(Ty::String);
        self.emit(MirInst::Call {
            dest: Some(owned),
            callee: "riven_string_from".to_string(),
            args: vec![MirValue::Use(raw)],
        });
        owned
    }

    /// Set the terminator of the current basic block.
    pub(super) fn set_terminator(&mut self, term: Terminator) {
        let block_id = self.current_block;
        let func = self.current_fn.as_mut().expect("no current function");
        func.blocks[block_id].terminator = term;
    }

    /// Read the terminator of the current basic block.
    pub(super) fn get_terminator(&self) -> &Terminator {
        let block_id = self.current_block;
        let func = self.current_fn.as_ref().expect("no current function");
        &func.blocks[block_id].terminator
    }

    /// Create a new basic block in the current function.
    pub(super) fn new_block(&mut self) -> BlockId {
        self.current_fn
            .as_mut()
            .expect("no current function")
            .new_block()
    }

    /// Create a new temporary local.
    pub(super) fn new_temp(&mut self, ty: Ty) -> LocalId {
        self.current_fn
            .as_mut()
            .expect("no current function")
            .new_temp(ty)
    }

    /// Compute the allocation size for a type using the layout system.
    ///
    /// Classes and structs are stored field-by-field using fixed 8-byte
    /// slots (see cranelift.rs `SetField`/`GetField`), so a struct of
    /// N declared fields needs at least `N * 8` bytes regardless of the
    /// C layout size — a 3xUInt8 struct has layout.size == 3 but we still
    /// write UInt8s at offsets 0, 8, 16 when setting its fields.
    ///
    /// Phase B-4: runtime-dispatch classes prepend a class_info_ptr
    /// header slot (8 bytes per runtime-dispatch mixin; v1 ships with
    /// a single class_info_ptr at offset 0). The allocation size
    /// grows by `header_slots * 8` to accommodate the header.
    pub(super) fn alloc_size(&self, ty: &Ty) -> usize {
        use crate::resolve::symbols::DefKind;
        let layout = crate::codegen::layout::layout_of(ty, self.symbols);
        let base = layout.size.max(8);
        if let Ty::Class { name, .. } | Ty::Struct { name, .. } = ty {
            let mut total_fields = 0usize;
            let mut cur = Some(name.clone());
            while let Some(n) = cur.take() {
                for def in self.symbols.iter() {
                    if def.name == n {
                        match &def.kind {
                            DefKind::Class { info } => {
                                total_fields += info.fields.len();
                                cur = info
                                    .parent
                                    .and_then(|p| self.symbols.get(p).map(|d| d.name.clone()));
                                break;
                            }
                            DefKind::Struct { info } => {
                                total_fields += info.fields.len();
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
            // Phase B-4: add header slots for runtime-dispatch classes.
            let header = self.class_field_shift_for_ty(ty);
            let needed = (total_fields + header) * 8;
            return base.max(needed).max(8);
        }
        base
    }

    /// Phase B-5: emit a `SetField` at slot 0 of `dest` writing the
    /// address of `__rvn_classinfo_<ClassName>` — runs immediately
    /// after every `Alloc` of a runtime-dispatch class. No-op for
    /// classes whose `runtime_dispatch_includes` is empty (and for
    /// non-class types).
    ///
    /// Spec: docs/specs/types/mixin_vtables.spec.md §B5.
    pub(super) fn emit_class_info_init(&mut self, ty: &Ty, dest: LocalId) {
        let class_name = match ty {
            Ty::Class { name, .. } => name.clone(),
            _ => return,
        };
        // Only emit for runtime-dispatch classes.
        if self.class_field_index_shift(&class_name) == 0 {
            return;
        }
        // Materialize the class_info symbol address into a fresh
        // temporary, then store at slot 0 of the alloc.
        let addr_local = self.new_temp(Ty::Int);
        let sym = format!("__rvn_classinfo_{}", class_name);
        self.emit(MirInst::DataAddr {
            dest: addr_local,
            data_sym: sym,
        });
        self.emit(MirInst::SetField {
            base: dest,
            field_index: 0,
            value: MirValue::Use(addr_local),
        });
    }
}
