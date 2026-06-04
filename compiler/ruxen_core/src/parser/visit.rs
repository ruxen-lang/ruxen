//! Exhaustive, shared traversal of the parser AST.
//!
//! `Visit` (immutable) and `VisitMut` (mutating) each provide a default method
//! per node type; the free `walk_*` super-functions contain the ONE exhaustive
//! match over `ExprKind`/`Statement`/`Pattern`/`TypeExpr`. Consumers override
//! only the nodes they care about and call the matching `walk_*` to recurse
//! (or DON'T call it to treat a subtree as opaque, e.g. closure bodies in the
//! async await-scan). There is no `_ =>` arm anywhere: adding an `ExprKind`
//! variant is a compile error until every `walk_*` handles it. That
//! exhaustiveness is the entire point — it ends the variant-drift bug class.

use crate::parser::ast::*;

#[allow(unused_variables)]
pub trait Visit: Sized {
    fn visit_expr(&mut self, e: &Expr) {
        walk_expr(self, e);
    }
    fn visit_block(&mut self, b: &Block) {
        walk_block(self, b);
    }
    fn visit_stmt(&mut self, s: &Statement) {
        walk_stmt(self, s);
    }
    fn visit_pattern(&mut self, p: &Pattern) {
        walk_pattern(self, p);
    }
    fn visit_type_expr(&mut self, t: &TypeExpr) {
        walk_type_expr(self, t);
    }
}

pub fn walk_block<V: Visit>(v: &mut V, b: &Block) {
    for s in &b.statements {
        v.visit_stmt(s);
    }
}

pub fn walk_stmt<V: Visit>(v: &mut V, s: &Statement) {
    match s {
        Statement::Let(lb) => {
            v.visit_pattern(&lb.pattern);
            if let Some(t) = &lb.type_annotation {
                v.visit_type_expr(t);
            }
            if let Some(val) = &lb.value {
                v.visit_expr(val);
            }
        }
        Statement::Expression(e) => v.visit_expr(e),
    }
}

pub fn walk_expr<V: Visit>(v: &mut V, e: &Expr) {
    match &e.kind {
        // Leaves — no nested AST node to visit. `InterpolatedString` holds
        // `StringPart`s whose `Expr` variant carries raw lexer tokens, not a
        // parsed `Expr`, so there is nothing to recurse into here.
        ExprKind::IntLiteral(..)
        | ExprKind::FloatLiteral(..)
        | ExprKind::StringLiteral(_)
        | ExprKind::InterpolatedString(_)
        | ExprKind::CharLiteral(_)
        | ExprKind::BoolLiteral(_)
        | ExprKind::UnitLiteral
        | ExprKind::Identifier(_)
        | ExprKind::SelfRef
        | ExprKind::SelfType
        | ExprKind::Continue
        | ExprKind::NullLiteral
        | ExprKind::RegexLiteral { .. } => {}

        ExprKind::BinaryOp { left, right, .. } => {
            v.visit_expr(left);
            v.visit_expr(right);
        }
        ExprKind::UnaryOp { operand, .. } => v.visit_expr(operand),
        ExprKind::Borrow(x) | ExprKind::BorrowMut(x) => v.visit_expr(x),
        ExprKind::FieldAccess { object, .. } => v.visit_expr(object),
        ExprKind::MethodCall {
            object,
            args,
            block,
            generic_args,
            ..
        } => {
            v.visit_expr(object);
            for t in generic_args {
                v.visit_type_expr(t);
            }
            for a in args {
                v.visit_expr(a);
            }
            if let Some(b) = block {
                v.visit_expr(b);
            }
        }
        ExprKind::SafeNav { object, .. } => v.visit_expr(object),
        ExprKind::SafeNavCall { object, args, .. } => {
            v.visit_expr(object);
            for a in args {
                v.visit_expr(a);
            }
        }
        ExprKind::Call { callee, args, block } => {
            v.visit_expr(callee);
            for a in args {
                v.visit_expr(a);
            }
            if let Some(b) = block {
                v.visit_expr(b);
            }
        }
        ExprKind::Index { object, index } => {
            v.visit_expr(object);
            v.visit_expr(index);
        }
        ExprKind::ClosureCall { callee, args } => {
            v.visit_expr(callee);
            for a in args {
                v.visit_expr(a);
            }
        }
        ExprKind::Try(x) => v.visit_expr(x),
        ExprKind::Await(x) => v.visit_expr(x),
        ExprKind::Assign { target, value } => {
            v.visit_expr(target);
            v.visit_expr(value);
        }
        ExprKind::CompoundAssign { target, value, .. } => {
            v.visit_expr(target);
            v.visit_expr(value);
        }
        ExprKind::If(IfExpr {
            condition,
            then_body,
            elsif_clauses,
            else_body,
            ..
        }) => {
            v.visit_expr(condition);
            v.visit_block(then_body);
            for el in elsif_clauses {
                v.visit_expr(&el.condition);
                v.visit_block(&el.body);
            }
            if let Some(b) = else_body {
                v.visit_block(b);
            }
        }
        ExprKind::IfLet(IfLetExpr {
            pattern,
            value,
            then_body,
            else_body,
            ..
        }) => {
            v.visit_pattern(pattern);
            v.visit_expr(value);
            v.visit_block(then_body);
            if let Some(b) = else_body {
                v.visit_block(b);
            }
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            v.visit_expr(subject);
            for a in arms {
                v.visit_pattern(&a.pattern);
                if let Some(g) = &a.guard {
                    v.visit_expr(g);
                }
                match &a.body {
                    MatchArmBody::Expr(ex) => v.visit_expr(ex),
                    MatchArmBody::Block(b) => v.visit_block(b),
                }
            }
        }
        ExprKind::While(WhileExpr {
            condition, body, ..
        }) => {
            v.visit_expr(condition);
            v.visit_block(body);
        }
        ExprKind::WhileLet(WhileLetExpr {
            pattern,
            value,
            body,
            ..
        }) => {
            v.visit_pattern(pattern);
            v.visit_expr(value);
            v.visit_block(body);
        }
        ExprKind::For(ForExpr {
            pattern,
            iterable,
            body,
            ..
        }) => {
            v.visit_pattern(pattern);
            v.visit_expr(iterable);
            v.visit_block(body);
        }
        ExprKind::Loop(LoopExpr { body, .. }) => v.visit_block(body),
        ExprKind::Block(b) => v.visit_block(b),
        ExprKind::UnsafeBlock(b) => v.visit_block(b),
        ExprKind::Closure(c) => match &c.body {
            ClosureBody::Expr(ex) => v.visit_expr(ex),
            ClosureBody::Block(b) => v.visit_block(b),
        },
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                v.visit_expr(s);
            }
            if let Some(e2) = end {
                v.visit_expr(e2);
            }
        }
        ExprKind::ArrayLiteral(items) | ExprKind::TupleLiteral(items) => {
            for it in items {
                v.visit_expr(it);
            }
        }
        ExprKind::ArrayFill { value, count } => {
            v.visit_expr(value);
            v.visit_expr(count);
        }
        ExprKind::MapLiteral(pairs) => {
            for (k, val) in pairs {
                v.visit_expr(k);
                v.visit_expr(val);
            }
        }
        ExprKind::Return(opt) | ExprKind::Break(opt) => {
            if let Some(x) = opt {
                v.visit_expr(x);
            }
        }
        ExprKind::Yield(items) => {
            for it in items {
                v.visit_expr(it);
            }
        }
        ExprKind::MacroCall { args, .. } => {
            for a in args {
                v.visit_expr(a);
            }
        }
        ExprKind::Cast { expr, target_type } => {
            v.visit_expr(expr);
            v.visit_type_expr(target_type);
        }
        ExprKind::EnumVariant { args, .. } => {
            for a in args {
                v.visit_expr(&a.value);
            }
        }
    }
}

pub fn walk_pattern<V: Visit>(v: &mut V, p: &Pattern) {
    // Exhaustive over `parser::ast::Pattern` (no `_` arm). A pattern may embed
    // a literal `Expr` (`Pattern::Literal`) and may nest sub-patterns.
    match p {
        Pattern::Literal { expr, .. } => v.visit_expr(expr),
        Pattern::Identifier { .. } => {}
        Pattern::Wildcard { .. } => {}
        Pattern::Tuple { elements, .. } => {
            for sub in elements {
                v.visit_pattern(sub);
            }
        }
        Pattern::Enum { fields, .. } => {
            for sub in fields {
                v.visit_pattern(sub);
            }
        }
        Pattern::Struct { fields, .. } => {
            for f in fields {
                v.visit_pattern(&f.pattern);
            }
        }
        Pattern::Or { patterns, .. } => {
            for sub in patterns {
                v.visit_pattern(sub);
            }
        }
        Pattern::Ref { .. } => {}
        Pattern::Rest { .. } => {}
    }
}

pub fn walk_type_expr<V: Visit>(v: &mut V, t: &TypeExpr) {
    // Exhaustive over `parser::ast::TypeExpr` (no `_` arm). Type expressions can
    // nest type expressions and, in array-size / const-expr-arg positions, carry
    // a parser `Expr`.
    match t {
        TypeExpr::Named(path) => {
            if let Some(args) = &path.generic_args {
                for a in args {
                    v.visit_type_expr(a);
                }
            }
        }
        TypeExpr::Reference { inner, .. } => v.visit_type_expr(inner),
        TypeExpr::Tuple { elements, .. } => {
            for el in elements {
                v.visit_type_expr(el);
            }
        }
        TypeExpr::Array { element, size, .. } => {
            v.visit_type_expr(element);
            if let Some(sz) = size {
                v.visit_expr(sz);
            }
        }
        TypeExpr::Function {
            params,
            return_type,
            ..
        } => {
            for p in params {
                v.visit_type_expr(p);
            }
            v.visit_type_expr(return_type);
        }
        TypeExpr::SomeMixin { .. } => {}
        TypeExpr::AnyMixin { .. } => {}
        TypeExpr::Never { .. } => {}
        TypeExpr::Inferred { .. } => {}
        TypeExpr::RawPointer { inner, .. } => v.visit_type_expr(inner),
        TypeExpr::ConstLit { .. } => {}
        TypeExpr::ConstExprArg { expr, .. } => v.visit_expr(expr),
    }
}

#[allow(unused_variables)]
pub trait VisitMut: Sized {
    fn visit_expr_mut(&mut self, e: &mut Expr) {
        walk_expr_mut(self, e);
    }
    fn visit_block_mut(&mut self, b: &mut Block) {
        walk_block_mut(self, b);
    }
    fn visit_stmt_mut(&mut self, s: &mut Statement) {
        walk_stmt_mut(self, s);
    }
}

pub fn walk_block_mut<V: VisitMut>(v: &mut V, b: &mut Block) {
    for s in &mut b.statements {
        v.visit_stmt_mut(s);
    }
}

pub fn walk_stmt_mut<V: VisitMut>(v: &mut V, s: &mut Statement) {
    match s {
        Statement::Let(lb) => {
            if let Some(val) = &mut lb.value {
                v.visit_expr_mut(val);
            }
        }
        Statement::Expression(e) => v.visit_expr_mut(e),
    }
}

/// Exhaustive `&mut` mirror of [`walk_expr`] (no `_` arm). The mutable
/// consumers (block_on rewrite) only mutate `Expr` nodes, so this descends
/// into every nested `Expr`/`Block` but does not visit patterns or type
/// expressions (`VisitMut` has no method for those).
pub fn walk_expr_mut<V: VisitMut>(v: &mut V, e: &mut Expr) {
    match &mut e.kind {
        ExprKind::IntLiteral(..)
        | ExprKind::FloatLiteral(..)
        | ExprKind::StringLiteral(_)
        | ExprKind::InterpolatedString(_)
        | ExprKind::CharLiteral(_)
        | ExprKind::BoolLiteral(_)
        | ExprKind::UnitLiteral
        | ExprKind::Identifier(_)
        | ExprKind::SelfRef
        | ExprKind::SelfType
        | ExprKind::Continue
        | ExprKind::NullLiteral
        | ExprKind::RegexLiteral { .. } => {}

        ExprKind::BinaryOp { left, right, .. } => {
            v.visit_expr_mut(left);
            v.visit_expr_mut(right);
        }
        ExprKind::UnaryOp { operand, .. } => v.visit_expr_mut(operand),
        ExprKind::Borrow(x) | ExprKind::BorrowMut(x) => v.visit_expr_mut(x),
        ExprKind::FieldAccess { object, .. } => v.visit_expr_mut(object),
        ExprKind::MethodCall {
            object, args, block, ..
        } => {
            // `generic_args` (a `Vec<TypeExpr>`, elided by `..`) is intentionally
            // NOT descended: `VisitMut` has no `visit_type_expr_mut` and no
            // consumer mutates type-expression positions.
            v.visit_expr_mut(object);
            for a in args {
                v.visit_expr_mut(a);
            }
            if let Some(b) = block {
                v.visit_expr_mut(b);
            }
        }
        ExprKind::SafeNav { object, .. } => v.visit_expr_mut(object),
        ExprKind::SafeNavCall { object, args, .. } => {
            v.visit_expr_mut(object);
            for a in args {
                v.visit_expr_mut(a);
            }
        }
        ExprKind::Call { callee, args, block } => {
            v.visit_expr_mut(callee);
            for a in args {
                v.visit_expr_mut(a);
            }
            if let Some(b) = block {
                v.visit_expr_mut(b);
            }
        }
        ExprKind::Index { object, index } => {
            v.visit_expr_mut(object);
            v.visit_expr_mut(index);
        }
        ExprKind::ClosureCall { callee, args } => {
            v.visit_expr_mut(callee);
            for a in args {
                v.visit_expr_mut(a);
            }
        }
        ExprKind::Try(x) => v.visit_expr_mut(x),
        ExprKind::Await(x) => v.visit_expr_mut(x),
        ExprKind::Assign { target, value } => {
            v.visit_expr_mut(target);
            v.visit_expr_mut(value);
        }
        ExprKind::CompoundAssign { target, value, .. } => {
            v.visit_expr_mut(target);
            v.visit_expr_mut(value);
        }
        ExprKind::If(IfExpr {
            condition,
            then_body,
            elsif_clauses,
            else_body,
            ..
        }) => {
            v.visit_expr_mut(condition);
            v.visit_block_mut(then_body);
            for el in elsif_clauses {
                v.visit_expr_mut(&mut el.condition);
                v.visit_block_mut(&mut el.body);
            }
            if let Some(b) = else_body {
                v.visit_block_mut(b);
            }
        }
        ExprKind::IfLet(IfLetExpr {
            value,
            then_body,
            else_body,
            ..
        }) => {
            v.visit_expr_mut(value);
            v.visit_block_mut(then_body);
            if let Some(b) = else_body {
                v.visit_block_mut(b);
            }
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            v.visit_expr_mut(subject);
            for a in arms {
                if let Some(g) = &mut a.guard {
                    v.visit_expr_mut(g);
                }
                match &mut a.body {
                    MatchArmBody::Expr(ex) => v.visit_expr_mut(ex),
                    MatchArmBody::Block(b) => v.visit_block_mut(b),
                }
            }
        }
        ExprKind::While(WhileExpr {
            condition, body, ..
        }) => {
            v.visit_expr_mut(condition);
            v.visit_block_mut(body);
        }
        ExprKind::WhileLet(WhileLetExpr { value, body, .. }) => {
            v.visit_expr_mut(value);
            v.visit_block_mut(body);
        }
        ExprKind::For(ForExpr { iterable, body, .. }) => {
            v.visit_expr_mut(iterable);
            v.visit_block_mut(body);
        }
        ExprKind::Loop(LoopExpr { body, .. }) => v.visit_block_mut(body),
        ExprKind::Block(b) => v.visit_block_mut(b),
        ExprKind::UnsafeBlock(b) => v.visit_block_mut(b),
        ExprKind::Closure(c) => match &mut c.body {
            ClosureBody::Expr(ex) => v.visit_expr_mut(ex),
            ClosureBody::Block(b) => v.visit_block_mut(b),
        },
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                v.visit_expr_mut(s);
            }
            if let Some(e2) = end {
                v.visit_expr_mut(e2);
            }
        }
        ExprKind::ArrayLiteral(items) | ExprKind::TupleLiteral(items) => {
            for it in items {
                v.visit_expr_mut(it);
            }
        }
        ExprKind::ArrayFill { value, count } => {
            v.visit_expr_mut(value);
            v.visit_expr_mut(count);
        }
        ExprKind::MapLiteral(pairs) => {
            for (k, val) in pairs {
                v.visit_expr_mut(k);
                v.visit_expr_mut(val);
            }
        }
        ExprKind::Return(opt) | ExprKind::Break(opt) => {
            if let Some(x) = opt {
                v.visit_expr_mut(x);
            }
        }
        ExprKind::Yield(items) => {
            for it in items {
                v.visit_expr_mut(it);
            }
        }
        ExprKind::MacroCall { args, .. } => {
            for a in args {
                v.visit_expr_mut(a);
            }
        }
        ExprKind::Cast { expr, .. } => v.visit_expr_mut(expr),
        ExprKind::EnumVariant { args, .. } => {
            for a in args {
                v.visit_expr_mut(&mut a.value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::token::Span;

    fn sp() -> Span {
        Span::new(0, 0, 0, 0)
    }

    fn ident(n: &str) -> Expr {
        Expr {
            kind: ExprKind::Identifier(n.into()),
            span: sp(),
        }
    }
    fn await_of(inner: Expr) -> Expr {
        Expr {
            kind: ExprKind::Await(Box::new(inner)),
            span: sp(),
        }
    }

    // A visitor that counts Await nodes, recursing through everything via walk_expr.
    struct AwaitCounter {
        n: usize,
    }
    impl Visit for AwaitCounter {
        fn visit_expr(&mut self, e: &Expr) {
            if matches!(e.kind, ExprKind::Await(_)) {
                self.n += 1;
            }
            walk_expr(self, e);
        }
    }

    fn count_awaits(e: &Expr) -> usize {
        let mut c = AwaitCounter { n: 0 };
        c.visit_expr(e);
        c.n
    }

    #[test]
    fn finds_await_inside_enum_variant_args() {
        // Some(x.await) — EnumVariant arg; the OLD hand-rolled scan missed this (bug #1).
        let e = Expr {
            kind: ExprKind::EnumVariant {
                type_path: vec!["Option".into()],
                variant: "Some".into(),
                args: vec![FieldArg {
                    name: None,
                    value: await_of(ident("x")),
                    span: sp(),
                }],
            },
            span: sp(),
        };
        assert_eq!(count_awaits(&e), 1);
    }

    #[test]
    fn finds_await_inside_unsafe_block() {
        // unsafe { x.await }
        let blk = Block {
            statements: vec![Statement::Expression(await_of(ident("x")))],
            span: sp(),
        };
        let e = Expr {
            kind: ExprKind::UnsafeBlock(blk),
            span: sp(),
        };
        assert_eq!(count_awaits(&e), 1);
    }
}
