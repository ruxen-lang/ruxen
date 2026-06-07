//! Free helper functions extracted from `resolve/mod.rs` to keep the
//! file under 5K lines.
//!
//! `yield VALUE` inside a function body implicitly introduces a
//! trailing `__block: Closure` parameter.  Before the main resolution
//! pass, walk the AST and record every function whose body contains a
//! `yield`, along with the arity of the first `yield` found.  The
//! arity is used to pre-shape the synthetic block's `Ty::Fn` parameter
//! list so that caller-side unification on the trailing closure
//! produces a concrete type.

use crate::parser::ast;
use std::collections::HashMap;

// ─── Yield Pre-Scan ────────────────────────────────────────────────────
//
// `yield VALUE` inside a function body implicitly introduces a trailing
// `__block: Closure` parameter.  Before the main resolution pass, walk
// the AST and record every function whose body contains a `yield`, along
// with the arity of the first `yield` found.  The arity is used to
// pre-shape the synthetic block's `Ty::Fn` parameter list so that
// caller-side unification on the trailing closure produces a concrete type.

pub(super) fn collect_yield_fns(item: &ast::TopLevelItem, out: &mut HashMap<String, usize>) {
    match item {
        ast::TopLevelItem::Function(f) => {
            if let Some(arity) = find_first_yield_arity_in_block(&f.body) {
                out.insert(f.name.clone(), arity);
            }
        }
        ast::TopLevelItem::Module(m) => {
            for sub in &m.items {
                collect_yield_fns(sub, out);
            }
        }
        ast::TopLevelItem::Class(c) => {
            for m in &c.methods {
                if let Some(arity) = find_first_yield_arity_in_block(&m.body) {
                    out.insert(m.name.clone(), arity);
                }
            }
        }
        ast::TopLevelItem::Impl(b) => {
            for it in &b.items {
                if let ast::ImplItem::Method(m) = it {
                    if let Some(arity) = find_first_yield_arity_in_block(&m.body) {
                        out.insert(m.name.clone(), arity);
                    }
                }
            }
        }
        _ => {}
    }
}

pub(super) fn find_first_yield_arity_in_block(block: &ast::Block) -> Option<usize> {
    find_first_yield_args_in_block(block).map(<[ast::Expr]>::len)
}

/// For the FIRST `yield` in `block`, return a mask marking which argument
/// positions are a bare `self` (`ExprKind::SelfRef`). Used to type the
/// corresponding synthetic `__block` parameter as the enclosing class rather
/// than a fresh type variable, so a method's `yield self` propagates a
/// concrete block-parameter type to the call site (free functions already
/// resolve it; methods went through a cloned signature that lost the link).
pub(super) fn first_yield_self_mask_in_block(block: &ast::Block) -> Option<Vec<bool>> {
    find_first_yield_args_in_block(block).map(|args| {
        args.iter()
            .map(|a| matches!(a.kind, ast::ExprKind::SelfRef))
            .collect()
    })
}

fn find_first_yield_args_in_block(block: &ast::Block) -> Option<&[ast::Expr]> {
    for stmt in &block.statements {
        if let Some(a) = find_first_yield_args_in_stmt(stmt) {
            return Some(a);
        }
    }
    None
}

fn find_first_yield_args_in_stmt(stmt: &ast::Statement) -> Option<&[ast::Expr]> {
    match stmt {
        ast::Statement::Let(b) => b.value.as_deref().and_then(find_first_yield_args_in_expr),
        ast::Statement::Expression(e) => find_first_yield_args_in_expr(e),
    }
}

fn find_first_yield_args_in_expr(expr: &ast::Expr) -> Option<&[ast::Expr]> {
    use ast::ExprKind::*;
    match &expr.kind {
        Yield(args) => Some(args.as_slice()),
        BinaryOp { left, right, .. } => {
            find_first_yield_args_in_expr(left).or_else(|| find_first_yield_args_in_expr(right))
        }
        UnaryOp { operand, .. } => find_first_yield_args_in_expr(operand),
        Borrow(e) | BorrowMut(e) => find_first_yield_args_in_expr(e),
        FieldAccess { object, .. } | SafeNav { object, .. } => {
            find_first_yield_args_in_expr(object)
        }
        MethodCall {
            object,
            args,
            block,
            ..
        } => find_first_yield_args_in_expr(object)
            .or_else(|| args.iter().find_map(find_first_yield_args_in_expr))
            .or_else(|| block.as_deref().and_then(find_first_yield_args_in_expr)),
        SafeNavCall { object, args, .. } => find_first_yield_args_in_expr(object)
            .or_else(|| args.iter().find_map(find_first_yield_args_in_expr)),
        Call {
            callee,
            args,
            block,
        } => find_first_yield_args_in_expr(callee)
            .or_else(|| args.iter().find_map(find_first_yield_args_in_expr))
            .or_else(|| block.as_deref().and_then(find_first_yield_args_in_expr)),
        Index { object, index } => {
            find_first_yield_args_in_expr(object).or_else(|| find_first_yield_args_in_expr(index))
        }
        ClosureCall { callee, args } => find_first_yield_args_in_expr(callee)
            .or_else(|| args.iter().find_map(find_first_yield_args_in_expr)),
        Try(e) => find_first_yield_args_in_expr(e),
        Assign { target, value } => {
            find_first_yield_args_in_expr(target).or_else(|| find_first_yield_args_in_expr(value))
        }
        CompoundAssign { target, value, .. } => {
            find_first_yield_args_in_expr(target).or_else(|| find_first_yield_args_in_expr(value))
        }
        If(ife) => find_first_yield_args_in_expr(&ife.condition)
            .or_else(|| find_first_yield_args_in_block(&ife.then_body))
            .or_else(|| {
                ife.elsif_clauses.iter().find_map(|c| {
                    find_first_yield_args_in_expr(&c.condition)
                        .or_else(|| find_first_yield_args_in_block(&c.body))
                })
            })
            .or_else(|| {
                ife.else_body
                    .as_ref()
                    .and_then(find_first_yield_args_in_block)
            }),
        IfLet(ile) => find_first_yield_args_in_expr(&ile.value)
            .or_else(|| find_first_yield_args_in_block(&ile.then_body))
            .or_else(|| {
                ile.else_body
                    .as_ref()
                    .and_then(find_first_yield_args_in_block)
            }),
        Match(me) => find_first_yield_args_in_expr(&me.subject).or_else(|| {
            me.arms.iter().find_map(|a| match &a.body {
                ast::MatchArmBody::Expr(e) => find_first_yield_args_in_expr(e),
                ast::MatchArmBody::Block(b) => find_first_yield_args_in_block(b),
            })
        }),
        While(we) => find_first_yield_args_in_expr(&we.condition)
            .or_else(|| find_first_yield_args_in_block(&we.body)),
        WhileLet(wle) => find_first_yield_args_in_expr(&wle.value)
            .or_else(|| find_first_yield_args_in_block(&wle.body)),
        For(fe) => find_first_yield_args_in_expr(&fe.iterable)
            .or_else(|| find_first_yield_args_in_block(&fe.body)),
        Loop(le) => find_first_yield_args_in_block(&le.body),
        Block(b) | UnsafeBlock(b) => find_first_yield_args_in_block(b),
        // A `yield` inside a nested closure does not belong to the
        // enclosing function for our v1 implicit-block scheme; skipping
        // avoids double-counting cases like `.filter { |x| yield x }`
        // where the surrounding method already declares an explicit
        // block parameter.
        Closure(_) => None,
        Range { start, end, .. } => start
            .as_deref()
            .and_then(find_first_yield_args_in_expr)
            .or_else(|| end.as_deref().and_then(find_first_yield_args_in_expr)),
        ArrayLiteral(elems) => elems.iter().find_map(find_first_yield_args_in_expr),
        ArrayFill { value, count } => {
            find_first_yield_args_in_expr(value).or_else(|| find_first_yield_args_in_expr(count))
        }
        TupleLiteral(elems) => elems.iter().find_map(find_first_yield_args_in_expr),
        Return(e) | Break(e) => e.as_deref().and_then(find_first_yield_args_in_expr),
        Continue => None,
        MacroCall { args, .. } => args.iter().find_map(find_first_yield_args_in_expr),
        Cast { expr, .. } => find_first_yield_args_in_expr(expr),
        EnumVariant { args, .. } => args
            .iter()
            .find_map(|fa| find_first_yield_args_in_expr(&fa.value)),
        InterpolatedString(_) => None,
        _ => None,
    }
}
