pub mod borrows;
pub mod errors;
pub mod lifetimes;
pub mod moves;
pub mod ownership;
pub mod regions;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::hir::nodes::*;
use crate::hir::types::{MoveSemantics, Ty};
use crate::lexer::token::Span;
use crate::resolve::symbols::{ty_is_effectively_copy, DefKind, SymbolTable};

use self::borrows::{BorrowKind, BorrowSet};
use self::errors::{BorrowError, ErrorCode, SpanLabel};
use self::lifetimes::LifetimeChecker;
use self::moves::MoveChecker;
use self::ownership::OwnershipState;
use self::regions::{ScopeKind, ScopeStack};

// ─── Public entry point ────────────────────────────────────────────────

/// Run borrow checking on a typed HIR program.
///
/// Returns a list of all ownership and borrowing violations found.
pub fn borrow_check(program: &HirProgram, symbols: &SymbolTable) -> Vec<BorrowError> {
    let mut checker = BorrowChecker::new(symbols);
    checker.check_program(program);
    checker.errors
}

// ─── BorrowChecker ─────────────────────────────────────────────────────

struct BorrowChecker<'a> {
    symbols: &'a SymbolTable,
    scopes: ScopeStack,
    ownership: OwnershipState,
    borrows: BorrowSet,
    moves: MoveChecker,
    lifetimes: LifetimeChecker,
    /// Tracks whether each DefId is mutable (let mut) or immutable (let).
    mutability: HashMap<DefId, bool>,
    /// Maps reference variables to the place they borrow from.
    /// e.g., `let r = &v` → ref_bindings[r_def_id] = v_def_id
    ref_bindings: HashMap<DefId, DefId>,
    errors: Vec<BorrowError>,
}

impl<'a> BorrowChecker<'a> {
    fn new(symbols: &'a SymbolTable) -> Self {
        Self {
            symbols,
            scopes: ScopeStack::new(),
            ownership: OwnershipState::new(),
            borrows: BorrowSet::new(),
            moves: MoveChecker::new(),
            lifetimes: LifetimeChecker::new(),
            mutability: HashMap::new(),
            ref_bindings: HashMap::new(),
            errors: Vec::new(),
        }
    }

    // ─── Program / Item walking ────────────────────────────────────

    fn check_program(&mut self, program: &HirProgram) {
        for item in &program.items {
            self.check_item(item);
        }
    }

    fn check_item(&mut self, item: &HirItem) {
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

    fn check_impl_block(&mut self, imp: &HirImplBlock) {
        for impl_item in &imp.items {
            if let HirImplItem::Method(func) = impl_item {
                self.check_function(func);
            }
        }
    }

    // ─── Function ──────────────────────────────────────────────────

    fn check_function(&mut self, func: &HirFuncDef) {
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

    fn check_expr(&mut self, expr: &HirExpr) {
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

    // ─── VarRef ────────────────────────────────────────────────────

    fn check_var_ref(&mut self, def_id: DefId, span: &Span) {
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

    fn check_let(
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

    fn check_assign(&mut self, target: &HirExpr, value: &HirExpr, semantics: MoveSemantics) {
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

    fn check_assign_target_mutability(&mut self, target: &HirExpr, span: &Span) {
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

    fn check_borrow(&mut self, mutable: bool, inner: &HirExpr, span: &Span) {
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

    fn check_fn_call(&mut self, callee_name: &str, args: &[HirExpr], _span: &Span) {
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
            // Structs that `derive Copy` are treated as Copy here.
            if let HirExprKind::VarRef(source_id) = &arg.kind {
                if !self.ty_is_effectively_copy(&arg.ty) {
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

    fn lookup_send_required_params(&self, callee_name: &str, arg_len: usize) -> Vec<bool> {
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

    fn check_send_required_closure(&mut self, captures: &[Capture], is_move: bool, span: &Span) {
        for cap in captures {
            if cap.by_move || is_move {
                if !cap.ty.is_send_with(self.symbols) {
                    self.errors.push(BorrowError {
                        code: ErrorCode::E1011,
                        primary: SpanLabel {
                            span: span.clone(),
                            label: format!(
                                "captured value `{}` of type `{}` is not `Send`",
                                cap.name, cap.ty
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

    fn check_method_call(
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
            if let HirExprKind::VarRef(source_id) = &arg.kind {
                if !self.ty_is_effectively_copy(&arg.ty) {
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

                // Name-based iterator ownership
                match method_name {
                    "iter" => {
                        let scope = self.scopes.current();
                        self.borrows.create(
                            BorrowKind::Shared,
                            *obj_id,
                            *obj_id,
                            span.clone(),
                            scope,
                        );
                    }
                    "into_iter" => {
                        if !self.ty_is_effectively_copy(&object.ty) {
                            self.moves.process_call_move(
                                *obj_id,
                                "into_iter".to_string(),
                                &object.ty,
                                span.clone(),
                            );
                            self.ownership.record_move_into_call(
                                *obj_id,
                                "into_iter".to_string(),
                                span.clone(),
                            );
                        }
                    }
                    _ => {}
                }
            }
        }

        // Kill temporary borrows from args — they're consumed by the callee
        self.borrows.kill_after_checkpoint(checkpoint);
    }

    // ─── Block ─────────────────────────────────────────────────────

    fn check_block(&mut self, stmts: &[HirStatement], tail: Option<&HirExpr>) {
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

    fn check_statement(&mut self, stmt: &HirStatement) {
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

    fn check_if(&mut self, cond: &HirExpr, then_branch: &HirExpr, else_branch: Option<&HirExpr>) {
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

    fn check_match(&mut self, scrutinee: &HirExpr, arms: &[HirMatchArm]) {
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

    fn check_loop(&mut self, body: &HirExpr) {
        let scope_id = self.scopes.push(ScopeKind::Loop);

        self.check_expr(body);

        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── While ─────────────────────────────────────────────────────

    fn check_while(&mut self, condition: &HirExpr, body: &HirExpr) {
        self.check_expr(condition);

        let scope_id = self.scopes.push(ScopeKind::Loop);

        self.check_expr(body);

        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── For ───────────────────────────────────────────────────────

    fn check_for(&mut self, binding: DefId, iterable: &HirExpr, body: &HirExpr) {
        self.check_expr(iterable);

        let scope_id = self.scopes.push(ScopeKind::Loop);

        // Register the loop variable as mutable (for-loop bindings are implicitly let)
        // The type comes from the iterable's element type; we use a simplified approach
        self.register_binding(binding, &Ty::Infer(0), false, iterable.span.clone());

        self.check_expr(body);

        self.borrows.kill_scope(scope_id);
        self.scopes.pop();
    }

    // ─── Return ────────────────────────────────────────────────────

    fn check_return(&mut self, opt_expr: Option<&HirExpr>, span: &Span) {
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

    // ─── Closure ───────────────────────────────────────────────────

    fn check_closure(
        &mut self,
        params: &[HirClosureParam],
        body: &HirExpr,
        captures: &[Capture],
        is_move: bool,
        span: &Span,
    ) {
        // Process captures
        for cap in captures {
            if cap.by_move || is_move {
                // Move capture invalidates the outer binding
                if !self.ty_is_effectively_copy(&cap.ty) {
                    self.moves.process_call_move(
                        cap.def_id,
                        "closure".to_string(),
                        &cap.ty,
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
                let kind = if cap.ty.is_mut_ref() {
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

    fn process_pattern(&mut self, pattern: &HirPattern) {
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

    // ─── Helpers ───────────────────────────────────────────────────

    /// Register a new binding across all sub-analyzers.
    fn register_binding(&mut self, def_id: DefId, ty: &Ty, mutable: bool, span: Span) {
        self.scopes.register_binding(def_id);
        self.ownership.declare(def_id);
        self.moves.declare(def_id, ty.clone(), span);
        self.mutability.insert(def_id, mutable);
    }

    /// Check if a DefId is mutable.
    fn is_mutable(&self, def_id: DefId) -> bool {
        // First check our local mutability map
        if let Some(&m) = self.mutability.get(&def_id) {
            return m;
        }
        // Fall back to the symbol table
        if let Some(def) = self.symbols.get(def_id) {
            match &def.kind {
                DefKind::Variable { mutable, .. } => return *mutable,
                DefKind::Param { .. } => return false,
                DefKind::SelfValue { .. } => return false,
                _ => {}
            }
        }
        false
    }

    /// Get a human-readable name for a DefId.
    fn def_name(&self, def_id: DefId) -> String {
        self.symbols
            .get(def_id)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| format!("_{}", def_id))
    }

    /// Returns `true` if the given value should be copied (not moved) on
    /// assignment or argument passing.  Extends `Ty::is_copy` by consulting
    /// the `derive_traits` list on user-defined structs — a struct with
    /// `derive Copy` behaves like a primitive for move analysis.
    fn ty_is_effectively_copy(&self, ty: &Ty) -> bool {
        ty_is_effectively_copy(ty, self.symbols)
    }
}

fn ty_has_bound(ty: &Ty, bound_name: &str) -> bool {
    match ty {
        Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
            bounds.iter().any(|bound| bound.name == bound_name)
        }
        _ => false,
    }
}
