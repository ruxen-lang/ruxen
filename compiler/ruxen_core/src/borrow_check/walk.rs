use super::*;

impl<'a> BorrowChecker<'a> {
    // ─── Program / Item walking ────────────────────────────────────

    pub(super) fn check_program(&mut self, program: &HirProgram) {
        for item in &program.items {
            self.check_item(item);
        }
    }

    pub(super) fn check_item(&mut self, item: &HirItem) {
        match item {
            HirItem::Function(func) => self.check_function(func),
            HirItem::Class(class) => {
                for method in &class.methods {
                    self.check_function(method);
                }
                for imp in &class.impl_blocks {
                    self.check_impl_block(imp);
                }
            }
            HirItem::Struct(_) => {
                // Struct definitions have no executable code to check.
            }
            HirItem::Enum(_) => {
                // Enum definitions have no executable code to check.
            }
            HirItem::Mixin(trait_def) => {
                for trait_item in &trait_def.items {
                    if let HirMixinItem::DefaultMethod(func) = trait_item {
                        self.check_function(func);
                    }
                }
            }
            HirItem::Impl(imp) => self.check_impl_block(imp),
            HirItem::Module(module) => {
                for sub_item in &module.items {
                    self.check_item(sub_item);
                }
            }
            HirItem::TypeAlias(_) | HirItem::Newtype(_) | HirItem::Const(_) => {}
        }
    }

    pub(super) fn check_impl_block(&mut self, imp: &HirImplBlock) {
        for impl_item in &imp.items {
            if let HirImplItem::Method(func) = impl_item {
                self.check_function(func);
            }
        }
    }

    // ─── Function ──────────────────────────────────────────────────

    pub(super) fn check_function(&mut self, func: &HirFuncDef) {
        // Push function scope
        let scope_id = self.scopes.push(ScopeKind::Function);
        self.lifetimes.clear_locals();

        // Register parameters
        for param in &func.params {
            self.register_binding(param.def_id, &param.ty, true, param.span.clone());
            self.lifetimes.register_local(param.def_id, scope_id);
        }

        // Walk the body
        self.check_expr(&func.body);

        // Exit scope: kill borrows and pop
        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── Expressions ───────────────────────────────────────────────

    pub(super) fn check_expr(&mut self, expr: &HirExpr) {
        // NLL: expire dead borrows before processing each expression
        self.borrows.expire_before(expr.span.clone());

        match &expr.kind {
            HirExprKind::VarRef(def_id) => {
                self.check_var_ref(*def_id, &expr.span);
            }

            HirExprKind::IntLiteral(_)
            | HirExprKind::FloatLiteral(_)
            | HirExprKind::StringLiteral(_)
            | HirExprKind::BoolLiteral(_)
            | HirExprKind::CharLiteral(_)
            | HirExprKind::UnitLiteral
            | HirExprKind::Continue
            | HirExprKind::Error => {}

            HirExprKind::FieldAccess { object, .. } => {
                self.check_expr(object);
            }

            HirExprKind::MethodCall {
                object,
                method,
                method_name,
                args,
                block,
                ..
            } => {
                self.check_method_call(object, *method, method_name, args, block, &expr.span);
            }

            HirExprKind::FnCall {
                callee: _,
                callee_name,
                args,
            } => {
                self.check_fn_call(callee_name, args, &expr.span);
            }

            HirExprKind::BinaryOp { left, right, .. } => {
                self.check_expr(left);
                self.check_expr(right);
            }

            HirExprKind::UnaryOp { operand, .. } => {
                self.check_expr(operand);
            }

            HirExprKind::Borrow {
                mutable,
                expr: inner,
            } => {
                self.check_borrow(*mutable, inner, &expr.span);
            }

            HirExprKind::Block(stmts, tail) => {
                self.check_block(stmts, tail.as_deref());
            }

            HirExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.check_if(cond, then_branch, else_branch.as_deref());
            }

            HirExprKind::Match { scrutinee, arms } => {
                self.check_match(scrutinee, arms);
            }

            HirExprKind::Loop { body } => {
                self.check_loop(body);
            }

            HirExprKind::While { condition, body } => {
                self.check_while(condition, body);
            }

            HirExprKind::For {
                binding,
                binding_name: _,
                iterable,
                body,
                tuple_bindings: _,
            } => {
                self.check_for(*binding, iterable, body);
            }

            HirExprKind::Assign {
                target,
                value,
                semantics,
            } => {
                self.check_assign(target, value, *semantics);
            }

            HirExprKind::CompoundAssign { target, value, .. } => {
                // Compound assign requires mutability of target
                self.check_assign_target_mutability(target, &expr.span);
                self.check_expr(target);
                self.check_expr(value);
            }

            HirExprKind::Return(opt_expr) => {
                self.check_return(opt_expr.as_deref(), &expr.span);
            }

            HirExprKind::Break(opt_expr) => {
                if let Some(inner) = opt_expr {
                    self.check_expr(inner);
                }
            }

            HirExprKind::Closure {
                params,
                body,
                captures,
                is_move,
                ..
            } => {
                self.check_closure(params, body, captures, *is_move, &expr.span);
            }

            HirExprKind::Construct { fields, .. } => {
                for (_, field_expr) in fields {
                    self.check_expr(field_expr);
                }
            }

            HirExprKind::EnumVariant { fields, .. } => {
                for (_, field_expr) in fields {
                    self.check_expr(field_expr);
                }
            }

            HirExprKind::Tuple(elems) => {
                for elem in elems {
                    self.check_expr(elem);
                }
            }

            HirExprKind::Index { object, index } => {
                self.check_expr(object);
                self.check_expr(index);
            }

            HirExprKind::Cast { expr: inner, .. } => {
                self.check_expr(inner);
            }

            HirExprKind::ArrayLiteral(elems) => {
                for elem in elems {
                    self.check_expr(elem);
                }
            }

            HirExprKind::MapLiteral(entries) => {
                for (k, v) in entries {
                    self.check_expr(k);
                    self.check_expr(v);
                }
            }

            HirExprKind::ArrayFill { value, .. } => {
                self.check_expr(value);
            }

            HirExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.check_expr(s);
                }
                if let Some(e) = end {
                    self.check_expr(e);
                }
            }

            HirExprKind::Interpolation { parts } => {
                for part in parts {
                    if let HirInterpolationPart::Expr { expr: e, .. } = part {
                        self.check_expr(e);
                    }
                }
            }

            HirExprKind::MacroCall { args, .. } => {
                for arg in args {
                    self.check_expr(arg);
                }
            }

            HirExprKind::UnsafeBlock(stmts, tail) => {
                for stmt in stmts {
                    self.check_statement(stmt);
                }
                if let Some(tail_expr) = tail {
                    self.check_expr(tail_expr);
                }
            }

            HirExprKind::NullLiteral => {
                // Nothing to check — no borrows involved.
            }
        }
    }

    // ─── Block ─────────────────────────────────────────────────────

    pub(super) fn check_block(&mut self, stmts: &[HirStatement], tail: Option<&HirExpr>) {
        let scope_id = self.scopes.push(ScopeKind::Block);

        for stmt in stmts {
            self.check_statement(stmt);
        }

        if let Some(tail_expr) = tail {
            self.check_expr(tail_expr);
        }

        // Exit: kill borrows in this scope, pop
        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── Statement ─────────────────────────────────────────────────

    pub(super) fn check_statement(&mut self, stmt: &HirStatement) {
        match stmt {
            HirStatement::Let {
                def_id,
                pattern,
                ty,
                value,
                mutable,
                span,
            } => {
                self.check_let(*def_id, pattern, ty, value.as_ref(), *mutable, span);
            }
            HirStatement::Expr(expr) => {
                self.check_expr(expr);
            }
        }
    }

    // ─── If ────────────────────────────────────────────────────────

    pub(super) fn check_if(
        &mut self,
        cond: &HirExpr,
        then_branch: &HirExpr,
        else_branch: Option<&HirExpr>,
    ) {
        self.check_expr(cond);

        // Snapshot state before branches
        let ownership_snap = self.ownership.snapshot();
        let moves_snap = self.moves.snapshot();
        let borrows_snap = self.borrows.snapshot();

        // Walk then-branch
        self.check_expr(then_branch);
        let then_ownership = self.ownership.snapshot();
        let then_moves = self.moves.snapshot();

        // Restore for else-branch
        self.ownership = ownership_snap.clone();
        self.moves.restore(&moves_snap);
        self.borrows.restore(&borrows_snap);

        if let Some(else_br) = else_branch {
            self.check_expr(else_br);
        }
        let else_ownership = self.ownership.snapshot();
        let else_moves = self.moves.snapshot();

        // Merge: conservative — moved on ANY branch → moved after
        self.ownership = OwnershipState::merge(vec![then_ownership, else_ownership]);
        self.moves.merge(vec![then_moves, else_moves]);
    }

    // ─── Match ─────────────────────────────────────────────────────

    pub(super) fn check_match(&mut self, scrutinee: &HirExpr, arms: &[HirMatchArm]) {
        self.check_expr(scrutinee);

        if arms.is_empty() {
            return;
        }

        let ownership_snap = self.ownership.snapshot();
        let moves_snap = self.moves.snapshot();
        let borrows_snap = self.borrows.snapshot();

        let mut branch_ownerships = Vec::new();
        let mut branch_moves = Vec::new();

        for arm in arms {
            // Restore state for each arm
            self.ownership = ownership_snap.clone();
            self.moves.restore(&moves_snap);
            self.borrows.restore(&borrows_snap);

            // Enter arm scope
            let scope_id = self.scopes.push(ScopeKind::MatchArm);

            // Process pattern bindings
            self.process_pattern(&arm.pattern);

            // Check guard
            if let Some(guard) = &arm.guard {
                self.check_expr(guard);
            }

            // Check body
            self.check_expr(&arm.body);

            self.borrows.kill_scope(scope_id);
            self.scopes.pop();

            branch_ownerships.push(self.ownership.snapshot());
            branch_moves.push(self.moves.snapshot());
        }

        // Conservative merge
        self.ownership = OwnershipState::merge(branch_ownerships);
        self.moves.merge(branch_moves);
    }

    // ─── Loop ──────────────────────────────────────────────────────

    pub(super) fn check_loop(&mut self, body: &HirExpr) {
        let scope_id = self.scopes.push(ScopeKind::Loop);

        self.check_expr(body);

        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── While ─────────────────────────────────────────────────────

    pub(super) fn check_while(&mut self, condition: &HirExpr, body: &HirExpr) {
        self.check_expr(condition);

        let scope_id = self.scopes.push(ScopeKind::Loop);

        self.check_expr(body);

        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── For ───────────────────────────────────────────────────────

    pub(super) fn check_for(&mut self, binding: DefId, iterable: &HirExpr, body: &HirExpr) {
        self.check_expr(iterable);

        let scope_id = self.scopes.push(ScopeKind::Loop);

        // Register the loop variable as mutable (for-loop bindings are implicitly let)
        // The type comes from the iterable's element type; we use a simplified approach
        self.register_binding(binding, &Ty::Infer(0), false, iterable.span.clone());

        self.check_expr(body);

        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── Closure ───────────────────────────────────────────────────

    pub(super) fn check_closure(
        &mut self,
        params: &[HirClosureParam],
        body: &HirExpr,
        captures: &[Capture],
        is_move: bool,
        span: &Span,
    ) {
        // Process captures. `cap.ty` is a resolve-time snapshot; the
        // post-typeck type lives in the symbol table and is what every
        // Copy/Send/mut-ref decision below MUST consult.
        // See `Capture::current_ty` docs.
        for cap in captures {
            let live_ty = cap.current_ty(self.symbols);
            if cap.by_move || is_move {
                // Move capture invalidates the outer binding
                if !self.ty_is_effectively_copy(&live_ty) {
                    self.moves.process_call_move(
                        cap.def_id,
                        "closure".to_string(),
                        &live_ty,
                        span.clone(),
                    );
                    self.ownership.record_move_into_call(
                        cap.def_id,
                        "closure".to_string(),
                        span.clone(),
                    );
                }
            } else {
                // Borrow capture: create a borrow
                let scope = self.scopes.current();
                let kind = if live_ty.is_mut_ref() {
                    BorrowKind::Mutable
                } else {
                    BorrowKind::Shared
                };
                self.borrows
                    .create(kind, cap.def_id, cap.def_id, span.clone(), scope);
            }
        }

        let scope_id = self.scopes.push(ScopeKind::Closure);

        // Register closure params
        for param in params {
            self.register_binding(param.def_id, &param.ty, false, param.span.clone());
        }

        self.check_expr(body);

        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── Pattern processing ────────────────────────────────────────

    pub(super) fn process_pattern(&mut self, pattern: &HirPattern) {
        match pattern {
            HirPattern::Binding {
                def_id,
                mutable,
                span,
                ..
            } => {
                // Register from the symbol table type if available, else use Infer
                let ty = self.symbols.def_ty(*def_id).unwrap_or(Ty::Infer(0));
                self.register_binding(*def_id, &ty, *mutable, span.clone());
            }
            HirPattern::Tuple { elements, .. } => {
                for elem in elements {
                    self.process_pattern(elem);
                }
            }
            HirPattern::Enum { fields, .. } => {
                for field in fields {
                    self.process_pattern(field);
                }
            }
            HirPattern::Struct { fields, .. } => {
                for (_, pat) in fields {
                    self.process_pattern(pat);
                }
            }
            HirPattern::Or { patterns, .. } => {
                for pat in patterns {
                    self.process_pattern(pat);
                }
            }
            HirPattern::Ref {
                def_id,
                mutable,
                span,
                ..
            } => {
                let ty = self.symbols.def_ty(*def_id).unwrap_or(Ty::Infer(0));
                self.register_binding(*def_id, &ty, *mutable, span.clone());
            }
            HirPattern::Wildcard { .. } | HirPattern::Literal { .. } | HirPattern::Rest { .. } => {}
        }
    }
}
