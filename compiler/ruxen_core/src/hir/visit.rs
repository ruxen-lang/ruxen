//! Exhaustive, shared traversal of the typed HIR.
//!
//! The HIR counterpart of `parser/visit.rs`. `Visit` (immutable) and
//! `VisitMut` (mutating) each provide a default method per node kind; the free
//! `walk_*` functions hold the ONE exhaustive match over `HirItem`,
//! `HirExprKind`, `HirStatement` and `HirPattern`. A consumer overrides only
//! the nodes it cares about and calls the matching `walk_*` to recurse, or
//! does NOT call it to treat a subtree as opaque (a nested loop for `break`
//! collection, a nested closure for `next`). There is no `_ =>` arm in any
//! `walk_*`: adding a HIR variant is a compile error here until it is
//! handled, which is what ends the variant-drift bug class (a hand-written
//! walker silently skipping `UnsafeBlock`, `Tuple` or a struct method body).
//!
//! What gets visited:
//!   * `walk_item` reaches every function body (free functions, class /
//!     struct / enum methods, methods of their inline `include` blocks,
//!     `extension` methods, mixin default methods) and every const
//!     initializer. It does not visit item-level type annotations (fields,
//!     variants, aliases, newtypes, mixin signatures); a consumer that needs
//!     those reads them in its `visit_item`.
//!   * `walk_func` visits the parameter types, the return type and the body.
//!     A parameter's default is a parser `Expr`, not HIR, and is not visited.
//!   * `walk_expr` calls `visit_ty` on the expression's own type, on a cast
//!     target, on closure parameter types and on explicit method generic
//!     arguments, then descends into every nested expression, statement and
//!     pattern. `Capture::ty` is a resolve-time snapshot (see its docs) and is
//!     not visited.
//!   * `walk_stmt` visits a `let`'s pattern, its declared type and its value.

use crate::hir::nodes::*;
use crate::hir::types::Ty;

/// Read-only HIR traversal. Every method defaults to the matching `walk_*`.
pub trait Visit: Sized {
    fn visit_item(&mut self, item: &HirItem) {
        walk_item(self, item);
    }
    fn visit_func(&mut self, func: &HirFuncDef) {
        walk_func(self, func);
    }
    fn visit_expr(&mut self, expr: &HirExpr) {
        walk_expr(self, expr);
    }
    fn visit_stmt(&mut self, stmt: &HirStatement) {
        walk_stmt(self, stmt);
    }
    fn visit_pattern(&mut self, pattern: &HirPattern) {
        walk_pattern(self, pattern);
    }
    /// Called on every `Ty` that `walk_expr`, `walk_stmt` and `walk_func`
    /// reach (see the module docs). A no-op by default.
    fn visit_ty(&mut self, _ty: &Ty) {}
}

/// Visit every function body and const initializer inside `item`.
pub fn walk_item<V: Visit>(v: &mut V, item: &HirItem) {
    match item {
        HirItem::Module(m) => {
            for sub in &m.items {
                v.visit_item(sub);
            }
        }
        HirItem::Class(c) => {
            for m in &c.methods {
                v.visit_func(m);
            }
            for b in &c.impl_blocks {
                walk_impl_block(v, b);
            }
        }
        HirItem::Struct(s) => {
            for m in &s.methods {
                v.visit_func(m);
            }
            for b in &s.impl_blocks {
                walk_impl_block(v, b);
            }
        }
        HirItem::Enum(e) => {
            for m in &e.methods {
                v.visit_func(m);
            }
            for b in &e.impl_blocks {
                walk_impl_block(v, b);
            }
        }
        HirItem::Mixin(m) => {
            for it in &m.items {
                match it {
                    HirMixinItem::DefaultMethod(f) => v.visit_func(f),
                    // Signatures only: no body to visit.
                    HirMixinItem::AssocType { .. } | HirMixinItem::MethodSig { .. } => {}
                }
            }
        }
        HirItem::Impl(b) => walk_impl_block(v, b),
        HirItem::Function(f) => v.visit_func(f),
        HirItem::Const(c) => v.visit_expr(&c.value),
        // Pure type-level items: no body.
        HirItem::TypeAlias(_) | HirItem::Newtype(_) => {}
    }
}

/// Visit every method of an `extension` / inline `include` block.
pub fn walk_impl_block<V: Visit>(v: &mut V, block: &HirImplBlock) {
    for it in &block.items {
        match it {
            HirImplItem::Method(f) => v.visit_func(f),
            HirImplItem::AssocType { .. } | HirImplItem::Include { .. } => {}
        }
    }
}

/// Visit a function's parameter types, return type and body.
pub fn walk_func<V: Visit>(v: &mut V, func: &HirFuncDef) {
    for p in &func.params {
        v.visit_ty(&p.ty);
    }
    v.visit_ty(&func.return_ty);
    v.visit_expr(&func.body);
}

pub fn walk_stmt<V: Visit>(v: &mut V, stmt: &HirStatement) {
    match stmt {
        HirStatement::Let {
            pattern, ty, value, ..
        } => {
            v.visit_pattern(pattern);
            v.visit_ty(ty);
            if let Some(val) = value {
                v.visit_expr(val);
            }
        }
        HirStatement::Expr(e) => v.visit_expr(e),
    }
}

pub fn walk_pattern<V: Visit>(v: &mut V, pattern: &HirPattern) {
    match pattern {
        HirPattern::Literal { expr, .. } => v.visit_expr(expr),
        HirPattern::Tuple { elements, .. } => {
            for sub in elements {
                v.visit_pattern(sub);
            }
        }
        HirPattern::Enum { fields, .. } => {
            for sub in fields {
                v.visit_pattern(sub);
            }
        }
        HirPattern::Struct { fields, .. } => {
            for (_, sub) in fields {
                v.visit_pattern(sub);
            }
        }
        HirPattern::Or { patterns, .. } => {
            for sub in patterns {
                v.visit_pattern(sub);
            }
        }
        HirPattern::Binding { .. }
        | HirPattern::Wildcard { .. }
        | HirPattern::Ref { .. }
        | HirPattern::Rest { .. } => {}
    }
}

pub fn walk_expr<V: Visit>(v: &mut V, expr: &HirExpr) {
    v.visit_ty(&expr.ty);
    match &expr.kind {
        // Leaves: no nested HIR node.
        HirExprKind::IntLiteral(_)
        | HirExprKind::FloatLiteral(_)
        | HirExprKind::StringLiteral(_)
        | HirExprKind::BoolLiteral(_)
        | HirExprKind::CharLiteral(_)
        | HirExprKind::UnitLiteral
        | HirExprKind::RegexLiteral { .. }
        | HirExprKind::VarRef(_)
        | HirExprKind::Continue
        | HirExprKind::NullLiteral
        | HirExprKind::Error => {}

        HirExprKind::FieldAccess { object, .. } => v.visit_expr(object),
        HirExprKind::MethodCall {
            object,
            generic_args,
            args,
            block,
            ..
        } => {
            v.visit_expr(object);
            for t in generic_args {
                v.visit_ty(t);
            }
            for a in args {
                v.visit_expr(a);
            }
            if let Some(b) = block {
                v.visit_expr(b);
            }
        }
        HirExprKind::FnCall { args, .. } | HirExprKind::MacroCall { args, .. } => {
            for a in args {
                v.visit_expr(a);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            v.visit_expr(left);
            v.visit_expr(right);
        }
        HirExprKind::UnaryOp { operand, .. } => v.visit_expr(operand),
        HirExprKind::Borrow { expr: inner, .. } => v.visit_expr(inner),
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            for s in stmts {
                v.visit_stmt(s);
            }
            if let Some(t) = tail {
                v.visit_expr(t);
            }
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            v.visit_expr(cond);
            v.visit_expr(then_branch);
            if let Some(e) = else_branch {
                v.visit_expr(e);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            v.visit_expr(scrutinee);
            for arm in arms {
                v.visit_pattern(&arm.pattern);
                if let Some(g) = &arm.guard {
                    v.visit_expr(g);
                }
                v.visit_expr(&arm.body);
            }
        }
        HirExprKind::Loop { body } => v.visit_expr(body),
        HirExprKind::While { condition, body } => {
            v.visit_expr(condition);
            v.visit_expr(body);
        }
        HirExprKind::For { iterable, body, .. } => {
            v.visit_expr(iterable);
            v.visit_expr(body);
        }
        HirExprKind::Assign { target, value, .. }
        | HirExprKind::CompoundAssign { target, value, .. } => {
            v.visit_expr(target);
            v.visit_expr(value);
        }
        HirExprKind::MultiAssign {
            targets, values, ..
        } => {
            for t in targets {
                v.visit_expr(t);
            }
            for val in values {
                v.visit_expr(val);
            }
        }
        HirExprKind::Return(value) | HirExprKind::Break(value) | HirExprKind::Next(value) => {
            if let Some(val) = value {
                v.visit_expr(val);
            }
        }
        HirExprKind::Closure { params, body, .. } => {
            for p in params {
                v.visit_ty(&p.ty);
            }
            v.visit_expr(body);
        }
        HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
            for (_, f) in fields {
                v.visit_expr(f);
            }
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            for e in elems {
                v.visit_expr(e);
            }
        }
        HirExprKind::Index { object, index } => {
            v.visit_expr(object);
            v.visit_expr(index);
        }
        HirExprKind::Cast {
            expr: inner,
            target,
        } => {
            v.visit_expr(inner);
            v.visit_ty(target);
        }
        HirExprKind::MapLiteral(pairs) => {
            for (k, val) in pairs {
                v.visit_expr(k);
                v.visit_expr(val);
            }
        }
        HirExprKind::ArrayFill { value, .. } => v.visit_expr(value),
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                v.visit_expr(s);
            }
            if let Some(e) = end {
                v.visit_expr(e);
            }
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                match p {
                    HirInterpolationPart::Literal(_) => {}
                    HirInterpolationPart::Expr { expr: e, .. } => v.visit_expr(e),
                }
            }
        }
    }
}

/// Mutating HIR traversal: the `&mut` mirror of [`Visit`] for function
/// bodies. Item-level walking is read-only today (no mutating consumer
/// needs it).
pub trait VisitMut: Sized {
    fn visit_func_mut(&mut self, func: &mut HirFuncDef) {
        walk_func_mut(self, func);
    }
    fn visit_expr_mut(&mut self, expr: &mut HirExpr) {
        walk_expr_mut(self, expr);
    }
    fn visit_stmt_mut(&mut self, stmt: &mut HirStatement) {
        walk_stmt_mut(self, stmt);
    }
    fn visit_pattern_mut(&mut self, pattern: &mut HirPattern) {
        walk_pattern_mut(self, pattern);
    }
    /// Called on every `Ty` the `walk_*_mut` functions reach, the same set as
    /// [`Visit::visit_ty`]. A no-op by default.
    fn visit_ty_mut(&mut self, _ty: &mut Ty) {}
}

/// `&mut` mirror of [`walk_func`].
pub fn walk_func_mut<V: VisitMut>(v: &mut V, func: &mut HirFuncDef) {
    for p in &mut func.params {
        v.visit_ty_mut(&mut p.ty);
    }
    v.visit_ty_mut(&mut func.return_ty);
    v.visit_expr_mut(&mut func.body);
}

/// `&mut` mirror of [`walk_stmt`].
pub fn walk_stmt_mut<V: VisitMut>(v: &mut V, stmt: &mut HirStatement) {
    match stmt {
        HirStatement::Let {
            pattern, ty, value, ..
        } => {
            v.visit_pattern_mut(pattern);
            v.visit_ty_mut(ty);
            if let Some(val) = value {
                v.visit_expr_mut(val);
            }
        }
        HirStatement::Expr(e) => v.visit_expr_mut(e),
    }
}

/// `&mut` mirror of [`walk_pattern`].
pub fn walk_pattern_mut<V: VisitMut>(v: &mut V, pattern: &mut HirPattern) {
    match pattern {
        HirPattern::Literal { expr, .. } => v.visit_expr_mut(expr),
        HirPattern::Tuple { elements, .. } => {
            for sub in elements {
                v.visit_pattern_mut(sub);
            }
        }
        HirPattern::Enum { fields, .. } => {
            for sub in fields {
                v.visit_pattern_mut(sub);
            }
        }
        HirPattern::Struct { fields, .. } => {
            for (_, sub) in fields {
                v.visit_pattern_mut(sub);
            }
        }
        HirPattern::Or { patterns, .. } => {
            for sub in patterns {
                v.visit_pattern_mut(sub);
            }
        }
        HirPattern::Binding { .. }
        | HirPattern::Wildcard { .. }
        | HirPattern::Ref { .. }
        | HirPattern::Rest { .. } => {}
    }
}

/// Exhaustive `&mut` mirror of [`walk_expr`] (no `_` arm).
pub fn walk_expr_mut<V: VisitMut>(v: &mut V, expr: &mut HirExpr) {
    v.visit_ty_mut(&mut expr.ty);
    match &mut expr.kind {
        HirExprKind::IntLiteral(_)
        | HirExprKind::FloatLiteral(_)
        | HirExprKind::StringLiteral(_)
        | HirExprKind::BoolLiteral(_)
        | HirExprKind::CharLiteral(_)
        | HirExprKind::UnitLiteral
        | HirExprKind::RegexLiteral { .. }
        | HirExprKind::VarRef(_)
        | HirExprKind::Continue
        | HirExprKind::NullLiteral
        | HirExprKind::Error => {}

        HirExprKind::FieldAccess { object, .. } => v.visit_expr_mut(object),
        HirExprKind::MethodCall {
            object,
            generic_args,
            args,
            block,
            ..
        } => {
            v.visit_expr_mut(object);
            for t in generic_args {
                v.visit_ty_mut(t);
            }
            for a in args {
                v.visit_expr_mut(a);
            }
            if let Some(b) = block {
                v.visit_expr_mut(b);
            }
        }
        HirExprKind::FnCall { args, .. } | HirExprKind::MacroCall { args, .. } => {
            for a in args {
                v.visit_expr_mut(a);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            v.visit_expr_mut(left);
            v.visit_expr_mut(right);
        }
        HirExprKind::UnaryOp { operand, .. } => v.visit_expr_mut(operand),
        HirExprKind::Borrow { expr: inner, .. } => v.visit_expr_mut(inner),
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            for s in stmts {
                v.visit_stmt_mut(s);
            }
            if let Some(t) = tail {
                v.visit_expr_mut(t);
            }
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            v.visit_expr_mut(cond);
            v.visit_expr_mut(then_branch);
            if let Some(e) = else_branch {
                v.visit_expr_mut(e);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            v.visit_expr_mut(scrutinee);
            for arm in arms {
                v.visit_pattern_mut(&mut arm.pattern);
                if let Some(g) = &mut arm.guard {
                    v.visit_expr_mut(g);
                }
                v.visit_expr_mut(&mut arm.body);
            }
        }
        HirExprKind::Loop { body } => v.visit_expr_mut(body),
        HirExprKind::While { condition, body } => {
            v.visit_expr_mut(condition);
            v.visit_expr_mut(body);
        }
        HirExprKind::For { iterable, body, .. } => {
            v.visit_expr_mut(iterable);
            v.visit_expr_mut(body);
        }
        HirExprKind::Assign { target, value, .. }
        | HirExprKind::CompoundAssign { target, value, .. } => {
            v.visit_expr_mut(target);
            v.visit_expr_mut(value);
        }
        HirExprKind::MultiAssign {
            targets, values, ..
        } => {
            for t in targets {
                v.visit_expr_mut(t);
            }
            for val in values {
                v.visit_expr_mut(val);
            }
        }
        HirExprKind::Return(value) | HirExprKind::Break(value) | HirExprKind::Next(value) => {
            if let Some(val) = value {
                v.visit_expr_mut(val);
            }
        }
        HirExprKind::Closure { params, body, .. } => {
            for p in params {
                v.visit_ty_mut(&mut p.ty);
            }
            v.visit_expr_mut(body);
        }
        HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
            for (_, f) in fields {
                v.visit_expr_mut(f);
            }
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            for e in elems {
                v.visit_expr_mut(e);
            }
        }
        HirExprKind::Index { object, index } => {
            v.visit_expr_mut(object);
            v.visit_expr_mut(index);
        }
        HirExprKind::Cast {
            expr: inner,
            target,
        } => {
            v.visit_expr_mut(inner);
            v.visit_ty_mut(target);
        }
        HirExprKind::MapLiteral(pairs) => {
            for (k, val) in pairs {
                v.visit_expr_mut(k);
                v.visit_expr_mut(val);
            }
        }
        HirExprKind::ArrayFill { value, .. } => v.visit_expr_mut(value),
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                v.visit_expr_mut(s);
            }
            if let Some(e) = end {
                v.visit_expr_mut(e);
            }
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                match p {
                    HirInterpolationPart::Literal(_) => {}
                    HirInterpolationPart::Expr { expr: e, .. } => v.visit_expr_mut(e),
                }
            }
        }
    }
}
