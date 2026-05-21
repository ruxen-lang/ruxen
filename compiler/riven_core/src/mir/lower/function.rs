use super::*;

impl<'a> Lowerer<'a> {
    // ── Function / Method lowering ──────────────────────────────────────

    pub(super) fn lower_function(&mut self, func: &HirFuncDef) -> Result<MirFunction, String> {
        self.lower_method(&func.name, func)
    }

    pub(super) fn lower_method(
        &mut self,
        name: &str,
        func: &HirFuncDef,
    ) -> Result<MirFunction, String> {
        // Reset per-function state.
        self.def_to_local.clear();
        self.cell_promoted.clear();
        self.initialized_heap_locals.clear();
        let mir_fn = MirFunction::new(name, func.return_ty.clone());
        self.current_block = mir_fn.entry_block;
        self.current_fn = Some(mir_fn);

        // If this method has a self_mode, add self as the first parameter.
        if func.self_mode.is_some() {
            // Derive the self type from the mangled method name (ClassName_method).
            //
            // Mangling is `format!("{ClassName}_{methodName}")`. Recovering
            // the class half is non-trivial because:
            //   * class names like `__HandlerFuture` start with `_`, so
            //     `split('_').next()` returns `""` (the empty prefix), and
            //   * method names like `do_something` carry their own
            //     underscores, so `splitn(2, '_')` would pick up `Foo` from
            //     `Foo_do_something` correctly but fail symmetrically on
            //     `__HandlerFuture_init`.
            // Resolve by walking underscore positions right-to-left and
            // accepting the longest prefix that actually names a class
            // (or struct/enum, for methods on those) in the symbol table.
            let self_ty = self
                .class_name_from_mangled(name)
                .map(|class_name| Ty::Class {
                    name: class_name.to_string(),
                    generic_args: vec![],
                })
                .unwrap_or(Ty::Unit);
            let local = self.fn_mut().new_local("self", self_ty, true);
            self.fn_mut().params.push(local);
            // Register all SelfValue DefIds in the symbol table so self.field works
            for def in self.symbols.iter() {
                if def.name == "self" {
                    if let crate::resolve::symbols::DefKind::SelfValue { .. } = &def.kind {
                        self.def_to_local.insert(def.id, local);
                    }
                }
            }
        }

        // Create locals for parameters.
        for param in &func.params {
            let local = self
                .fn_mut()
                .new_local(&param.name, param.ty.clone(), false);
            self.fn_mut().params.push(local);
            self.def_to_local.insert(param.def_id, local);
        }

        // Handle auto-assign params (@field) in init methods.
        // Generate SetField for each auto_assign param.
        // The field_index must match the class field order, not the param
        // order, since the class may have fields that aren't auto-assigned
        // (e.g., `status` in Task is set in the init body, not via @param).
        if func.name == "init" && func.self_mode.is_some() {
            // Find the self local (should be local 0 if self_mode is set)
            let self_local = self.def_to_local.values().copied().min().unwrap_or(0);
            // Recover the owning class name from the mangled method name
            // (see the symmetric note above on why `split('_').next()`
            // doesn't work for `__`-prefixed synth classes — that bug
            // caused auto-assigns on async state-machine classes to write
            // to slot 0/1 instead of slot 1/2, clobbering the
            // class_info_ptr header at slot 0).
            let class_name = self.class_name_from_mangled(name).unwrap_or("");
            let class_fields = self.get_class_field_names(class_name);
            // Phase B-4: shift past class_info_ptr header for
            // runtime-dispatch classes — declared field idx 0 maps
            // to MIR slot 1, etc. Computed once per init body.
            let shift = self.class_field_index_shift(class_name);
            for param in func.params.iter() {
                if param.auto_assign {
                    if let Some(&param_local) = self.def_to_local.get(&param.def_id) {
                        // Look up the field index by name in the class.
                        let field_index = class_fields
                            .iter()
                            .position(|f| f == &param.name)
                            .unwrap_or_else(|| {
                                // Fallback: try to find in the param list by position
                                func.params
                                    .iter()
                                    .position(|p| p.def_id == param.def_id)
                                    .unwrap_or(0)
                            });
                        self.emit(MirInst::SetField {
                            base: self_local,
                            field_index: field_index + shift,
                            value: MirValue::Use(param_local),
                        });
                    }
                }
            }
        }

        // Lower the body.
        let result = self.lower_expr(&func.body)?;

        // If the current block's terminator is still Unreachable, add an
        // implicit return.
        if matches!(self.get_terminator(), Terminator::Unreachable) {
            if func.return_ty == Ty::Unit || func.return_ty == Ty::Never {
                self.set_terminator(Terminator::Return(None));
            } else if let Some(local) = result {
                self.set_terminator(Terminator::Return(Some(MirValue::Use(local))));
            } else {
                self.set_terminator(Terminator::Return(None));
            }
        }

        let mut mir_fn = self.current_fn.take().expect("current_fn must be Some");

        // Determine the return-value locals so we don't drop them. A
        // function may return through multiple `Return` terminators (one
        // per match arm, for example), so we collect every distinct local
        // referenced in such a terminator.
        let return_locals = self.find_return_locals(&mir_fn);

        // Insert Drop instructions for Move-type locals before every Return.
        insert_drops(
            &mut mir_fn,
            &return_locals,
            self.symbols,
            &self.user_drop_classes,
        );

        Ok(mir_fn)
    }

    /// Find every local that appears as the value of a `Return` terminator.
    ///
    /// Functions with multiple early-return paths (e.g. each match arm
    /// returning a freshly built `String`) end up with several `Return`
    /// terminators referencing different locals. Drop elaboration must
    /// exclude all of them — otherwise the final scope-exit free would
    /// release the value the caller is about to read.
    pub(super) fn find_return_locals(&self, func: &MirFunction) -> HashSet<LocalId> {
        let mut out = HashSet::new();
        for block in &func.blocks {
            if let Terminator::Return(Some(MirValue::Use(local))) = &block.terminator {
                out.insert(*local);
            }
        }
        out
    }

    /// Lower a function-call argument, auto-invoking bare zero-arg function
    /// references.  In Riven, `puts greet` is parsed as `puts(greet)` with
    /// `greet` an `Identifier`; resolution turns the identifier into a
    /// `VarRef` even when it points at a function.  Without special handling
    /// the MIR would try to pass the function address as a value and end up
    /// passing `MirValue::Unit` (NULL).  Instead, detect that case and emit
    /// a `Call` that actually invokes the function.
    pub(super) fn lower_fn_arg(&mut self, arg: &HirExpr) -> Result<Option<LocalId>, String> {
        use crate::resolve::symbols::DefKind;
        if let HirExprKind::VarRef(def_id) = &arg.kind {
            // Only auto-invoke if the DefId is a zero-arg function and the
            // identifier is not already mapped to a local (which would mean
            // it was shadowed by a `let` binding of the same name).
            if !self.def_to_local.contains_key(def_id) {
                if let Some(def) = self.symbols.get(*def_id) {
                    if let DefKind::Function { signature } = &def.kind {
                        if signature.params.is_empty() {
                            let ret_ty = signature.return_ty.clone();
                            let callee_name = def.name.clone();
                            let dest = if ret_ty != Ty::Unit && ret_ty != Ty::Never {
                                Some(self.new_temp(ret_ty))
                            } else {
                                None
                            };
                            self.emit(MirInst::Call {
                                dest,
                                callee: callee_name,
                                args: vec![],
                            });
                            return Ok(dest);
                        }
                    }
                }
            }
        }
        self.lower_expr(arg)
    }
}
