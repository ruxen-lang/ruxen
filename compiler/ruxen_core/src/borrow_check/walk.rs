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
        // Register parameters
        for param in &func.params {
            self.register_binding(param.def_id, true, param.span.clone());
        }
        let block_params = func
            .params
            .iter()
            .filter(|p| p.ty.is_borrowed_block())
            .map(|p| p.def_id)
            .collect();
        let outer_block_params = std::mem::replace(&mut self.block_params, block_params);
        let handle_refmut_params = func
            .params
            .iter()
            .filter(|p| {
                matches!(p.ty, Ty::RefMut(_) | Ty::RefMutLifetime(..))
                    && !p.ty.refmut_is_slot_address()
            })
            .map(|p| (p.def_id, p.ty.clone()))
            .collect();
        let outer_handle_refmut_params =
            std::mem::replace(&mut self.handle_refmut_params, handle_refmut_params);

        self.check_expr(&func.body);
        self.block_params = outer_block_params;
        self.handle_refmut_params = outer_handle_refmut_params;
    }

    // ─── Expressions ───────────────────────────────────────────────

    pub(super) fn check_expr(&mut self, expr: &HirExpr) {
        match &expr.kind {
            HirExprKind::VarRef(def_id) => {
                let allowed = std::mem::take(&mut self.block_use_allowed);
                if !allowed && self.block_params.contains(def_id) {
                    self.report_stored_block(*def_id, &expr.span);
                }
                self.check_var_ref(*def_id, &expr.span);
            }

            HirExprKind::Continue => self.record_loop_exit(),

            HirExprKind::IntLiteral(_)
            | HirExprKind::FloatLiteral(_)
            | HirExprKind::StringLiteral(_)
            | HirExprKind::BoolLiteral(_)
            | HirExprKind::CharLiteral(_)
            | HirExprKind::UnitLiteral
            | HirExprKind::RegexLiteral { .. }
            | HirExprKind::Error => {}

            // Reading a field only reads the receiver.
            HirExprKind::FieldAccess { object, .. } => {
                self.check_expr(object);
            }

            HirExprKind::MethodCall {
                object,
                resolved,
                requirement_params,
                method_name,
                args,
                block,
                ..
            } => {
                self.check_method_call(
                    object,
                    *resolved,
                    requirement_params.as_deref(),
                    method_name,
                    args,
                    block,
                    &expr.span,
                );
            }

            HirExprKind::FnCall {
                callee,
                callee_name,
                args,
            } => {
                self.check_fn_call(*callee, callee_name, args);
            }

            HirExprKind::BinaryOp { op, left, right } => {
                // `block_given?` desugars to `__block != nil`: testing a block
                // against `nil` does not store it.
                let nil_test = matches!(op, BinOp::Eq | BinOp::NotEq);
                if nil_test && matches!(right.kind, HirExprKind::NullLiteral) {
                    self.allow_block_use(left);
                }
                self.check_expr(left);
                if nil_test && matches!(left.kind, HirExprKind::NullLiteral) {
                    self.allow_block_use(right);
                }
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
                self.check_multi_assign(targets, values, &expr.span);
            }

            HirExprKind::CompoundAssign { target, value, .. } => {
                // Compound assign requires mutability of target.
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
                    self.consume(inner, MoveInto::Binding(None));
                }
                self.record_loop_exit();
            }

            HirExprKind::Closure {
                params,
                body,
                captures,
                is_move,
                kind,
                ..
            } => {
                self.check_closure(params, body, captures, *is_move, *kind, &expr.span);
            }

            // A literal stores each element by value.
            HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
                self.check_elements(fields.iter().map(|(_, value)| value));
            }

            HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
                self.check_elements(elems.iter());
            }

            HirExprKind::Index { object, index } => {
                self.check_expr(object);
                self.check_expr(index);
            }

            HirExprKind::Cast { expr: inner, .. } => {
                self.check_expr(inner);
            }

            HirExprKind::MapLiteral(entries) => {
                self.check_elements(entries.iter().flat_map(|(k, v)| [k, v]));
            }

            HirExprKind::ArrayFill { value, .. } => {
                self.check_expr(value);
                self.consume(value, MoveInto::Binding(None));
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

    /// The elements of a literal, in order: each is stored by value.
    fn check_elements<'e>(&mut self, elems: impl Iterator<Item = &'e HirExpr>) {
        for elem in elems {
            self.check_expr(elem);
            self.consume(elem, MoveInto::Binding(None));
        }
    }

    // ─── Block ─────────────────────────────────────────────────────

    pub(super) fn check_block(&mut self, stmts: &[HirStatement], tail: Option<&HirExpr>) {
        for stmt in stmts {
            self.check_statement(stmt);
        }

        if let Some(tail_expr) = tail {
            self.check_expr(tail_expr);
        }
    }

    // ─── Multi-assign ──────────────────────────────────────────────

    /// Parallel assignment (`q, n = s, 1`): every value is read first, then
    /// each target is written. A value stored into its target moves a
    /// non-Copy variable it yields; a target variable is written, not read,
    /// so it is live again afterwards (`a, b = b, a` swaps). Targets are
    /// pre-declared `var`s. A single right-hand value split over several
    /// targets (`a, b = pair`) is destructured by MIR element by element and
    /// is left as a read here.
    pub(super) fn check_multi_assign(
        &mut self,
        targets: &[HirExpr],
        values: &[HirExpr],
        span: &Span,
    ) {
        for value in values {
            self.check_expr(value);
        }
        for target in targets {
            self.check_assign_target_mutability(target, span);
            if !matches!(target.kind, HirExprKind::VarRef(_)) {
                self.check_expr(target);
            }
        }
        if values.len() == targets.len() {
            for (target, value) in targets.iter().zip(values) {
                let target_def = match &target.kind {
                    HirExprKind::VarRef(def_id) => Some(*def_id),
                    _ => None,
                };
                self.consume(value, MoveInto::Binding(target_def));
            }
        }
        for target in targets {
            if let HirExprKind::VarRef(def_id) = &target.kind {
                if self.ownership.is_moved(*def_id) {
                    self.ownership.reinitialize(*def_id);
                    self.moves.reinitialize(*def_id, target.span.clone());
                }
            }
        }
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

        // Walk then-branch
        self.check_expr(then_branch);
        let then_ownership = self.ownership.snapshot();
        let then_moves = self.moves.snapshot();

        // Restore for else-branch
        self.ownership = ownership_snap.clone();
        self.moves.restore(&moves_snap);

        if let Some(else_br) = else_branch {
            self.check_expr(else_br);
        }
        let else_ownership = self.ownership.snapshot();
        let else_moves = self.moves.snapshot();

        // Merge: conservative — moved on ANY branch → moved after. A
        // branch that never reaches the code after the `if` (it returns,
        // breaks or nexts) does not take part.
        let mut ownerships = Vec::new();
        let mut moves = Vec::new();
        if !diverges(then_branch) {
            ownerships.push(then_ownership);
            moves.push(then_moves);
        }
        if !else_branch.is_some_and(diverges) {
            ownerships.push(else_ownership);
            moves.push(else_moves);
        }
        self.merge_branches(ownerships, moves, ownership_snap, &moves_snap);
    }

    /// Install the merged state of the branches that fall through to the
    /// code after them; when none does, that code is unreachable and the
    /// state from before the branches stands.
    fn merge_branches(
        &mut self,
        ownerships: Vec<OwnershipState>,
        moves: Vec<MoveChecker>,
        ownership_before: OwnershipState,
        moves_before: &MoveChecker,
    ) {
        if ownerships.is_empty() {
            self.ownership = ownership_before;
            self.moves.restore(moves_before);
            return;
        }
        self.ownership = OwnershipState::merge(ownerships);
        self.moves.merge(moves);
    }

    /// `break` / `next` / `continue`: record the move state they carry to
    /// the innermost loop's exit.
    fn record_loop_exit(&mut self) {
        let state = (self.ownership.snapshot(), self.moves.snapshot());
        if let Some(frame) = self.loop_exits.last_mut() {
            frame.push(state);
        }
    }

    /// Pop the innermost loop's frame and merge every state that left the
    /// loop body through `break` / `next` / `continue` into the state after
    /// it: a move made before a `break` is a move for the code after the
    /// loop (I-16).
    fn exit_loop(&mut self) {
        let exits = self.loop_exits.pop().unwrap_or_default();
        if exits.is_empty() {
            return;
        }
        let mut ownerships = vec![self.ownership.snapshot()];
        let mut moves = vec![self.moves.snapshot()];
        for (ownership, moved) in exits {
            ownerships.push(ownership);
            moves.push(moved);
        }
        self.ownership = OwnershipState::merge(ownerships);
        self.moves.merge(moves);
    }

    // ─── Match ─────────────────────────────────────────────────────

    pub(super) fn check_match(&mut self, scrutinee: &HirExpr, arms: &[HirMatchArm]) {
        self.check_expr(scrutinee);

        if arms.is_empty() {
            return;
        }

        let ownership_snap = self.ownership.snapshot();
        let moves_snap = self.moves.snapshot();

        let mut branch_ownerships = Vec::new();
        let mut branch_moves = Vec::new();

        for arm in arms {
            // Restore state for each arm
            self.ownership = ownership_snap.clone();
            self.moves.restore(&moves_snap);

            // A by-value binding of a non-Copy payload moves the scrutinee
            // into the arm (P-3): two `if let Some(v) = o` would hand the
            // same String to two owning bindings.
            if self.pattern_binds_by_value(&arm.pattern) {
                self.consume(scrutinee, MoveInto::Binding(None));
            }

            // Process pattern bindings
            self.process_pattern(&arm.pattern);

            // Check guard
            if let Some(guard) = &arm.guard {
                self.check_expr(guard);
            }

            // Check body
            self.check_expr(&arm.body);

            if !diverges(&arm.body) {
                branch_ownerships.push(self.ownership.snapshot());
                branch_moves.push(self.moves.snapshot());
            }
        }

        // Conservative merge over the arms that fall through.
        self.merge_branches(branch_ownerships, branch_moves, ownership_snap, &moves_snap);
    }

    // ─── Loop ──────────────────────────────────────────────────────

    pub(super) fn check_loop(&mut self, body: &HirExpr) {
        self.loop_exits.push(Vec::new());
        self.check_expr(body);
        self.exit_loop();
    }

    // ─── While ─────────────────────────────────────────────────────

    pub(super) fn check_while(&mut self, condition: &HirExpr, body: &HirExpr) {
        self.check_expr(condition);

        self.loop_exits.push(Vec::new());
        self.check_expr(body);
        self.exit_loop();
    }

    // ─── For ───────────────────────────────────────────────────────

    pub(super) fn check_for(&mut self, binding: DefId, iterable: &HirExpr, body: &HirExpr) {
        self.check_expr(iterable);

        // Register the loop variable as mutable (for-loop bindings are implicitly let)
        // The type comes from the iterable's element type; we use a simplified approach
        self.register_binding(binding, false, iterable.span.clone());

        self.loop_exits.push(Vec::new());
        self.check_expr(body);
        self.exit_loop();
    }

    // ─── Closure ───────────────────────────────────────────────────

    pub(super) fn check_closure(
        &mut self,
        params: &[HirClosureParam],
        body: &HirExpr,
        captures: &[Capture],
        is_move: bool,
        kind: ClosureKind,
        span: &Span,
    ) {
        // Process captures. `cap.ty` is a resolve-time snapshot; the
        // post-typeck type lives in the symbol table and is what every
        // Copy/Send/mut-ref decision below MUST consult.
        // See `Capture::current_ty` docs.
        //
        // A block borrows its captures for the call it is attached to; a proc,
        // a lambda and any `move` closure own them, so a non-Copy capture is a
        // move and a later use (or a second owning capture) is E1001. An owning
        // closure must not capture a block parameter: that stores it (E1018).
        //
        // Inside the body every non-Copy capture is a borrowed place
        // (`body_borrows`), whatever the kind: the enclosing scope frees a
        // block's captures and the closure frees its own on drop, so a move
        // out of the body (each call would hand out the same value) is
        // E1019, reported by `record_move`. That includes an owning closure
        // nested in this body capturing one of them.
        let mut moved_captures: Vec<DefId> = Vec::new();
        let mut body_borrows: HashSet<DefId> = HashSet::new();
        for cap in captures {
            let live_ty = cap.current_ty(self.symbols);
            let owns = closure_owns(cap, is_move, kind);
            if owns && self.block_params.contains(&cap.def_id) {
                self.report_stored_block(cap.def_id, span);
            }
            let copy = self.ty_is_effectively_copy(&live_ty);
            if !copy && !live_ty.is_mut_ref() {
                body_borrows.insert(cap.def_id);
            }
            if owns && !copy {
                // Capturing an already-moved value is a use after move.
                self.check_var_ref(cap.def_id, span);
                if self.record_move(cap.def_id, &live_ty, MoveInto::Closure, span) {
                    moved_captures.push(cap.def_id);
                }
            }
        }

        // Register closure params. A closure parameter typed `Fn[...]` is a
        // block parameter inside the body, like a function's: the outer
        // block parameters stay usable too (a borrowing block may call them;
        // an owning closure that captured one was reported above).
        for param in params {
            self.register_binding(param.def_id, false, param.span.clone());
        }
        let mut body_block_params = self.block_params.clone();
        body_block_params.extend(
            params
                .iter()
                .filter(|p| p.ty.is_borrowed_block())
                .map(|p| p.def_id),
        );
        let outer_block_params = std::mem::replace(&mut self.block_params, body_block_params);

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
        let outer_capture_borrows = std::mem::replace(&mut self.capture_borrows, body_borrows);

        // `next` in a closure body returns from the closure: its own frame,
        // discarded with the body's state below.
        let outer_loop_exits = std::mem::take(&mut self.loop_exits);
        self.check_expr(body);
        self.loop_exits = outer_loop_exits;
        // The body's value is returned to the caller by value.
        self.consume(body, MoveInto::Binding(None));
        self.capture_borrows = outer_capture_borrows;
        self.block_params = outer_block_params;

        self.moves.restore(&moves_snapshot);
    }

    // ─── Pattern processing ────────────────────────────────────────

    /// Whether `pattern` binds a non-Copy value by value (a `ref` binding,
    /// `_`, a literal or `..` binds nothing owned). A binding whose type
    /// typeck left unrecorded is treated as binding nothing.
    pub(super) fn pattern_binds_by_value(&self, pattern: &HirPattern) -> bool {
        match pattern {
            HirPattern::Binding { def_id, .. } => self
                .symbols
                .def_ty(*def_id)
                .is_some_and(|ty| !self.ty_is_effectively_copy(&ty)),
            HirPattern::Tuple { elements, .. } => {
                elements.iter().any(|p| self.pattern_binds_by_value(p))
            }
            HirPattern::Enum { fields, .. } => {
                fields.iter().any(|p| self.pattern_binds_by_value(p))
            }
            HirPattern::Struct { fields, .. } => {
                fields.iter().any(|(_, p)| self.pattern_binds_by_value(p))
            }
            HirPattern::Or { patterns, .. } => {
                patterns.iter().any(|p| self.pattern_binds_by_value(p))
            }
            HirPattern::Ref { .. }
            | HirPattern::Wildcard { .. }
            | HirPattern::Literal { .. }
            | HirPattern::Rest { .. } => false,
        }
    }

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

/// Whether control never leaves `expr` normally: it ends in `return`,
/// `break`, `next` or `continue` on every path (a block whose last
/// statement or tail does, an `if` with an `else` or a `match` whose every
/// branch does), or typeck typed it `Never`.
fn diverges(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Return(_)
        | HirExprKind::Break(_)
        | HirExprKind::Next(_)
        | HirExprKind::Continue => true,
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            tail.as_deref().is_some_and(diverges)
                || stmts
                    .iter()
                    .any(|stmt| matches!(stmt, HirStatement::Expr(e) if diverges(e)))
        }
        HirExprKind::If {
            then_branch,
            else_branch: Some(else_branch),
            ..
        } => diverges(then_branch) && diverges(else_branch),
        HirExprKind::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|arm| diverges(&arm.body))
        }
        _ => matches!(expr.ty, crate::hir::types::Ty::Never),
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
