use super::*;
use crate::hir::visit::Visit;

impl<'a> BorrowChecker<'a> {
    // ─── Program / Item walking ────────────────────────────────────

    pub(super) fn check_program(&mut self, program: &HirProgram) {
        // Skip the prepended stdlib/prelude prefix (`prelude_item_count`): it is
        // trusted + e2e-verified, and its generic combinator bodies (abstract
        // `T`, e.g. `Enumerable#select`'s `pred.(x)` then `out.push(x)`) trip
        // false-positive move/borrow errors that don't reflect the monomorphized
        // reality. Only user-authored items get borrow-checked. (0 ⇒ check all.)
        for item in program.items.iter().skip(program.prelude_item_count) {
            self.check_item(item);
        }
    }

    /// Borrow-check every function body inside `item` (free functions,
    /// class / struct / enum methods and their inline `include` blocks,
    /// `extension` methods, mixin default methods) via the shared HIR walk.
    pub(super) fn check_item(&mut self, item: &HirItem) {
        self.visit_item(item);
    }

    // ─── Function ──────────────────────────────────────────────────

    pub(super) fn check_function(&mut self, func: &HirFuncDef) {
        // Push function scope
        let scope_id = self.scopes.push();

        // Register parameters
        for param in &func.params {
            self.register_binding(param.def_id, true, param.span.clone());
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
            | HirExprKind::RegexLiteral { .. }
            | HirExprKind::Error => {}

            HirExprKind::FieldAccess { object, .. } => {
                self.check_expr(object);
            }

            HirExprKind::MethodCall {
                object,
                resolved,
                method_name,
                args,
                block,
                ..
            } => {
                self.check_method_call(object, *resolved, method_name, args, block, &expr.span);
            }

            HirExprKind::FnCall {
                callee,
                callee_name,
                args,
            } => {
                self.check_fn_call(*callee, callee_name, args, &expr.span);
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

            HirExprKind::MultiAssign {
                targets, values, ..
            } => {
                // Parallel assignment: all values are read first, then each
                // target is written. Check the value expressions and that every
                // target is a mutable place; the actual move/store ordering is
                // handled at MIR. Modeled on `CompoundAssign` (targets are
                // pre-declared `var`s, so no fresh move-out tracking).
                for value in values {
                    self.check_expr(value);
                }
                for target in targets {
                    self.check_assign_target_mutability(target, &expr.span);
                    self.check_expr(target);
                }
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

            HirExprKind::Break(opt_expr) | HirExprKind::Next(opt_expr) => {
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
        let scope_id = self.scopes.push();

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
                ty: _,
                value,
                mutable,
                span,
            } => {
                self.check_let(*def_id, pattern, value.as_ref(), *mutable, span);
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
            let scope_id = self.scopes.push();

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
        let scope_id = self.scopes.push();

        self.check_expr(body);

        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── While ─────────────────────────────────────────────────────

    pub(super) fn check_while(&mut self, condition: &HirExpr, body: &HirExpr) {
        self.check_expr(condition);

        let scope_id = self.scopes.push();

        self.check_expr(body);

        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── For ───────────────────────────────────────────────────────

    pub(super) fn check_for(&mut self, binding: DefId, iterable: &HirExpr, body: &HirExpr) {
        self.check_expr(iterable);

        let scope_id = self.scopes.push();

        // Register the loop variable as mutable (for-loop bindings are implicitly let)
        // The type comes from the iterable's element type; we use a simplified approach
        self.register_binding(binding, false, iterable.span.clone());

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
        let mut moved_captures: Vec<DefId> = Vec::new();
        let mut borrowed_captures: Vec<(DefId, BorrowKind)> = Vec::new();
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
                    moved_captures.push(cap.def_id);
                }
            } else {
                let kind = if live_ty.is_mut_ref() {
                    BorrowKind::Mutable
                } else {
                    BorrowKind::Shared
                };
                borrowed_captures.push((cap.def_id, kind));
            }
        }

        let scope_id = self.scopes.push();

        // Register closure params
        for param in params {
            self.register_binding(param.def_id, false, param.span.clone());
        }

        // Q4: a move-captured value is owned by the closure, so the BODY may
        // freely use it — the move only invalidates the OUTER binding (after
        // the closure). Snapshot the post-capture move state, reinitialize
        // the captured bindings so body uses don't trip a false
        // use-after-move (E1001), walk the body, then restore so the outer
        // move persists for code after the closure.
        let moves_snapshot = self.moves.snapshot();
        for def_id in &moved_captures {
            self.moves.reinitialize(*def_id, span.clone());
        }

        self.check_expr(body);

        self.moves.restore(&moves_snapshot);
        self.borrows.kill_scope(scope_id);
        self.scopes.pop();

        // Borrow captures: the closure value holds these borrows from here
        // on. They are created after the body is checked because the body
        // reaches each captured variable through the capture itself, so a
        // body that calls a `def var` method on one does not conflict with
        // its own capture.
        let scope = self.scopes.current();
        for (def_id, kind) in borrowed_captures {
            self.borrows
                .create(kind, def_id, def_id, span.clone(), scope);
        }
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
                self.register_binding(*def_id, *mutable, span.clone());
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
                self.register_binding(*def_id, *mutable, span.clone());
            }
            HirPattern::Wildcard { .. } | HirPattern::Literal { .. } | HirPattern::Rest { .. } => {}
        }
    }
}

impl Visit for BorrowChecker<'_> {
    fn visit_func(&mut self, func: &HirFuncDef) {
        self.check_function(func);
    }

    /// `walk_item` reaches this only for a const initializer (function bodies
    /// stop at `visit_func`). Const initializers bind nothing and are not
    /// borrow-checked.
    fn visit_expr(&mut self, _expr: &HirExpr) {}
}
