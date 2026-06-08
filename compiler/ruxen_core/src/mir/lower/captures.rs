use super::*;

pub(super) fn collect_captures(
    expr: &HirExpr,
    closure_params: &HashSet<DefId>,
    outer_defs: &HashSet<DefId>,
    out: &mut Vec<DefId>,
    seen: &mut HashSet<DefId>,
) {
    let mut locally_bound: HashSet<DefId> = HashSet::new();
    collect_captures_inner(
        expr,
        closure_params,
        outer_defs,
        &mut locally_bound,
        out,
        seen,
    );
}

pub(super) fn collect_captures_inner(
    expr: &HirExpr,
    closure_params: &HashSet<DefId>,
    outer_defs: &HashSet<DefId>,
    locally_bound: &mut HashSet<DefId>,
    out: &mut Vec<DefId>,
    seen: &mut HashSet<DefId>,
) {
    match &expr.kind {
        HirExprKind::VarRef(def_id) => {
            if !closure_params.contains(def_id)
                && !locally_bound.contains(def_id)
                && outer_defs.contains(def_id)
                && !seen.contains(def_id)
            {
                out.push(*def_id);
                seen.insert(*def_id);
            }
        }
        HirExprKind::FieldAccess { object, .. } => {
            collect_captures_inner(object, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            collect_captures_inner(object, closure_params, outer_defs, locally_bound, out, seen);
            for a in args {
                collect_captures_inner(a, closure_params, outer_defs, locally_bound, out, seen);
            }
            if let Some(b) = block {
                collect_captures_inner(b, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::FnCall { args, .. } => {
            for a in args {
                collect_captures_inner(a, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            collect_captures_inner(left, closure_params, outer_defs, locally_bound, out, seen);
            collect_captures_inner(right, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::UnaryOp { operand, .. } => {
            collect_captures_inner(
                operand,
                closure_params,
                outer_defs,
                locally_bound,
                out,
                seen,
            );
        }
        HirExprKind::Borrow { expr: inner, .. } => {
            collect_captures_inner(inner, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            let saved_bound = locally_bound.clone();
            for s in stmts {
                collect_captures_in_stmt(s, closure_params, outer_defs, locally_bound, out, seen);
            }
            if let Some(t) = tail {
                collect_captures_inner(t, closure_params, outer_defs, locally_bound, out, seen);
            }
            *locally_bound = saved_bound;
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_captures_inner(cond, closure_params, outer_defs, locally_bound, out, seen);
            collect_captures_inner(
                then_branch,
                closure_params,
                outer_defs,
                locally_bound,
                out,
                seen,
            );
            if let Some(e) = else_branch {
                collect_captures_inner(e, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_captures_inner(
                scrutinee,
                closure_params,
                outer_defs,
                locally_bound,
                out,
                seen,
            );
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_captures_inner(g, closure_params, outer_defs, locally_bound, out, seen);
                }
                collect_captures_inner(
                    &arm.body,
                    closure_params,
                    outer_defs,
                    locally_bound,
                    out,
                    seen,
                );
            }
        }
        HirExprKind::While { condition, body } => {
            collect_captures_inner(
                condition,
                closure_params,
                outer_defs,
                locally_bound,
                out,
                seen,
            );
            collect_captures_inner(body, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Loop { body } => {
            collect_captures_inner(body, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::For {
            iterable,
            body,
            binding,
            tuple_bindings,
            ..
        } => {
            collect_captures_inner(
                iterable,
                closure_params,
                outer_defs,
                locally_bound,
                out,
                seen,
            );
            let saved_bound = locally_bound.clone();
            locally_bound.insert(*binding);
            for (d, _) in tuple_bindings {
                locally_bound.insert(*d);
            }
            collect_captures_inner(body, closure_params, outer_defs, locally_bound, out, seen);
            *locally_bound = saved_bound;
        }
        HirExprKind::Assign { target, value, .. }
        | HirExprKind::CompoundAssign { target, value, .. } => {
            collect_captures_inner(target, closure_params, outer_defs, locally_bound, out, seen);
            collect_captures_inner(value, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Return(Some(inner)) | HirExprKind::Break(Some(inner)) => {
            collect_captures_inner(inner, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            for e in elems {
                collect_captures_inner(e, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::Index { object, index } => {
            collect_captures_inner(object, closure_params, outer_defs, locally_bound, out, seen);
            collect_captures_inner(index, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Construct { fields, .. } => {
            for (_n, v) in fields {
                collect_captures_inner(v, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::EnumVariant { fields, .. } => {
            for (_n, v) in fields {
                collect_captures_inner(v, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                if let crate::hir::nodes::HirInterpolationPart::Expr { expr: e, .. } = p {
                    collect_captures_inner(e, closure_params, outer_defs, locally_bound, out, seen);
                }
            }
        }
        HirExprKind::MacroCall { args, .. } => {
            for a in args {
                collect_captures_inner(a, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                collect_captures_inner(s, closure_params, outer_defs, locally_bound, out, seen);
            }
            if let Some(e) = end {
                collect_captures_inner(e, closure_params, outer_defs, locally_bound, out, seen);
            }
        }
        HirExprKind::ArrayFill { value, .. } => {
            collect_captures_inner(value, closure_params, outer_defs, locally_bound, out, seen);
        }
        HirExprKind::Closure {
            body: nested,
            params: nested_params,
            ..
        } => {
            // A nested closure sees our captured vars too.  Merge its
            // parameters into `closure_params` just for the nested walk.
            let mut merged = closure_params.clone();
            for p in nested_params {
                merged.insert(p.def_id);
            }
            let saved_bound = locally_bound.clone();
            collect_captures_inner(nested, &merged, outer_defs, locally_bound, out, seen);
            *locally_bound = saved_bound;
        }
        HirExprKind::Cast { expr: inner, .. } => {
            collect_captures_inner(inner, closure_params, outer_defs, locally_bound, out, seen);
        }
        // Leaf expressions — nothing to traverse.
        _ => {}
    }
}

pub(super) fn collect_captures_in_stmt(
    stmt: &HirStatement,
    closure_params: &HashSet<DefId>,
    outer_defs: &HashSet<DefId>,
    locally_bound: &mut HashSet<DefId>,
    out: &mut Vec<DefId>,
    seen: &mut HashSet<DefId>,
) {
    match stmt {
        HirStatement::Let { def_id, value, .. } => {
            if let Some(v) = value {
                collect_captures_inner(v, closure_params, outer_defs, locally_bound, out, seen);
            }
            locally_bound.insert(*def_id);
        }
        HirStatement::Expr(e) => {
            collect_captures_inner(e, closure_params, outer_defs, locally_bound, out, seen);
        }
    }
}

/// Return `true` if the closure body performs any assignment to the given
/// outer-frame `def_id` (used to decide between ByValue and ByRef storage).
pub(super) fn closure_body_mutates(body: &HirExpr, def_id: DefId) -> bool {
    match &body.kind {
        HirExprKind::Assign { target, value, .. }
        | HirExprKind::CompoundAssign { target, value, .. } => {
            if let HirExprKind::VarRef(d) = &target.kind {
                if *d == def_id {
                    return true;
                }
            }
            closure_body_mutates(target, def_id) || closure_body_mutates(value, def_id)
        }
        HirExprKind::FieldAccess { object, .. } => closure_body_mutates(object, def_id),
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            closure_body_mutates(object, def_id)
                || args.iter().any(|a| closure_body_mutates(a, def_id))
                || block
                    .as_ref()
                    .is_some_and(|b| closure_body_mutates(b, def_id))
        }
        HirExprKind::FnCall { args, .. } => args.iter().any(|a| closure_body_mutates(a, def_id)),
        HirExprKind::BinaryOp { left, right, .. } => {
            closure_body_mutates(left, def_id) || closure_body_mutates(right, def_id)
        }
        HirExprKind::UnaryOp { operand, .. } => closure_body_mutates(operand, def_id),
        HirExprKind::Borrow { expr, .. } => closure_body_mutates(expr, def_id),
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            for s in stmts {
                if stmt_mutates(s, def_id) {
                    return true;
                }
            }
            tail.as_ref()
                .is_some_and(|t| closure_body_mutates(t, def_id))
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            closure_body_mutates(cond, def_id)
                || closure_body_mutates(then_branch, def_id)
                || else_branch
                    .as_ref()
                    .is_some_and(|e| closure_body_mutates(e, def_id))
        }
        HirExprKind::Match { scrutinee, arms } => {
            if closure_body_mutates(scrutinee, def_id) {
                return true;
            }
            arms.iter().any(|arm| {
                arm.guard
                    .as_ref()
                    .is_some_and(|g| closure_body_mutates(g, def_id))
                    || closure_body_mutates(&arm.body, def_id)
            })
        }
        HirExprKind::While { condition, body } => {
            closure_body_mutates(condition, def_id) || closure_body_mutates(body, def_id)
        }
        HirExprKind::Loop { body } => closure_body_mutates(body, def_id),
        HirExprKind::For { iterable, body, .. } => {
            closure_body_mutates(iterable, def_id) || closure_body_mutates(body, def_id)
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            elems.iter().any(|e| closure_body_mutates(e, def_id))
        }
        HirExprKind::Index { object, index } => {
            closure_body_mutates(object, def_id) || closure_body_mutates(index, def_id)
        }
        HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
            fields.iter().any(|(_, v)| closure_body_mutates(v, def_id))
        }
        HirExprKind::Interpolation { parts } => parts.iter().any(|p| match p {
            crate::hir::nodes::HirInterpolationPart::Expr { expr: e, .. } => {
                closure_body_mutates(e, def_id)
            }
            _ => false,
        }),
        HirExprKind::MacroCall { args, .. } => args.iter().any(|a| closure_body_mutates(a, def_id)),
        HirExprKind::Range { start, end, .. } => {
            start
                .as_ref()
                .is_some_and(|s| closure_body_mutates(s, def_id))
                || end
                    .as_ref()
                    .is_some_and(|e| closure_body_mutates(e, def_id))
        }
        HirExprKind::ArrayFill { value, .. } => closure_body_mutates(value, def_id),
        HirExprKind::Return(Some(inner)) | HirExprKind::Break(Some(inner)) => {
            closure_body_mutates(inner, def_id)
        }
        HirExprKind::Closure { body: nested, .. } => closure_body_mutates(nested, def_id),
        HirExprKind::Cast { expr, .. } => closure_body_mutates(expr, def_id),
        _ => false,
    }
}

pub(super) fn stmt_mutates(stmt: &HirStatement, def_id: DefId) -> bool {
    match stmt {
        HirStatement::Let { value: Some(v), .. } => closure_body_mutates(v, def_id),
        HirStatement::Let { .. } => false,
        HirStatement::Expr(e) => closure_body_mutates(e, def_id),
    }
}
