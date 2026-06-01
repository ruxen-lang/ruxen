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

/// Generic-class monomorphization (option 1): substitute every
/// `Ty::TypeParam { name }` whose `name` is a key in `subst` with the
/// concrete type bound to it. This is the generalization of
/// `rewrite_self_in_func` from the single `Self` rewrite to an arbitrary
/// `{ param_name → concrete Ty }` map, so a generic class method body
/// (`Box[T].eq`) can be specialized to `T = String` before MIR binop /
/// interpolation lowering runs — at which point the existing
/// `Ty::String` special-cases in `binops.rs` / `interpolation.rs` fire
/// naturally. The walk mirrors `rewrite_self_in_*` exactly so the two
/// stay structurally in sync.
pub(super) fn subst_type_params_in_func(
    func: &mut HirFuncDef,
    subst: &std::collections::HashMap<String, Ty>,
) {
    subst_type_params_in_ty(&mut func.return_ty, subst);
    for p in &mut func.params {
        subst_type_params_in_ty(&mut p.ty, subst);
    }
    subst_type_params_in_expr(&mut func.body, subst);
}

fn subst_type_params_in_ty(ty: &mut Ty, subst: &std::collections::HashMap<String, Ty>) {
    match ty {
        Ty::TypeParam { name, .. } => {
            if let Some(concrete) = subst.get(name) {
                *ty = concrete.clone();
            }
        }
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => subst_type_params_in_ty(inner, subst),
        Ty::Tuple(elems) => {
            for e in elems {
                subst_type_params_in_ty(e, subst);
            }
        }
        Ty::FixedArray(inner, _) => subst_type_params_in_ty(inner, subst),
        Ty::Option(inner) => subst_type_params_in_ty(inner, subst),
        Ty::Array(inner) | Ty::Set(inner) => subst_type_params_in_ty(inner, subst),
        Ty::Map(k, v) => {
            subst_type_params_in_ty(k, subst);
            subst_type_params_in_ty(v, subst);
        }
        Ty::Result(ok, err) => {
            subst_type_params_in_ty(ok, subst);
            subst_type_params_in_ty(err, subst);
        }
        Ty::Class { generic_args, .. }
        | Ty::Struct { generic_args, .. }
        | Ty::Enum { generic_args, .. } => {
            for a in generic_args {
                subst_type_params_in_ty(a, subst);
            }
        }
        _ => {}
    }
}

fn subst_type_params_in_expr(expr: &mut HirExpr, subst: &std::collections::HashMap<String, Ty>) {
    subst_type_params_in_ty(&mut expr.ty, subst);
    match &mut expr.kind {
        HirExprKind::FieldAccess { object, .. } => {
            subst_type_params_in_expr(object, subst);
        }
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            subst_type_params_in_expr(object, subst);
            for a in args {
                subst_type_params_in_expr(a, subst);
            }
            if let Some(b) = block {
                subst_type_params_in_expr(b, subst);
            }
        }
        HirExprKind::FnCall { args, .. } => {
            for a in args {
                subst_type_params_in_expr(a, subst);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            subst_type_params_in_expr(left, subst);
            subst_type_params_in_expr(right, subst);
        }
        HirExprKind::UnaryOp { operand, .. } => {
            subst_type_params_in_expr(operand, subst);
        }
        HirExprKind::Borrow { expr: inner, .. } => {
            subst_type_params_in_expr(inner, subst);
        }
        HirExprKind::Block(stmts, tail) | HirExprKind::UnsafeBlock(stmts, tail) => {
            for s in stmts {
                subst_type_params_in_stmt(s, subst);
            }
            if let Some(t) = tail {
                subst_type_params_in_expr(t, subst);
            }
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            subst_type_params_in_expr(cond, subst);
            subst_type_params_in_expr(then_branch, subst);
            if let Some(e) = else_branch {
                subst_type_params_in_expr(e, subst);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            subst_type_params_in_expr(scrutinee, subst);
            for arm in arms {
                if let Some(g) = &mut arm.guard {
                    subst_type_params_in_expr(g, subst);
                }
                subst_type_params_in_expr(&mut arm.body, subst);
            }
        }
        HirExprKind::Loop { body } => subst_type_params_in_expr(body, subst),
        HirExprKind::While { condition, body } => {
            subst_type_params_in_expr(condition, subst);
            subst_type_params_in_expr(body, subst);
        }
        HirExprKind::For { iterable, body, .. } => {
            subst_type_params_in_expr(iterable, subst);
            subst_type_params_in_expr(body, subst);
        }
        HirExprKind::Assign { target, value, .. } => {
            subst_type_params_in_expr(target, subst);
            subst_type_params_in_expr(value, subst);
        }
        HirExprKind::CompoundAssign { target, value, .. } => {
            subst_type_params_in_expr(target, subst);
            subst_type_params_in_expr(value, subst);
        }
        HirExprKind::Return(e) | HirExprKind::Break(e) => {
            if let Some(inner) = e {
                subst_type_params_in_expr(inner, subst);
            }
        }
        HirExprKind::Closure { body, .. } => {
            subst_type_params_in_expr(body, subst);
        }
        HirExprKind::Construct { fields, .. } | HirExprKind::EnumVariant { fields, .. } => {
            for (_, e) in fields {
                subst_type_params_in_expr(e, subst);
            }
        }
        HirExprKind::Tuple(elems) | HirExprKind::ArrayLiteral(elems) => {
            for e in elems {
                subst_type_params_in_expr(e, subst);
            }
        }
        HirExprKind::Index { object, index } => {
            subst_type_params_in_expr(object, subst);
            subst_type_params_in_expr(index, subst);
        }
        HirExprKind::Cast {
            expr: inner,
            target,
        } => {
            subst_type_params_in_expr(inner, subst);
            subst_type_params_in_ty(target, subst);
        }
        HirExprKind::ArrayFill { value, .. } => {
            subst_type_params_in_expr(value, subst);
        }
        HirExprKind::Range { start, end, .. } => {
            if let Some(s) = start {
                subst_type_params_in_expr(s, subst);
            }
            if let Some(e) = end {
                subst_type_params_in_expr(e, subst);
            }
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                if let HirInterpolationPart::Expr { expr: e, .. } = p {
                    subst_type_params_in_expr(e, subst);
                }
            }
        }
        HirExprKind::MacroCall { args, .. } => {
            for a in args {
                subst_type_params_in_expr(a, subst);
            }
        }
        _ => {}
    }
}

fn subst_type_params_in_stmt(
    stmt: &mut HirStatement,
    subst: &std::collections::HashMap<String, Ty>,
) {
    match stmt {
        HirStatement::Let { ty, value, .. } => {
            subst_type_params_in_ty(ty, subst);
            if let Some(v) = value {
                subst_type_params_in_expr(v, subst);
            }
        }
        HirStatement::Expr(e) => subst_type_params_in_expr(e, subst),
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
