use super::*;

/// Rewrite every occurrence of `Ty::TypeParam { name == "Self" }` inside a
/// cloned trait default method's body/params/return type to point at the
/// concrete `impl` target. This is how we monomorphise a default method for
/// each implementor so that `self.field` / `self.other_method` dispatch
/// resolves through the normal `{ConcreteType}_{method}` path.
pub(super) fn rewrite_self_in_func(func: &mut HirFuncDef, concrete: &Ty) {
    rewrite_self_in_ty(&mut func.return_ty, concrete);
    for p in &mut func.params {
        rewrite_self_in_ty(&mut p.ty, concrete);
    }
    rewrite_self_in_expr(&mut func.body, concrete);
}

fn rewrite_self_in_ty(ty: &mut Ty, concrete: &Ty) {
    match ty {
        Ty::TypeParam { name, .. } if name == "Self" => {
            *ty = concrete.clone();
        }
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => rewrite_self_in_ty(inner, concrete),
        Ty::Tuple(elems) => {
            for e in elems {
                rewrite_self_in_ty(e, concrete);
            }
        }
        Ty::FixedArray(inner, _) => rewrite_self_in_ty(inner, concrete),
        Ty::Option(inner) => rewrite_self_in_ty(inner, concrete),
        Ty::Result(ok, err) => {
            rewrite_self_in_ty(ok, concrete);
            rewrite_self_in_ty(err, concrete);
        }
        _ => {}
    }
}

fn rewrite_self_in_expr(expr: &mut HirExpr, concrete: &Ty) {
    rewrite_self_in_ty(&mut expr.ty, concrete);
    match &mut expr.kind {
        HirExprKind::FieldAccess { object, .. } => {
            rewrite_self_in_expr(object, concrete);
        }
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            rewrite_self_in_expr(object, concrete);
            for a in args {
                rewrite_self_in_expr(a, concrete);
            }
            if let Some(b) = block {
                rewrite_self_in_expr(b, concrete);
            }
        }
        HirExprKind::FnCall { args, .. } => {
            for a in args {
                rewrite_self_in_expr(a, concrete);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            rewrite_self_in_expr(left, concrete);
            rewrite_self_in_expr(right, concrete);
        }
        HirExprKind::UnaryOp { operand, .. } => {
            rewrite_self_in_expr(operand, concrete);
        }
        HirExprKind::Borrow { expr: inner, .. } => {
            rewrite_self_in_expr(inner, concrete);
        }
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            for s in stmts {
                rewrite_self_in_stmt(s, concrete);
            }
            if let Some(t) = tail {
                rewrite_self_in_expr(t, concrete);
            }
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            rewrite_self_in_expr(cond, concrete);
            rewrite_self_in_expr(then_branch, concrete);
            if let Some(e) = else_branch {
                rewrite_self_in_expr(e, concrete);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            rewrite_self_in_expr(scrutinee, concrete);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    rewrite_self_in_expr(g, concrete);
                }
                rewrite_self_in_expr(&mut arm.body, concrete);
            }
        }
        HirExprKind::Loop { body } => rewrite_self_in_expr(body, concrete),
        HirExprKind::While { condition, body } => {
            rewrite_self_in_expr(condition, concrete);
            rewrite_self_in_expr(body, concrete);
        }
        HirExprKind::For { iterable, body, .. } => {
            rewrite_self_in_expr(iterable, concrete);
            rewrite_self_in_expr(body, concrete);
        }
        HirExprKind::Assign { target, value, .. } => {
            rewrite_self_in_expr(target, concrete);
            rewrite_self_in_expr(value, concrete);
        }
        HirExprKind::CompoundAssign { target, value, .. } => {
            rewrite_self_in_expr(target, concrete);
            rewrite_self_in_expr(value, concrete);
        }
        HirExprKind::Return(e) | HirExprKind::Break(e) => {
            if let Some(inner) = e {
                rewrite_self_in_expr(inner, concrete);
            }
        }
        HirExprKind::Closure { body, .. } => {
            rewrite_self_in_expr(body, concrete);
        }
        HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
            for (_, e) in fields {
                rewrite_self_in_expr(e, concrete);
            }
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            for e in elems {
                rewrite_self_in_expr(e, concrete);
            }
        }
        HirExprKind::Index { object, index } => {
            rewrite_self_in_expr(object, concrete);
            rewrite_self_in_expr(index, concrete);
        }
        HirExprKind::Cast {
            expr: inner,
            target,
        } => {
            rewrite_self_in_expr(inner, concrete);
            rewrite_self_in_ty(target, concrete);
        }
        HirExprKind::ArrayFill { value, .. } => {
            rewrite_self_in_expr(value, concrete);
        }
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                rewrite_self_in_expr(s, concrete);
            }
            if let Some(e) = end {
                rewrite_self_in_expr(e, concrete);
            }
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                if let HirInterpolationPart::Expr { expr: e, .. } = p {
                    rewrite_self_in_expr(e, concrete);
                }
            }
        }
        HirExprKind::MacroCall { args, .. } => {
            for a in args {
                rewrite_self_in_expr(a, concrete);
            }
        }
        _ => {}
    }
}

fn rewrite_self_in_stmt(stmt: &mut HirStatement, concrete: &Ty) {
    match stmt {
        HirStatement::Let { ty, value, .. } => {
            rewrite_self_in_ty(ty, concrete);
            if let Some(v) = value {
                rewrite_self_in_expr(v, concrete);
            }
        }
        HirStatement::Expr(e) => rewrite_self_in_expr(e, concrete),
    }
}
