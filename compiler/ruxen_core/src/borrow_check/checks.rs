use super::*;

impl<'a> BorrowChecker<'a> {
    // ─── VarRef ────────────────────────────────────────────────────

    pub(super) fn check_var_ref(&mut self, def_id: DefId, span: &Span) {
        // NLL: update last_use for borrows associated with this variable.
        // Direct borrows held by this def_id:
        let held: Vec<_> = self
            .borrows
            .borrows_held_by(def_id)
            .iter()
            .map(|b| b.id)
            .collect();
        for borrow_id in held {
            self.borrows.record_use(borrow_id, span.clone());
        }
        // If this is a reference variable (e.g., `let r = &v`), update borrows on the source:
        if let Some(&source) = self.ref_bindings.get(&def_id) {
            let source_borrows: Vec<_> = self
                .borrows
                .active_borrows_of(source)
                .iter()
                .map(|b| b.id)
                .collect();
            for borrow_id in source_borrows {
                self.borrows.record_use(borrow_id, span.clone());
            }
        }

        // Check use-after-move
        if let Err(err) = self.moves.check_use(def_id, span.clone()) {
            let name = self
                .symbols
                .get(def_id)
                .map(|d| d.name.clone())
                .unwrap_or_else(|| format!("_{}", def_id));

            let mut secondary = vec![SpanLabel {
                span: err.declared_span.clone(),
                label: format!("`{}` defined here", name),
            }];

            let move_label = if let Some(ref callee) = err.callee {
                format!("value moved into `{}` here", callee)
            } else {
                "value moved here".to_string()
            };
            secondary.push(SpanLabel {
                span: err.move_span.clone(),
                label: move_label,
            });

            self.errors.push(BorrowError {
                code: ErrorCode::E1001,
                primary: SpanLabel {
                    span: span.clone(),
                    label: format!("`{}` used here after move", name),
                },
                secondary,
                help: vec![format!("consider cloning the value: `{}.clone`", name)],
            });
        }
    }

    // ─── Let statement ─────────────────────────────────────────────

    pub(super) fn check_let(
        &mut self,
        def_id: DefId,
        pattern: &HirPattern,
        ty: &Ty,
        value: Option<&HirExpr>,
        mutable: bool,
        span: &Span,
    ) {
        // Check the value expression first
        if let Some(val) = value {
            self.check_expr(val);

            // If the value is a VarRef and the type is Move, record the move.
            // Structs that `derive Copy` are treated as Copy here — see
            // `ty_is_effectively_copy`.
            if let HirExprKind::VarRef(source_id) = &val.kind {
                if !self.ty_is_effectively_copy(&val.ty) {
                    self.moves
                        .process_transfer(*source_id, Some(def_id), &val.ty, span.clone());
                    self.ownership.record_move(*source_id, def_id, span.clone());
                }
            }
        }

        // If the value is a Borrow of a VarRef, record the ref→source mapping for NLL
        if let Some(val) = value {
            if let HirExprKind::Borrow { expr: inner, .. } = &val.kind {
                if let HirExprKind::VarRef(source_id) = &inner.kind {
                    self.ref_bindings.insert(def_id, *source_id);
                }
            }
        }

        // Register the new binding
        self.register_binding(def_id, ty, mutable, span.clone());
        self.lifetimes.register_local(def_id, self.scopes.current());

        // Process pattern bindings (for destructuring)
        self.process_pattern(pattern);
    }

    // ─── Assign ────────────────────────────────────────────────────

    pub(super) fn check_assign(
        &mut self,
        target: &HirExpr,
        value: &HirExpr,
        semantics: MoveSemantics,
    ) {
        // Check the value first
        self.check_expr(value);

        // Check mutability of target
        self.check_assign_target_mutability(target, &target.span);

        // Check borrow conflicts on mutation
        if let HirExprKind::VarRef(def_id) = &target.kind {
            if let Err(conflict) = self.borrows.check_mutation(*def_id) {
                let name = self.def_name(*def_id);
                self.errors.push(BorrowError {
                    code: ErrorCode::E1009,
                    primary: SpanLabel {
                        span: target.span.clone(),
                        label: format!("cannot assign to `{}` — currently borrowed", name),
                    },
                    secondary: vec![SpanLabel {
                        span: conflict.existing.created_span.clone(),
                        label: "borrow created here".to_string(),
                    }],
                    help: vec![],
                });
            }

            // If the target variable was previously moved, reinitialize it
            if self.ownership.is_moved(*def_id) {
                self.ownership.reinitialize(*def_id);
                self.moves.reinitialize(*def_id, target.span.clone());
            }

            // If value is a VarRef and move semantics, record the move.
            // Structs that `derive Copy` are treated as Copy here.
            if semantics == MoveSemantics::Move {
                if let HirExprKind::VarRef(source_id) = &value.kind {
                    if !self.ty_is_effectively_copy(&value.ty) {
                        self.moves.process_transfer(
                            *source_id,
                            Some(*def_id),
                            &value.ty,
                            target.span.clone(),
                        );
                        self.ownership
                            .record_move(*source_id, *def_id, target.span.clone());
                    }
                }
            }
        }
    }

    pub(super) fn check_assign_target_mutability(&mut self, target: &HirExpr, span: &Span) {
        if let HirExprKind::VarRef(def_id) = &target.kind {
            if !self.is_mutable(*def_id) {
                let name = self.def_name(*def_id);
                self.errors.push(BorrowError {
                    code: ErrorCode::E1006,
                    primary: SpanLabel {
                        span: span.clone(),
                        label: format!(
                            "cannot assign to `{}` — variable is immutable (declared with `let`)",
                            name
                        ),
                    },
                    secondary: vec![],
                    help: vec![format!("consider declaring with `var {}`", name)],
                });
            }
        }
    }

    // ─── Borrow ────────────────────────────────────────────────────

    pub(super) fn check_borrow(&mut self, mutable: bool, inner: &HirExpr, span: &Span) {
        self.check_expr(inner);

        if let HirExprKind::VarRef(def_id) = &inner.kind {
            // For &mut borrows, check the target is mutable
            if mutable && !self.is_mutable(*def_id) {
                let name = self.def_name(*def_id);
                self.errors.push(BorrowError {
                    code: ErrorCode::E1007,
                    primary: SpanLabel {
                        span: span.clone(),
                        label: format!(
                            "cannot borrow `{}` as mutable — it is immutable (declared with `let`, not `var`)",
                            name
                        ),
                    },
                    secondary: vec![],
                    help: vec![format!("consider declaring with `var {}`", name)],
                });
            }

            // Check for conflicts with existing borrows
            let kind = if mutable {
                BorrowKind::Mutable
            } else {
                BorrowKind::Shared
            };
            if let Err(conflict) = self.borrows.check_new_borrow(kind, *def_id) {
                let name = self.def_name(*def_id);
                let code = match (kind, conflict.existing.kind) {
                    (BorrowKind::Mutable, BorrowKind::Shared) => ErrorCode::E1002,
                    (BorrowKind::Shared, BorrowKind::Mutable) => ErrorCode::E1003,
                    (BorrowKind::Mutable, BorrowKind::Mutable) => ErrorCode::E1002,
                    _ => ErrorCode::E1002,
                };
                self.errors.push(BorrowError {
                    code,
                    primary: SpanLabel {
                        span: span.clone(),
                        label: format!("cannot borrow `{}` here", name),
                    },
                    secondary: vec![SpanLabel {
                        span: conflict.existing.created_span.clone(),
                        label: format!(
                            "previous {} borrow of `{}` here",
                            if conflict.existing.kind == BorrowKind::Mutable {
                                "mutable"
                            } else {
                                "immutable"
                            },
                            name
                        ),
                    }],
                    help: vec![],
                });
            } else {
                // Create the borrow
                // Use a dummy borrower DefId (we don't always have a target binding)
                let scope = self.scopes.current();
                self.borrows
                    .create(kind, *def_id, *def_id, span.clone(), scope);
            }
        }
    }

    // ─── FnCall ────────────────────────────────────────────────────

    pub(super) fn check_fn_call(&mut self, callee_name: &str, args: &[HirExpr], _span: &Span) {
        // Checkpoint: borrows created for function args are temporary
        let checkpoint = self.borrows.checkpoint();
        let send_required_params = self.lookup_send_required_params(callee_name, args.len());

        for (idx, arg) in args.iter().enumerate() {
            self.check_expr(arg);
            if send_required_params.get(idx).copied().unwrap_or(false) {
                if let HirExprKind::Closure {
                    captures, is_move, ..
                } = &arg.kind
                {
                    self.check_send_required_closure(captures, *is_move, &arg.span);
                }
            }
            // If arg is a VarRef and type is Move, record the move.
            // Structs that `derive Copy` are treated as Copy here. A
            // reference-typed arg (`&T`/`&var T`) is an implicit REBORROW,
            // not a move (Q12) — skip it so the same reference can be passed
            // to several calls without a false E1001.
            if let HirExprKind::VarRef(source_id) = &arg.kind {
                if !self.ty_is_effectively_copy(&arg.ty) && !arg_is_reborrowed_reference(&arg.ty) {
                    self.moves.process_call_move(
                        *source_id,
                        callee_name.to_string(),
                        &arg.ty,
                        arg.span.clone(),
                    );
                    self.ownership.record_move_into_call(
                        *source_id,
                        callee_name.to_string(),
                        arg.span.clone(),
                    );
                }
            }
        }

        // Kill temporary borrows from args — they're consumed by the callee
        self.borrows.kill_after_checkpoint(checkpoint);
    }

    pub(super) fn lookup_send_required_params(
        &self,
        callee_name: &str,
        arg_len: usize,
    ) -> Vec<bool> {
        let mut required = vec![false; arg_len];
        let Some(def) = self.symbols.iter().find(|def| {
            def.name == callee_name
                && matches!(def.kind, DefKind::Function { .. } | DefKind::Method { .. })
        }) else {
            return required;
        };
        let signature = match &def.kind {
            DefKind::Function { signature } | DefKind::Method { signature, .. } => signature,
            _ => return required,
        };
        for (idx, param) in signature.params.iter().enumerate().take(arg_len) {
            required[idx] = ty_has_bound(&param.ty, "Send");
        }
        required
    }

    pub(super) fn check_send_required_closure(
        &mut self,
        captures: &[Capture],
        is_move: bool,
        span: &Span,
    ) {
        for cap in captures {
            // `cap.ty` is a resolve-time snapshot (often Ty::Infer for
            // values whose type wasn't pinned yet); refetch from the
            // symbol table to get the post-typeck type. See
            // `Capture::current_ty` docs.
            let live_ty = cap.current_ty(self.symbols);
            if cap.by_move || is_move {
                if !live_ty.is_send_with(self.symbols) {
                    self.errors.push(BorrowError {
                        code: ErrorCode::E1011,
                        primary: SpanLabel {
                            span: span.clone(),
                            label: format!(
                                "captured value `{}` of type `{}` is not `Send`",
                                cap.name, live_ty
                            ),
                        },
                        secondary: vec![],
                        help: vec![
                            "move only `Send` values into send-required closures".to_string()
                        ],
                    });
                }
            } else {
                self.errors.push(BorrowError {
                    code: ErrorCode::E1013,
                    primary: SpanLabel {
                        span: span.clone(),
                        label: format!(
                            "closure captures local `{}` by borrow and is not `'static`",
                            cap.name
                        ),
                    },
                    secondary: vec![],
                    help: vec![
                        "use a `move` closure or avoid capturing stack-local borrows".to_string(),
                    ],
                });
            }
        }
    }

    // ─── MethodCall ────────────────────────────────────────────────

    pub(super) fn check_method_call(
        &mut self,
        object: &HirExpr,
        method_def_id: DefId,
        method_name: &str,
        args: &[HirExpr],
        block: &Option<Box<HirExpr>>,
        span: &Span,
    ) {
        // Checkpoint: borrows created for method args are temporary
        let checkpoint = self.borrows.checkpoint();

        // Check the object expression
        self.check_expr(object);

        // Check args
        for arg in args {
            self.check_expr(arg);
            // Reference-typed args reborrow rather than move (Q12).
            if let HirExprKind::VarRef(source_id) = &arg.kind {
                if !self.ty_is_effectively_copy(&arg.ty) && !arg_is_reborrowed_reference(&arg.ty) {
                    self.moves.process_call_move(
                        *source_id,
                        method_name.to_string(),
                        &arg.ty,
                        arg.span.clone(),
                    );
                    self.ownership.record_move_into_call(
                        *source_id,
                        method_name.to_string(),
                        arg.span.clone(),
                    );
                }
            }
        }

        // Check block argument
        if let Some(blk) = block {
            self.check_expr(blk);
        }

        // Check self_mode: consuming methods move receiver, &mut self checks mutation
        if let HirExprKind::VarRef(obj_id) = &object.kind {
            // Look up method in symbol table
            if let Some(def) = self.symbols.get(method_def_id) {
                if let DefKind::Method { signature, .. } = &def.kind {
                    match signature.self_mode {
                        Some(HirSelfMode::Consuming) => {
                            if !self.ty_is_effectively_copy(&object.ty) {
                                self.moves.process_call_move(
                                    *obj_id,
                                    method_name.to_string(),
                                    &object.ty,
                                    span.clone(),
                                );
                                self.ownership.record_move_into_call(
                                    *obj_id,
                                    method_name.to_string(),
                                    span.clone(),
                                );
                            }
                        }
                        Some(HirSelfMode::RefMut) => {
                            // &mut self method: check mutation conflicts
                            if let Err(conflict) = self.borrows.check_mutation(*obj_id) {
                                let name = self.def_name(*obj_id);
                                self.errors.push(BorrowError {
                                    code: ErrorCode::E1002,
                                    primary: SpanLabel {
                                        span: span.clone(),
                                        label: format!(
                                            "cannot mutably borrow `{}` — already borrowed",
                                            name
                                        ),
                                    },
                                    secondary: vec![SpanLabel {
                                        span: conflict.existing.created_span.clone(),
                                        label: format!(
                                            "previous {} borrow of `{}` here",
                                            if conflict.existing.kind == BorrowKind::Mutable {
                                                "mutable"
                                            } else {
                                                "immutable"
                                            },
                                            name
                                        ),
                                    }],
                                    help: vec!["ensure the previous borrow is no longer in use"
                                        .to_string()],
                                });
                            }
                        }
                        _ => {}
                    }
                }
            } else {
                // Method not in symbol table — use name-based heuristic for common mutating methods
                let is_mutating = matches!(
                    method_name,
                    "push"
                        | "pop"
                        | "insert"
                        | "remove"
                        | "clear"
                        | "sort"
                        | "reverse"
                        | "push_str"
                        | "truncate"
                        | "extend"
                        | "retain"
                        | "drain"
                        | "iter_mut"
                        | "set"
                );
                if is_mutating {
                    if let Err(conflict) = self.borrows.check_mutation(*obj_id) {
                        let name = self.def_name(*obj_id);
                        self.errors.push(BorrowError {
                            code: ErrorCode::E1002,
                            primary: SpanLabel {
                                span: span.clone(),
                                label: format!("cannot mutably borrow `{}` — already borrowed", name),
                            },
                            secondary: vec![SpanLabel {
                                span: conflict.existing.created_span.clone(),
                                label: format!(
                                    "previous {} borrow of `{}` here",
                                    if conflict.existing.kind == BorrowKind::Mutable { "mutable" } else { "immutable" },
                                    name
                                ),
                            }],
                            help: vec!["ensure the previous borrow is no longer in use".to_string()],
                        });
                    }
                }

                // (The name-based `iter` / `into_iter` borrow/move
                // ownership tracking was removed with the orphaned
                // iterator machinery — Phase B / Milestone 2. Nothing
                // produces those calls.)
            }
        }

        // Kill temporary borrows from args — they're consumed by the callee
        self.borrows.kill_after_checkpoint(checkpoint);
    }

    // ─── Return ────────────────────────────────────────────────────

    pub(super) fn check_return(&mut self, opt_expr: Option<&HirExpr>, span: &Span) {
        if let Some(expr) = opt_expr {
            self.check_expr(expr);

            // Check for returning reference to local (E1010)
            if expr.ty.is_ref() {
                if let HirExprKind::Borrow { expr: inner, .. } = &expr.kind {
                    if let HirExprKind::VarRef(def_id) = &inner.kind {
                        // Check if this def is a local variable (not a parameter)
                        if let Some(def) = self.symbols.get(*def_id) {
                            if matches!(def.kind, DefKind::Variable { .. }) {
                                self.errors.push(BorrowError {
                                    code: ErrorCode::E1010,
                                    primary: SpanLabel {
                                        span: span.clone(),
                                        label: format!(
                                            "returns a reference to local variable `{}`",
                                            def.name
                                        ),
                                    },
                                    secondary: vec![SpanLabel {
                                        span: def.span.clone(),
                                        label: format!("`{}` defined here", def.name),
                                    }],
                                    help: vec![
                                        "consider returning an owned value instead".to_string()
                                    ],
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}

/// A reference-typed argument is reborrowed at the call site, not moved
/// (Q12, gui-stack-v1-issues). `&T` / `&var T` passed to a callee that
/// expects a reference is an implicit reborrow — recording it as a move
/// produces a false E1001 on any later use of the same reference (e.g. a
/// closure that passes its `&var Ui` param to two different calls).
fn arg_is_reborrowed_reference(ty: &crate::hir::types::Ty) -> bool {
    use crate::hir::types::Ty;
    matches!(
        ty,
        Ty::Ref(_) | Ty::RefMut(_) | Ty::RefLifetime(_, _) | Ty::RefMutLifetime(_, _)
    )
}
