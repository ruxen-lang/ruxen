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
        self.register_binding(def_id, mutable, span.clone());

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

    pub(super) fn check_fn_call(
        &mut self,
        callee: DefId,
        callee_name: &str,
        args: &[HirExpr],
        _span: &Span,
    ) {
        // Checkpoint: borrows created for function args are temporary
        let checkpoint = self.borrows.checkpoint();
        let send_required_params =
            self.lookup_send_required_params(callee, callee_name, args.len());
        // A value passed to a `&T` / `&var T` parameter is auto-borrowed, not
        // moved — same reborrow rule as the method-call path.
        let param_is_ref = self.lookup_param_is_ref(callee, callee_name, args.len(), None);

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
            // not a move (Q12) — checked on both the argument type AND the
            // declared parameter type (an owned value auto-borrowed into a
            // `&T` param), so the same value can be passed without a false E1001.
            if let HirExprKind::VarRef(source_id) = &arg.kind {
                if !self.ty_is_effectively_copy(&arg.ty)
                    && !arg_is_reborrowed_reference(&arg.ty)
                    && !param_is_ref.get(idx).copied().unwrap_or(false)
                {
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
        callee: DefId,
        callee_name: &str,
        arg_len: usize,
    ) -> Vec<bool> {
        let mut required = vec![false; arg_len];
        let Some(signature) = self.callee_signature(callee, callee_name, None) else {
            return required;
        };
        for (idx, param) in signature.params.iter().enumerate().take(arg_len) {
            required[idx] = ty_has_bound(&param.ty, "Send");
        }
        required
    }

    /// The declared signature of the function or method a call targets.
    ///
    /// The call's own DefId wins when it names a `Function` or `Method`:
    /// resolve records the selected overload on a `FnCall`, and typeck
    /// records the selected method on a `MethodCall` (`resolved`, builtin
    /// heads included). Only when it does not (`UNRESOLVED_DEF` for a mixin
    /// default, an unselected overload set) does the lookup fall back to the
    /// callee's name: first a method homed on `receiver_home`, then the
    /// first function or method of that name.
    fn callee_signature(
        &self,
        callee: DefId,
        callee_name: &str,
        receiver_home: Option<&str>,
    ) -> Option<&crate::resolve::symbols::FnSignature> {
        fn signature_of(
            def: &crate::resolve::symbols::Definition,
        ) -> Option<&crate::resolve::symbols::FnSignature> {
            match &def.kind {
                DefKind::Function { signature } | DefKind::Method { signature, .. } => {
                    Some(signature)
                }
                _ => None,
            }
        }
        if let Some(signature) = self.symbols.get(callee).and_then(signature_of) {
            return Some(signature);
        }
        let homed = receiver_home.and_then(|home| {
            self.symbols.named(callee_name).find(|def| match &def.kind {
                DefKind::Method { parent, .. } => {
                    self.symbols.get(*parent).is_some_and(|p| p.name == home)
                }
                _ => false,
            })
        });
        homed
            .or_else(|| {
                self.symbols.named(callee_name).find(|def| {
                    matches!(def.kind, DefKind::Function { .. } | DefKind::Method { .. })
                })
            })
            .and_then(signature_of)
    }

    /// For each of the callee's first `arg_len` parameters, whether its declared
    /// type is a reference (`&T` / `&var T`). A value passed to a reference
    /// parameter is AUTO-BORROWED, not moved, so the borrow checker must not
    /// flag it as a move (false E1001). The callee is found by
    /// [`Self::callee_signature`]. (Pin: release-e2e 649_ffi_borrowed_string_arg.)
    /// `receiver_home`, when known, is the method home of the call's receiver
    /// (`Char`, `String`, `Hash`, a user class name, …). It disambiguates a
    /// method name that several classes define when the call carries no
    /// resolved DefId.
    ///
    /// A name-only lookup takes the FIRST def of that name in the whole
    /// symbol table, so one class's signature silently governed every other
    /// class's same-named method. That is not hypothetical: adding a by-value
    /// `def <=>(other: Char)` to `class Char` made every `x.<=>(y)` in the
    /// program look like a MOVE, and `965_comparable_spaceship` started
    /// reporting a false `value used here after move` on an unrelated `Int`
    /// receiver. Same root-cause family as Q58: a name-only lookup that
    /// ignores which class the method lives on.
    ///
    /// The exact-home pass is a preference, never a restriction. When the
    /// receiver is unknown (a free function) or no def is homed on it (a
    /// mixin default, an inherited method), it falls back to the name-only
    /// lookup.
    pub(super) fn lookup_param_is_ref(
        &self,
        callee: DefId,
        callee_name: &str,
        arg_len: usize,
        receiver_home: Option<&str>,
    ) -> Vec<bool> {
        let mut is_ref = vec![false; arg_len];
        let Some(signature) = self.callee_signature(callee, callee_name, receiver_home) else {
            return is_ref;
        };
        for (idx, param) in signature.params.iter().enumerate().take(arg_len) {
            is_ref[idx] = arg_is_reborrowed_reference(&param.ty);
        }
        is_ref
    }

    /// A send-required closure (passed to a `Send`-bounded parameter) may
    /// not borrow stack locals: it can outlive the caller's frame (E1013).
    /// Whether the captured VALUES are `Send` is typeck's job
    /// (`InferenceEngine::check_send_required_closure`), not checked here.
    pub(super) fn check_send_required_closure(
        &mut self,
        captures: &[Capture],
        is_move: bool,
        span: &Span,
    ) {
        for cap in captures {
            if cap.by_move || is_move {
                continue;
            }
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

    // ─── MethodCall ────────────────────────────────────────────────

    /// `resolved` is the method typeck selected (`MethodCall::resolved`),
    /// `UNRESOLVED_DEF` when it selected none.
    pub(super) fn check_method_call(
        &mut self,
        object: &HirExpr,
        resolved: DefId,
        method_name: &str,
        args: &[HirExpr],
        block: &Option<Box<HirExpr>>,
        span: &Span,
    ) {
        // Checkpoint: borrows created for method args are temporary
        let checkpoint = self.borrows.checkpoint();

        // Check the object expression
        self.check_expr(object);

        // Whether each of the callee method's parameters is a reference type, so
        // a value passed to a `&T` / `&var T` parameter is treated as an
        // AUTO-BORROW, not a move. Read from the method typeck selected, else
        // (an inherited mixin default, an unknown receiver) looked up by name
        // on the receiver's home. Without this, `s.include?(needle)` where
        // `include?(needle: &String)` borrows but `needle` is an owned `String`
        // value falsely reports `value used after move` (E1001) — the owned
        // value is auto-referenced to `&String` at the call site, never
        // consumed. (Pin: release-e2e 649_ffi_borrowed_string_arg.)
        let receiver_home = object.ty.method_home();
        let param_is_ref = self.lookup_param_is_ref(
            resolved,
            method_name,
            args.len(),
            Some(receiver_home.as_str()),
        );

        // Check args
        for (idx, arg) in args.iter().enumerate() {
            self.check_expr(arg);
            // Reference-typed args reborrow rather than move (Q12) — checked on
            // both the ARGUMENT type (`&x` passed directly) and the declared
            // PARAMETER type (an owned value auto-borrowed into a `&T` param).
            if let HirExprKind::VarRef(source_id) = &arg.kind {
                if !self.ty_is_effectively_copy(&arg.ty)
                    && !arg_is_reborrowed_reference(&arg.ty)
                    && !param_is_ref.get(idx).copied().unwrap_or(false)
                {
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

        // The receiver mode decides the receiver's fate: a `consume self`
        // method moves it, a `def var` method mutates it.
        if let HirExprKind::VarRef(obj_id) = &object.kind {
            match self.receiver_mode(resolved, method_name, &object.ty) {
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
                            help: vec!["ensure the previous borrow is no longer in use".to_string()],
                        });
                    }
                }
                Some(HirSelfMode::Ref) | None => {}
            }
        }

        // Kill temporary borrows from args — they're consumed by the callee
        self.borrows.kill_after_checkpoint(checkpoint);
    }

    /// The receiver mode of the method a call on `receiver` targets: the
    /// mode `resolved` declares. A builtin head's `lib` method aliased to a
    /// C symbol (`def push as "ruxen_vec_push"`) declares none (its
    /// signature always says `&self`), and an unselected method has no
    /// signature; for those the mode is the one of the common mutating
    /// builtins by that name (`push`, `insert`, ...), else `None`.
    fn receiver_mode(
        &self,
        resolved: DefId,
        method_name: &str,
        receiver: &crate::hir::types::Ty,
    ) -> Option<HirSelfMode> {
        use crate::hir::types::Ty;
        let nominal = matches!(
            receiver.peel_refs(),
            Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
        );
        let declared = self.symbols.get(resolved).and_then(|def| match &def.kind {
            DefKind::Method { signature, .. } => Some(signature),
            _ => None,
        });
        match declared {
            Some(signature) if nominal || signature.c_symbol.is_none() => signature.self_mode,
            _ => matches!(
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
            )
            .then_some(HirSelfMode::RefMut),
        }
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
