use super::*;
use crate::hir::visit::{walk_expr, Visit};

impl<'a> Lowerer<'a> {
    // ── Function / Method lowering ──────────────────────────────────────

    pub(super) fn lower_function(&mut self, func: &HirFuncDef) -> Result<MirFunction, String> {
        self.lower_method(&func.name, func)
    }

    /// The resolver signature `func` was declared with, before any generic
    /// substitution a monomorphised copy applied to its HIR types.
    fn declared_signature(
        &self,
        func: &HirFuncDef,
    ) -> Option<&'a crate::resolve::symbols::FnSignature> {
        use crate::resolve::symbols::DefKind;
        let symbols: &'a SymbolTable = self.symbols;
        match &symbols.get(func.def_id)?.kind {
            DefKind::Method { signature, .. } | DefKind::Function { signature } => Some(signature),
            _ => None,
        }
    }

    /// The declared types of `func`'s parameters (see `declared_signature`),
    /// or none when the signature does not line up with the HIR params.
    fn declared_param_tys(&self, func: &HirFuncDef) -> Vec<Ty> {
        self.declared_signature(func)
            .filter(|sig| sig.params.len() == func.params.len())
            .map(|sig| sig.params.iter().map(|p| p.ty.clone()).collect())
            .unwrap_or_default()
    }

    pub(super) fn lower_method(
        &mut self,
        name: &str,
        func: &HirFuncDef,
    ) -> Result<MirFunction, String> {
        // Reset per-function state.
        self.def_to_local.clear();
        self.cell_promoted.clear();
        self.refmut_slots.clear();
        self.initialized_heap_locals.clear();
        self.stmt_log = drops::StmtScopeLog::default();
        // Seed the current debug span with the function's own definition line
        // so any pre-body emits (auto-assign SetFields) get a sensible
        // location; body expressions override it per-node in `lower_expr`.
        self.current_span = Some(func.span.clone());
        let mir_fn = MirFunction::new(name, func.return_ty.clone());
        self.current_block = mir_fn.entry_block;
        self.current_fn = Some(mir_fn);

        let has_self_param = func.self_mode.is_some()
            || (!func.is_class_method && self.class_name_from_mangled(name).is_some())
            || self
                .symbols
                .get(func.def_id)
                .map(|def| {
                    matches!(def.kind, crate::resolve::symbols::DefKind::Method { .. })
                        && !func.is_class_method
                })
                .unwrap_or(false);

        // If this method has a self receiver, add self as the first parameter.
        let mut self_local = None;
        if has_self_param {
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
            // A caller (currently only `emit_universal_kernel_bodies`) may pin
            // the concrete `self` type when the mangled name cannot name it —
            // e.g. a synthesized `Array[Int]_tap` whose `Array[Int]` prefix is
            // neither a symbol-table class nor a primitive, so the derivation
            // below would fall back to `Ty::Unit`.
            let self_ty = self
                .self_ty_override
                .clone()
                .or_else(|| {
                    self.class_name_from_mangled(name)
                        .map(|class_name| Ty::Class {
                            name: class_name.to_string(),
                            generic_args: vec![],
                        })
                })
                .or_else(|| self.primitive_self_ty_from_mangled(name))
                .unwrap_or(Ty::Unit);
            let local = self.fn_mut().new_local("self", self_ty, true);
            self.fn_mut().params.push(local);
            // Bind the `self` this body refers to (its own SelfValue, or the
            // declaring mixin's for a copied default body) so `self.field`
            // works. Other methods' SelfValues stay unbound.
            for self_def in self_value_refs(&func.body, self.symbols) {
                self.def_to_local.insert(self_def, local);
            }
            self_local = Some(local);
        }

        // Create locals for parameters. A slot-addressed `&var T` parameter
        // (`n: &var Int`, `s: &var String`) receives the address of the
        // caller's storage (`mir::refmut_abi`): every read loads through it
        // and every write stores through it, like a cell-promoted local. The
        // decision reads the DECLARED signature, as the caller does, so a
        // generic `&var T` stays a handle in a monomorphised copy.
        let declared = self.declared_param_tys(func);
        for (index, param) in func.params.iter().enumerate() {
            let local = self
                .fn_mut()
                .new_local(&param.name, param.ty.clone(), false);
            self.fn_mut().params.push(local);
            self.def_to_local.insert(param.def_id, local);
            let declared_ty = declared.get(index).unwrap_or(&param.ty);
            if crate::mir::refmut_abi::is_slot_refmut(declared_ty) {
                self.cell_promoted.insert(param.def_id);
                self.refmut_slots.insert(param.def_id);
            }
        }
        let receiver_mode = self_local.map(|_| {
            func.self_mode
                .as_ref()
                .map_or(ParamMode::Read, crate::mir::param_modes::receiver_mode)
        });
        let param_modes = receiver_mode
            .into_iter()
            .chain(func.params.iter().enumerate().map(|(index, param)| {
                crate::mir::param_modes::param_mode(
                    declared.get(index).unwrap_or(&param.ty),
                    self.symbols,
                )
            }))
            .collect();
        self.fn_mut().param_modes = param_modes;
        let declared_return = self
            .declared_signature(func)
            .map_or(&func.return_ty, |sig| &sig.return_ty);
        if crate::mir::refmut_abi::is_slot_refmut(declared_return) {
            return Err(format!(
                "`{name}` returns a `&var` of a scalar or String; returning the address of a \
                 variable is not supported yet, so return the value instead"
            ));
        }

        // Resolve rejects user params on `main` (E0739); never lower them
        // against the fixed C entry signature.
        if name == "main" && !has_self_param && !func.params.is_empty() {
            return Err("`main` cannot take parameters (E0739)".to_string());
        }
        if name == "main" && !has_self_param {
            let entry = self.fn_mut();
            entry.is_entry = true;
            entry.linkage = MirLinkage::Export;
        }
        // The process entry point takes the C `main(argc, argv)` ABI and hands
        // the arguments to `std.env` before the body runs. A no_std build has
        // no `std.env` and no `ruxen_env_init`, so its `main` keeps no params.
        if name == "main" && !has_self_param && !self.no_std {
            let argc = self.fn_mut().new_local("__argc", Ty::Int32, false);
            let argv = self.fn_mut().new_local("__argv", Ty::RawPtrVoid, false);
            self.fn_mut().params.extend([argc, argv]);
            self.emit(MirInst::Call {
                dest: None,
                callee: "ruxen_env_init".to_string(),
                args: vec![MirValue::Use(argc), MirValue::Use(argv)],
            });
        }

        // Handle auto-assign params (@field) in init methods.
        // Generate SetField for each auto_assign param.
        // The field_index must match the class field order, not the param
        // order, since the class may have fields that aren't auto-assigned
        // (e.g., `status` in Task is set in the init body, not via @param).
        if let Some(self_local) = self_local.filter(|_| func.name == "init") {
            // Recover the owning class name from the mangled method name
            // (see the symmetric note above on why `split('_').next()`
            // doesn't work for `__`-prefixed synth classes — that bug
            // caused auto-assigns on async state-machine classes to write
            // to slot 0/1 instead of slot 1/2, clobbering the
            // class_info_ptr header at slot 0).
            let class_name = self.class_name_from_mangled(name).unwrap_or_default();
            let class_fields = self.get_class_field_names(&class_name);
            // Phase B-4: shift past class_info_ptr header for
            // runtime-dispatch classes — declared field idx 0 maps
            // to MIR slot 1, etc. Computed once per init body.
            let shift = self.class_field_index_shift(&class_name);
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
        // Record the function's definition line for DWARF (P0.14). Only real
        // HIR functions reach here; synthesized MIR functions keep `None`.
        mir_fn.def_span = Some(func.span.clone());

        // Drop elaboration runs once every body is lowered
        // (`Lowerer::elaborate_drops`); record this body and its statement
        // boundaries for it.
        let scopes = std::mem::take(&mut self.stmt_log).scopes;
        self.drop_pending.insert(mir_fn.name.clone(), scopes);

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
    /// references.  In Ruxen, `puts greet` is parsed as `puts(greet)` with
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

/// The SelfValue DefIds `body` refers to: every `self` VarRef and every
/// closure capture of `self`, in first-reference order.
fn self_value_refs(body: &HirExpr, symbols: &SymbolTable) -> Vec<DefId> {
    struct SelfRefs<'s> {
        symbols: &'s SymbolTable,
        found: Vec<DefId>,
    }
    impl SelfRefs<'_> {
        fn note(&mut self, id: DefId) {
            let is_self = matches!(
                self.symbols.get(id).map(|def| &def.kind),
                Some(crate::resolve::symbols::DefKind::SelfValue { .. })
            );
            if is_self && !self.found.contains(&id) {
                self.found.push(id);
            }
        }
    }
    impl Visit for SelfRefs<'_> {
        fn visit_expr(&mut self, expr: &HirExpr) {
            match &expr.kind {
                HirExprKind::VarRef(id) => self.note(*id),
                HirExprKind::Closure { captures, .. } => {
                    for capture in captures {
                        self.note(capture.def_id);
                    }
                }
                _ => {}
            }
            walk_expr(self, expr);
        }
    }
    let mut refs = SelfRefs {
        symbols,
        found: Vec::new(),
    };
    refs.visit_expr(body);
    refs.found
}
