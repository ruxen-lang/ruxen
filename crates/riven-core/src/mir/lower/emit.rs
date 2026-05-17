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
            return base.max(total_fields * 8).max(8);
        }
        base
    }
}
