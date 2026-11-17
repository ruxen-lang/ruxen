use super::*;

impl<'a> BorrowChecker<'a> {
    // ─── VarRef ────────────────────────────────────────────────────

    pub(super) fn check_var_ref(&mut self, def_id: DefId, span: &Span) {
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
        // Check the value expression first; binding it moves a non-Copy
        // variable it yields.
        if let Some(val) = value {
            self.check_expr(val);
            self.consume(val, MoveInto::Binding(Some(def_id)));
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
        if !matches!(target.kind, HirExprKind::VarRef(_)) {
            self.check_expr(target);
        }

        // Rebinding a moved variable makes it live again. (A write through
        // a `&var` binding does not rebind it.)
        if let HirExprKind::VarRef(def_id) = &target.kind {
            if !writes_through(self.symbols, *def_id) && self.ownership.is_moved(*def_id) {
                self.ownership.reinitialize(*def_id);
                self.moves.reinitialize(*def_id, target.span.clone());
            }
        }

        // Storing the value moves a non-Copy variable it yields, whatever
        // the target (a variable, a field, an element).
        if semantics == MoveSemantics::Move {
            let target_def = match &target.kind {
                HirExprKind::VarRef(def_id) => Some(*def_id),
                _ => None,
            };
            self.consume(value, MoveInto::Binding(target_def));
        }
    }

    /// Every assignment form (`=`, `op=`, multi-assign) to a variable: it
    /// must be mutable (E1006), and it must not be a `&var` parameter that
    /// holds a handle (E1020).
    pub(super) fn check_assign_target_mutability(&mut self, target: &HirExpr, span: &Span) {
        self.check_handle_refmut_rebind(target, span);
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

    /// E1020: rebinding a `&var` parameter whose type is passed as its
    /// handle word (`!Ty::refmut_is_slot_address`: a class, struct, enum,
    /// array, hash, closure or type parameter). The callee holds a copy of
    /// the caller's handle, so `p = other` would be a silent no-op for the
    /// caller. Mutating through the handle (`p.x = 1`, `p.push(v)`) is fine.
    fn check_handle_refmut_rebind(&mut self, target: &HirExpr, span: &Span) {
        let HirExprKind::VarRef(def_id) = &target.kind else {
            return;
        };
        let Some(ty) = self.handle_refmut_params.get(def_id) else {
            return;
        };
        let name = self.def_name(*def_id);
        let label = format!(
            "cannot assign to `{name}`: a `{ty}` parameter holds the caller's handle, \
             so the new value would never reach the caller"
        );
        self.errors.push(BorrowError {
            code: ErrorCode::E1020,
            primary: SpanLabel {
                span: span.clone(),
                label,
            },
            secondary: vec![],
            help: vec!["return the new value, or wrap it in a field".to_string()],
        });
    }

    // ─── Borrow ────────────────────────────────────────────────────

    pub(super) fn check_borrow(&mut self, mutable: bool, inner: &HirExpr, span: &Span) {
        self.check_expr(inner);

        // `&var h["a"]`: a String or scalar place with no address is E1021.
        if mutable
            && !matches!(inner.kind, HirExprKind::VarRef(_))
            && inner.ty.refmut_is_slot_address()
            && inner.is_unaddressable_var_place()
        {
            self.report_unaddressable_var_place(inner);
        }

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
        }
    }

    // ─── FnCall ────────────────────────────────────────────────────

    pub(super) fn check_fn_call(&mut self, callee: DefId, callee_name: &str, args: &[HirExpr]) {
        let send_required_params =
            self.lookup_send_required_params(callee, callee_name, args.len());
        // A value passed to a `&T` / `&var T` parameter is auto-borrowed, and
        // a closure passed to a `Fn[...]` parameter is borrowed for the call:
        // neither is a move — same rule as the method-call path.
        let param_is_ref = self.lookup_param_is_ref(callee, callee_name, args.len(), None, None);
        let param_is_fn = self.lookup_param_is_fn(callee, callee_name, args.len(), None, None);
        let param_is_var = self.lookup_param_is_var(callee, callee_name, args.len(), None, None);

        for (idx, arg) in args.iter().enumerate() {
            if param_is_fn.get(idx).copied().unwrap_or(false) {
                self.allow_block_use(arg);
            }
            self.check_expr(arg);
            if send_required_params.get(idx).copied().unwrap_or(false) {
                if let HirExprKind::Closure {
                    captures,
                    is_move,
                    kind,
                    ..
                } = &arg.kind
                {
                    self.check_send_required_closure(captures, *is_move, *kind, &arg.span);
                }
            }
            // If arg is a VarRef and type is Move, record the move.
            // Structs that `derive Copy` are treated as Copy here. A
            // reference-typed arg (`&T`/`&var T`) is an implicit REBORROW,
            // not a move (Q12) — checked on both the argument type AND the
            // declared parameter type (an owned value auto-borrowed into a
            // `&T` param), so the same value can be passed without a false E1001.
            let by_value = !param_is_ref.get(idx).copied().unwrap_or(false);
            if by_value && !arg_is_reborrowed_reference(&arg.ty) {
                self.consume(arg, MoveInto::Call(callee_name));
            }
        }
        self.check_var_argument_addresses(args, &param_is_var);
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
    /// records the method it selected on a `MethodCall` (`resolved`: an own
    /// or inherited method on any head, or the mixin / `Kernel` default it
    /// typed the call with). Only when it does not does the lookup fall back
    /// to the callee's name. A method call (`receiver_home` given) falls
    /// back only to a method homed on its receiver, never to an unrelated
    /// same-named method elsewhere; a free call (an unselected overload
    /// set) takes the first function or method of that name.
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
        let found = match receiver_home {
            Some(home) => self.symbols.named(callee_name).find(|def| match &def.kind {
                DefKind::Method { parent, .. } => {
                    self.symbols.get(*parent).is_some_and(|p| p.name == home)
                }
                _ => false,
            }),
            None => self
                .symbols
                .named(callee_name)
                .find(|def| matches!(def.kind, DefKind::Function { .. } | DefKind::Method { .. })),
        };
        found.and_then(signature_of)
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
    /// A method call therefore never falls back past its receiver's home:
    /// typeck records inherited methods and mixin / `Kernel` defaults in
    /// `resolved`, and a free function (no receiver) keeps the name-only
    /// lookup.
    pub(super) fn lookup_param_is_ref(
        &self,
        callee: DefId,
        callee_name: &str,
        arg_len: usize,
        receiver: Option<&crate::hir::types::Ty>,
        requirement: Option<&[crate::hir::types::Ty]>,
    ) -> Vec<bool> {
        // A `Fn[...]` parameter borrows its closure for the call (the caller
        // keeps and frees the pair), so passing one is not a move either.
        self.param_flags(callee, callee_name, arg_len, receiver, requirement, |ty| {
            arg_is_reborrowed_reference(ty) || ty.is_borrowed_block()
        })
    }

    /// For each of the callee's first `arg_len` parameters, whether it is
    /// typed `Fn[...]`: such a parameter can only call its argument, so a
    /// block forwarded to it is not stored (E1018).
    pub(super) fn lookup_param_is_fn(
        &self,
        callee: DefId,
        callee_name: &str,
        arg_len: usize,
        receiver: Option<&crate::hir::types::Ty>,
        requirement: Option<&[crate::hir::types::Ty]>,
    ) -> Vec<bool> {
        self.param_flags(callee, callee_name, arg_len, receiver, requirement, |ty| {
            ty.is_borrowed_block()
        })
    }

    /// For each of the callee's first `arg_len` parameters, whether it is
    /// typed `&var T`: a place passed to it is mutably borrowed for the call.
    pub(super) fn lookup_param_is_var(
        &self,
        callee: DefId,
        callee_name: &str,
        arg_len: usize,
        receiver: Option<&crate::hir::types::Ty>,
        requirement: Option<&[crate::hir::types::Ty]>,
    ) -> Vec<bool> {
        self.param_flags(callee, callee_name, arg_len, receiver, requirement, |ty| {
            ty.is_mut_ref()
        })
    }

    /// `flag` applied to each of the callee's first `arg_len` declared
    /// parameter types (found by [`Self::declared_param_tys`]); `false` where
    /// the signature is unknown or has no parameter at that position.
    fn param_flags(
        &self,
        callee: DefId,
        callee_name: &str,
        arg_len: usize,
        receiver: Option<&crate::hir::types::Ty>,
        requirement: Option<&[crate::hir::types::Ty]>,
        flag: impl Fn(&crate::hir::types::Ty) -> bool,
    ) -> Vec<bool> {
        let mut flags = vec![false; arg_len];
        let Some(params) = self.declared_param_tys(callee, callee_name, receiver, requirement)
        else {
            return flags;
        };
        for (idx, ty) in params.iter().enumerate().take(arg_len) {
            flags[idx] = flag(ty);
        }
        flags
    }

    /// The declared parameter types of the call's target. `receiver` is the
    /// method call's receiver type (`None` for a free call):
    /// - calling a callable value (`f.(x)`, `f.call(x)`) reads the callable's
    ///   own signature;
    /// - otherwise the method typeck selected or the name lookup
    ///   ([`Self::callee_signature`]);
    /// - a call typeck resolved to a bodiless requirement of a mixin bound
    ///   (no def to record) reads the parameter types typeck recorded for
    ///   the overload it selected (`requirement`,
    ///   `MethodCall::requirement_params`).
    fn declared_param_tys(
        &self,
        callee: DefId,
        callee_name: &str,
        receiver: Option<&crate::hir::types::Ty>,
        requirement: Option<&[crate::hir::types::Ty]>,
    ) -> Option<Vec<crate::hir::types::Ty>> {
        // Resolve lowers every callable-value call (`f.(x)`, `f.call(x)`,
        // `yield`) to a `MethodCall` named `call`; typeck and MIR read the
        // same name. A distinct HIR node would retire it (TODO.md).
        if callee_name == "call" {
            if let Some((params, _)) = receiver.and_then(|r| r.peel_refs().callable_signature()) {
                return Some(params.to_vec());
            }
        }
        let home = receiver.map(|r| r.method_home(self.symbols.closure_shell_name()));
        if let Some(signature) = self.callee_signature(callee, callee_name, home.as_deref()) {
            return Some(signature.params.iter().map(|p| p.ty.clone()).collect());
        }
        requirement.map(<[_]>::to_vec)
    }

    /// Let the next `check_expr` of `child` use a block parameter: called by
    /// a parent for a position that calls, tests or forwards it. Only a direct
    /// `VarRef` child is marked, so the permission never reaches a nested use.
    pub(super) fn allow_block_use(&mut self, child: &HirExpr) {
        if let HirExprKind::VarRef(def_id) = &child.kind {
            self.block_use_allowed = self.block_params.contains(def_id);
        }
    }

    /// E1021: an argument auto-borrowed for a String or scalar `&var`
    /// parameter (the address of the caller's storage) that has no address:
    /// a Hash value, a safe-navigation field read. An explicit `&var place`
    /// is checked where it is taken (`check_borrow`).
    fn check_var_argument_addresses(&mut self, args: &[HirExpr], refmut_params: &[bool]) {
        let unaddressable = args.iter().enumerate().find(|(idx, arg)| {
            refmut_params.get(*idx).copied().unwrap_or(false)
                && !matches!(arg.kind, HirExprKind::Borrow { mutable: true, .. })
                && arg.ty.refmut_is_slot_address()
                && arg.is_unaddressable_var_place()
        });
        if let Some((_, arg)) = unaddressable {
            self.report_unaddressable_var_place(arg);
        }
    }

    /// E1021: `place`, passed for a String or scalar `&var` parameter (the
    /// address of the caller's storage), has no address: a Hash value, a
    /// safe-navigation field read (`HirExpr::is_unaddressable_var_place`).
    fn report_unaddressable_var_place(&mut self, place: &HirExpr) {
        self.errors.push(BorrowError {
            code: ErrorCode::E1021,
            primary: SpanLabel {
                span: place.span.clone(),
                label: format!(
                    "this `{}` has no address to pass to a `&var` parameter: only a \
                     variable, a field of an object or an Array element does",
                    place.ty.peel_refs()
                ),
            },
            secondary: vec![],
            help: vec!["bind it to a variable, pass that, and store the variable back".to_string()],
        });
    }

    /// E1018: a block parameter used other than by calling, testing or
    /// forwarding it, which would store a closure that borrows its captures.
    pub(super) fn report_stored_block(&mut self, def_id: DefId, span: &Span) {
        let name = self.def_name(def_id);
        let proc_ty = self
            .symbols
            .def_ty(def_id)
            .and_then(|ty| ty.proc_spelling())
            .unwrap_or_else(|| "Proc[...]".to_string());
        self.errors.push(BorrowError {
            code: ErrorCode::E1018,
            primary: SpanLabel {
                span: span.clone(),
                label: format!(
                    "block `{name}` is stored here; a block may only be called, \
                     tested with `block_given?` or forwarded to another `Fn[...]` parameter"
                ),
            },
            secondary: vec![],
            help: vec![format!(
                "declare `{name}` as `{proc_ty}` to store it: a proc owns its captures"
            )],
        });
    }

    /// E1019: `def_id` is a capture the current closure body borrows, and
    /// `span` moves it (`by_closure`: an owning proc or lambda captures it).
    pub(super) fn report_capture_move(&mut self, def_id: DefId, span: &Span, by_closure: bool) {
        let name = self.def_name(def_id);
        let label = if by_closure {
            format!(
                "`{name}` is a capture this closure body only borrows; \
                 capturing it in an owning proc or lambda here moves it out"
            )
        } else {
            format!(
                "`{name}` is a capture this closure body only borrows; \
                 using it by value here moves it out"
            )
        };
        self.errors.push(BorrowError {
            code: ErrorCode::E1019,
            primary: SpanLabel {
                span: span.clone(),
                label,
            },
            secondary: vec![],
            help: vec![format!(
                "a closure body borrows its captures: the enclosing scope frees a \
                 block's, the proc or lambda frees its own on drop; \
                 use `{name}.clone` for an owned copy"
            )],
        });
    }

    /// A send-required closure (passed to a `Send`-bounded parameter) may
    /// not borrow stack locals: it can outlive the caller's frame (E1013).
    /// Only a block borrows (`kind`, settled by typeck); a proc, a lambda
    /// and a `move` closure own their captures, except a capture whose type
    /// is a reference: owning `&T` does not own the pointee, which still
    /// lives in the caller's frame. Whether the captured VALUES
    /// are `Send` is typeck's job
    /// (`InferenceEngine::check_send_required_closure`), not checked here.
    pub(super) fn check_send_required_closure(
        &mut self,
        captures: &[Capture],
        is_move: bool,
        kind: ClosureKind,
        span: &Span,
    ) {
        for cap in captures {
            let owns_captures = closure_owns(cap, is_move, kind);
            let captures_a_ref = cap.current_ty(self.symbols).is_ref()
                || self.ref_bindings.contains_key(&cap.def_id);
            if owns_captures && !captures_a_ref {
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
        requirement: Option<&[crate::hir::types::Ty]>,
        method_name: &str,
        args: &[HirExpr],
        block: &Option<Box<HirExpr>>,
        span: &Span,
    ) {
        // Check the object expression. Calling a block (`block.(x)`, `yield`)
        // does not store it (the `call` protocol, see `declared_param_tys`).
        if method_name == "call" {
            self.allow_block_use(object);
        }
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
        let receiver = Some(&object.ty);
        let param_is_ref =
            self.lookup_param_is_ref(resolved, method_name, args.len(), receiver, requirement);
        let param_is_fn =
            self.lookup_param_is_fn(resolved, method_name, args.len(), receiver, requirement);
        let param_is_var =
            self.lookup_param_is_var(resolved, method_name, args.len(), receiver, requirement);

        // Check args
        for (idx, arg) in args.iter().enumerate() {
            if param_is_fn.get(idx).copied().unwrap_or(false) {
                self.allow_block_use(arg);
            }
            self.check_expr(arg);
            // Reference-typed args reborrow rather than move (Q12) — checked on
            // both the ARGUMENT type (`&x` passed directly) and the declared
            // PARAMETER type (an owned value auto-borrowed into a `&T` param).
            let by_value = !param_is_ref.get(idx).copied().unwrap_or(false);
            if by_value && !arg_is_reborrowed_reference(&arg.ty) {
                self.consume(arg, MoveInto::Call(method_name));
            }
        }

        // Check block argument. A literal block's kind was settled by typeck
        // (`check_closure` reads it). Forwarding a block (`&block`) to a final
        // `Fn[...]` parameter does not store it. `resolved` names the method
        // typeck settled the block against, mixin defaults included, so this
        // reads the same signature that settled a literal block.
        if let Some(blk) = block {
            let block_param_is_fn = self
                .declared_param_tys(resolved, method_name, receiver, requirement)
                .and_then(|params| params.last().cloned())
                .is_some_and(|ty| ty.is_borrowed_block());
            if block_param_is_fn {
                self.allow_block_use(blk);
            }
            self.check_expr(blk);
        }

        // A `consume self` method moves its receiver.
        if self.receiver_is_consumed(resolved, &object.ty) {
            if let HirExprKind::VarRef(obj_id) = &object.kind {
                self.record_move(*obj_id, &object.ty, MoveInto::Call(method_name), span);
            }
        }
        self.check_var_argument_addresses(args, &param_is_var);
    }

    /// Whether the method a call on `receiver` targets is a `consume self`
    /// method: the mode `resolved` declares. A builtin head's `lib` method
    /// aliased to a C symbol (`def push as "ruxen_vec_push"`) declares none
    /// (its signature always says `&self`).
    fn receiver_is_consumed(&self, resolved: DefId, receiver: &crate::hir::types::Ty) -> bool {
        use crate::hir::types::Ty;
        let nominal = matches!(
            receiver.peel_refs(),
            Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. }
        );
        self.symbols
            .get(resolved)
            .is_some_and(|def| match &def.kind {
                DefKind::Method { signature, .. } => {
                    (nominal || signature.c_symbol.is_none())
                        && signature.self_mode == Some(HirSelfMode::Consuming)
                }
                _ => false,
            })
    }

    // ─── Return ────────────────────────────────────────────────────

    pub(super) fn check_return(&mut self, opt_expr: Option<&HirExpr>, span: &Span) {
        if let Some(expr) = opt_expr {
            self.check_expr(expr);
            self.consume(expr, MoveInto::Binding(None));

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
