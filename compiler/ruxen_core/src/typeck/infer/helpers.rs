//! Free helper functions used by the inference engine.
//!
//! These do not need (and historically did not have) access to
//! `InferenceEngine`'s private state — they are pure functions on `Ty`
//! and HIR nodes, plus two helpers that take `&mut InferenceEngine` so
//! they can synthesise inference variables.

use crate::hir::nodes::{DefId, *};
use crate::hir::types::Ty;
use crate::hir::visit::{walk_expr, Visit};
use crate::lexer::token::Span;
use crate::resolve::symbols::{DefKind, SymbolTable, VariantDefKind};

use super::super::unify::unify;
use super::InferenceEngine;

/// Propagate `scrutinee_ty` into each `Binding`'s DefId inside `pattern`.
///
/// Match-arm patterns and `let <pat> = e` destructurings allocate
/// DefIds at resolve time but never get their types updated by the
/// inference engine. The result is every inner pattern binding ending
/// up with `Ty::Infer`, which surfaces at codegen as `?T<id>_<method>`
/// symbols the runtime can't resolve — e.g. `Some(t) -> t.describe`
/// mangles `t.describe` to `?T518_describe`.
///
/// This walker structurally matches the pattern against the
/// scrutinee's resolved type and writes each binding's concrete
/// type back into the symbol table before the arm body is inferred.
pub(super) fn propagate_pattern_types(
    pattern: &HirPattern,
    scrutinee_ty: &Ty,
    symbols: &mut SymbolTable,
) {
    // Destructuring a borrow (`match self.next` on an `&Option[Node]` field
    // read) matches the borrowed value and binds each non-Copy payload as a
    // borrow of it: the scrutinee's owner keeps owning the payload.
    if let (
        Ty::Ref(inner),
        HirPattern::Tuple { .. } | HirPattern::Enum { .. } | HirPattern::Struct { .. },
    ) = (scrutinee_ty, pattern)
    {
        propagate_pattern_types(pattern, inner, symbols);
        borrow_pattern_bindings(pattern, symbols);
        return;
    }
    // A tuple keeps owning its elements, so destructuring one binds each
    // non-Copy element as a borrow of it, exactly as reading `t.0` does.
    if let (Ty::Tuple(_), HirPattern::Tuple { .. }) = (scrutinee_ty, pattern) {
        propagate_owned_pattern_types(pattern, scrutinee_ty, symbols);
        borrow_pattern_bindings(pattern, symbols);
        return;
    }
    propagate_owned_pattern_types(pattern, scrutinee_ty, symbols);
}

/// [`propagate_pattern_types`] on a scrutinee the pattern owns.
fn propagate_owned_pattern_types(
    pattern: &HirPattern,
    scrutinee_ty: &Ty,
    symbols: &mut SymbolTable,
) {
    match pattern {
        HirPattern::Wildcard { .. } | HirPattern::Rest { .. } | HirPattern::Literal { .. } => {}
        HirPattern::Binding { def_id, .. } => {
            symbols.update_ty(*def_id, scrutinee_ty.clone());
        }
        HirPattern::Ref {
            def_id, mutable, ..
        } => {
            let ref_ty = if *mutable {
                Ty::RefMut(Box::new(scrutinee_ty.clone()))
            } else {
                Ty::Ref(Box::new(scrutinee_ty.clone()))
            };
            symbols.update_ty(*def_id, ref_ty);
        }
        HirPattern::Tuple { elements, .. } => {
            if let Ty::Tuple(slots) = scrutinee_ty {
                for (elem, slot_ty) in elements.iter().zip(slots.iter()) {
                    propagate_pattern_types(elem, slot_ty, symbols);
                }
            }
        }
        HirPattern::Or { patterns, .. } => {
            for p in patterns {
                propagate_pattern_types(p, scrutinee_ty, symbols);
            }
        }
        HirPattern::Struct {
            type_def, fields, ..
        } => {
            let struct_def = match symbols.get(*type_def).cloned() {
                Some(d) => d,
                None => return,
            };
            let generic_args = scrutinee_class_generic_args(scrutinee_ty);
            let (field_def_ids, generic_params): (Vec<DefId>, Vec<String>) = match &struct_def.kind
            {
                DefKind::Class { info } => (
                    info.fields.clone(),
                    info.generic_params
                        .iter()
                        .map(|gp| gp.name.clone())
                        .collect(),
                ),
                DefKind::Struct { info } => (
                    info.fields.clone(),
                    info.generic_params
                        .iter()
                        .map(|gp| gp.name.clone())
                        .collect(),
                ),
                _ => return,
            };
            let subst = generic_subst(generic_params, generic_args);
            let mut field_tys: std::collections::HashMap<String, Ty> =
                std::collections::HashMap::new();
            for fid in &field_def_ids {
                if let Some(fdef) = symbols.get(*fid) {
                    if let DefKind::Field { ty, .. } = &fdef.kind {
                        field_tys.insert(fdef.name.clone(), ty.subst_params(&subst));
                    }
                }
            }
            for (name, inner_pat) in fields {
                if let Some(t) = field_tys.get(name).cloned() {
                    propagate_pattern_types(inner_pat, &t, symbols);
                }
            }
        }
        HirPattern::Enum {
            variant_name,
            fields,
            type_def,
            ..
        } => {
            // Built-in `Option[T]` / `Result[T, E]` are modelled as
            // dedicated `Ty::Option` / `Ty::Result` variants — peel
            // payload types directly instead of walking the symbol
            // table.
            match scrutinee_ty {
                Ty::Option(inner) => {
                    if variant_name == "Some" {
                        if let Some(field) = fields.first() {
                            propagate_pattern_types(field, inner, symbols);
                        }
                    }
                    return;
                }
                Ty::Result(ok, err) => {
                    let payload = if variant_name == "Ok" {
                        Some(ok.as_ref())
                    } else if variant_name == "Err" {
                        Some(err.as_ref())
                    } else {
                        None
                    };
                    if let Some(p) = payload {
                        if let Some(field) = fields.first() {
                            propagate_pattern_types(field, p, symbols);
                        }
                    }
                    return;
                }
                _ => {}
            }

            // User-defined enum: resolve `type_def` to its `EnumInfo`,
            // pull the variant's DefId by index, then read the
            // payload's declared types from that variant's
            // `VariantDefKind`. Substitute generic params from the
            // scrutinee's generic args before binding each field.
            let enum_def = match symbols.get(*type_def).cloned() {
                Some(d) => d,
                None => return,
            };
            let enum_info = match &enum_def.kind {
                DefKind::Enum { info } => info.clone(),
                _ => return,
            };
            // The HirPattern carries `variant_idx`; rebind it here to
            // make the index lookup unambiguous.
            let variant_idx = match pattern {
                HirPattern::Enum { variant_idx, .. } => *variant_idx,
                _ => return,
            };
            let variant_def_id = match enum_info.variants.get(variant_idx) {
                Some(id) => *id,
                None => return,
            };
            let variant_def = match symbols.get(variant_def_id).cloned() {
                Some(d) => d,
                None => return,
            };
            let payload_tys: Vec<Ty> = match &variant_def.kind {
                DefKind::EnumVariant { kind, .. } => match kind {
                    VariantDefKind::Unit => return,
                    VariantDefKind::Tuple(tys) => tys.clone(),
                    VariantDefKind::Struct(named) => named.iter().map(|(_, t)| t.clone()).collect(),
                },
                _ => return,
            };
            let generic_args = scrutinee_class_generic_args(scrutinee_ty);
            let generic_param_names: Vec<String> = enum_info
                .generic_params
                .iter()
                .map(|gp| gp.name.clone())
                .collect();
            let subst = generic_subst(generic_param_names, generic_args);
            for (field_pat, declared_ty) in fields.iter().zip(payload_tys.iter()) {
                propagate_pattern_types(field_pat, &declared_ty.subst_params(&subst), symbols);
            }
        }
    }
}

/// The type of reading a value out of a slot its container keeps owning (a
/// class field, a tuple element, a destructured payload): a non-Copy `T` is
/// read as the borrow `&T` (TODO.md Decisions, "Fields"). Copy values,
/// borrows, and types not yet known read as themselves.
pub(super) fn slot_read_ty(ty: Ty, symbols: &SymbolTable) -> Ty {
    if matches!(ty, Ty::Ref(_) | Ty::RefMut(_))
        || ty.is_copy_with(symbols)
        || ty.is_infer()
        || ty.is_error()
    {
        ty
    } else {
        Ty::Ref(Box::new(ty))
    }
}

/// Re-type every by-value binding inside a destructuring `pattern` as its
/// [`slot_read_ty`] (see [`propagate_pattern_types`] on a borrowed scrutinee
/// or a tuple).
fn borrow_pattern_bindings(pattern: &HirPattern, symbols: &mut SymbolTable) {
    match pattern {
        HirPattern::Binding { def_id, .. } => {
            let Some(ty) = symbols.def_ty(*def_id) else {
                return;
            };
            let read = slot_read_ty(ty.clone(), symbols);
            if read != ty {
                symbols.update_ty(*def_id, read);
            }
        }
        HirPattern::Tuple { elements, .. } => {
            for elem in elements {
                borrow_pattern_bindings(elem, symbols);
            }
        }
        HirPattern::Enum { fields, .. } => {
            for field in fields {
                borrow_pattern_bindings(field, symbols);
            }
        }
        HirPattern::Struct { fields, .. } => {
            for (_, field) in fields {
                borrow_pattern_bindings(field, symbols);
            }
        }
        HirPattern::Or { patterns, .. } => {
            for p in patterns {
                borrow_pattern_bindings(p, symbols);
            }
        }
        HirPattern::Wildcard { .. }
        | HirPattern::Rest { .. }
        | HirPattern::Literal { .. }
        | HirPattern::Ref { .. } => {}
    }
}

/// Extract the generic-argument list from a class/enum/struct type.
fn scrutinee_class_generic_args(ty: &Ty) -> Vec<Ty> {
    match ty {
        Ty::Class { generic_args, .. }
        | Ty::Enum { generic_args, .. }
        | Ty::Struct { generic_args, .. } => generic_args.clone(),
        _ => Vec::new(),
    }
}

/// Pair a type's declared generic param names with a use site's generic
/// args. A param with no matching arg is left out, so substitution leaves it
/// in place and typeck surfaces the mismatch via the downstream unify.
fn generic_subst(params: Vec<String>, args: Vec<Ty>) -> std::collections::HashMap<String, Ty> {
    params.into_iter().zip(args).collect()
}

/// Infer the concrete `generic_args` for a user-defined enum variant
/// constructor.  Builds a substitution from each declared generic param
/// name to the concrete arg type observed at the matching payload slot,
/// then resolves each declared generic param through that substitution.
/// Generic params with no matching payload slot (e.g. the unit variant
/// `MyOpt.None` has no payload) are filled from the enclosing return
/// type (if it's the same enum) or a fresh inference variable.
pub(super) fn infer_user_enum_generic_args(
    engine: &mut InferenceEngine<'_>,
    type_def: DefId,
    variant_idx: usize,
    fields: &[(String, HirExpr)],
    type_name: &str,
) -> Vec<Ty> {
    use crate::resolve::symbols::{DefKind, VariantDefKind};

    let (generic_param_names, variant_field_tys): (Vec<String>, Vec<Ty>) = {
        let enum_def = match engine.symbols.get(type_def) {
            Some(d) => d,
            None => return vec![],
        };
        let info = match &enum_def.kind {
            DefKind::Enum { info } => info,
            _ => return vec![],
        };
        let param_names: Vec<String> = info
            .generic_params
            .iter()
            .map(|gp| gp.name.clone())
            .collect();
        if param_names.is_empty() {
            return vec![];
        }
        let variant_def_id = match info.variants.get(variant_idx).copied() {
            Some(id) => id,
            None => return vec![],
        };
        let variant_def = match engine.symbols.get(variant_def_id) {
            Some(d) => d,
            None => return vec![],
        };
        let field_tys: Vec<Ty> = match &variant_def.kind {
            DefKind::EnumVariant { kind, .. } => match kind {
                VariantDefKind::Tuple(tys) => tys.clone(),
                VariantDefKind::Struct(fs) => fs.iter().map(|(_, t)| t.clone()).collect(),
                VariantDefKind::Unit => vec![],
            },
            _ => vec![],
        };
        (param_names, field_tys)
    };

    // Match each declared payload slot to the actual arg type and build
    // a name -> concrete-ty substitution.
    let mut subst: std::collections::HashMap<String, Ty> = std::collections::HashMap::new();
    for (decl_ty, (_, arg_expr)) in variant_field_tys.iter().zip(fields.iter()) {
        let arg_ty = engine.ctx.resolve(&arg_expr.ty);
        record_tyvar_binding(decl_ty, &arg_ty, &mut subst);
    }

    // For any generic param we didn't pin, try the enclosing return type
    // (`Ty::Enum { name: type_name, generic_args: [...] }`), else fall
    // back to a fresh inference variable.
    let return_args: Option<Vec<Ty>> =
        engine.current_return_ty.as_ref().and_then(|ret| match ret {
            Ty::Enum { name, generic_args } if name == type_name => Some(generic_args.clone()),
            _ => None,
        });

    generic_param_names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            if let Some(t) = subst.get(name) {
                return t.clone();
            }
            if let Some(args) = &return_args {
                if let Some(t) = args.get(idx) {
                    return t.clone();
                }
            }
            engine.ctx.fresh_type_var()
        })
        .collect()
}

/// Walk `decl_ty` (a declared variant field type that may contain
/// `Ty::TypeParam { name }` placeholders) alongside `arg_ty` (the
/// concrete argument type) and record each placeholder name's concrete
/// binding into `subst`. Mismatched shapes silently drop their
/// contribution — the type checker's normal unification later flags
/// any genuine error.
fn record_tyvar_binding(
    decl_ty: &Ty,
    arg_ty: &Ty,
    subst: &mut std::collections::HashMap<String, Ty>,
) {
    match (decl_ty, arg_ty) {
        (Ty::TypeParam { name, .. }, concrete) => {
            subst
                .entry(name.clone())
                .or_insert_with(|| concrete.clone());
        }
        (Ty::Option(a), Ty::Option(b)) => record_tyvar_binding(a, b, subst),
        (Ty::Array(a), Ty::Array(b)) => record_tyvar_binding(a, b, subst),
        (Ty::Ref(a), Ty::Ref(b)) => record_tyvar_binding(a, b, subst),
        (Ty::RefMut(a), Ty::RefMut(b)) => record_tyvar_binding(a, b, subst),
        (Ty::Result(a1, a2), Ty::Result(b1, b2)) => {
            record_tyvar_binding(a1, b1, subst);
            record_tyvar_binding(a2, b2, subst);
        }
        (Ty::Tuple(a), Ty::Tuple(b)) if a.len() == b.len() => {
            for (x, y) in a.iter().zip(b.iter()) {
                record_tyvar_binding(x, y, subst);
            }
        }
        (
            Ty::Enum {
                name: an,
                generic_args: aa,
            },
            Ty::Enum {
                name: bn,
                generic_args: ba,
            },
        ) if an == bn && aa.len() == ba.len() => {
            for (x, y) in aa.iter().zip(ba.iter()) {
                record_tyvar_binding(x, y, subst);
            }
        }
        _ => {}
    }
}

/// Walk a loop body collecting the types of every `break VALUE` (and
/// recording `Unit` for bare `break`) so the enclosing `loop` expression
/// can be given a precise type. Recursion stops at nested `Loop`/`While`/
/// `For` bodies (those breaks belong to the inner loop, not ours), at nested
/// closures (a `break` cannot cross a closure boundary) and at `return`.
pub(super) fn collect_break_types(
    expr: &HirExpr,
    ctx: &mut crate::hir::context::TypeContext,
    acc: &mut Option<Ty>,
    loop_span: &Span,
) {
    JumpTypes {
        jump: Jump::Break,
        ctx,
        acc,
        span: loop_span,
    }
    .visit_expr(expr);
}

/// Collect the value types of every `next expr` that belongs to THIS block
/// (Block Tier-2). Mirrors `collect_break_types` but: a `next` targets the
/// enclosing *closure*, so it descends through nested loops (a `next` inside a
/// `for`/`while` in the block still belongs to the block) and STOPS at a nested
/// closure boundary (that closure's `next` is its own). `return` is skipped —
/// it is a separate jump. The accumulator unifies all collected types.
pub(super) fn collect_next_types(
    expr: &HirExpr,
    ctx: &mut crate::hir::context::TypeContext,
    acc: &mut Option<Ty>,
    block_span: &Span,
) {
    JumpTypes {
        jump: Jump::Next,
        ctx,
        acc,
        span: block_span,
    }
    .visit_expr(expr);
}

/// Which jump `JumpTypes` collects value types for.
#[derive(Clone, Copy)]
enum Jump {
    Break,
    Next,
}

/// Shared walk behind `collect_break_types` and `collect_next_types`: unifies
/// the value type of every jump of kind `jump` that targets the construct
/// being typed into `acc`.
struct JumpTypes<'a> {
    jump: Jump,
    ctx: &'a mut crate::hir::context::TypeContext,
    acc: &'a mut Option<Ty>,
    span: &'a Span,
}

impl Visit for JumpTypes<'_> {
    fn visit_expr(&mut self, expr: &HirExpr) {
        let (value, boundary) = match self.jump {
            Jump::Break => (
                if let HirExprKind::Break(value) = &expr.kind {
                    Some(value)
                } else {
                    None
                },
                // A nested loop owns its own breaks.
                matches!(
                    expr.kind,
                    HirExprKind::Loop { .. }
                        | HirExprKind::While { .. }
                        | HirExprKind::For { .. }
                        | HirExprKind::Closure { .. }
                        | HirExprKind::Return(_)
                ),
            ),
            Jump::Next => (
                if let HirExprKind::Next(value) = &expr.kind {
                    Some(value)
                } else {
                    None
                },
                // Unlike `break`, a `next` inside a nested loop still targets
                // this block, so only a nested closure is a boundary.
                matches!(
                    expr.kind,
                    HirExprKind::Closure { .. } | HirExprKind::Return(_)
                ),
            ),
        };
        if let Some(value) = value {
            let t = match value {
                Some(v) => self.ctx.resolve(&v.ty),
                None => Ty::Unit,
            };
            match self.acc {
                Some(prev) => {
                    let _ = unify(prev, &t, self.ctx, self.span);
                }
                None => *self.acc = Some(t),
            }
        }
        if !boundary {
            walk_expr(self, expr);
        }
    }
}

/// Whether a parallel-assignment RHS value is *freshly produced* and so
/// cannot alias one of the assignment targets.
///
/// The MIR lowering snapshots each RHS value into a temporary before any
/// target is written, but for an owned heap aggregate (`Class`/`Struct`/
/// `Enum`) that snapshot is a pointer copy, not a deep clone. A value read
/// from a *place* (a variable, field, or index) therefore aliases the
/// existing object, and the pre-reassign `dealloc` for the target
/// double-frees it. Only a value produced afresh by this expression is safe.
///
/// This peels the wrapper forms (`if`/ternary, `match`, `cast`, block) and
/// requires *every* value that could be produced to be fresh, so a ternary
/// of two constructors is fresh but a ternary of two variable reads is not.
pub(super) fn multiassign_value_is_fresh(expr: &HirExpr) -> bool {
    match &expr.kind {
        // A call, constructor, or enum-variant literal yields a brand-new
        // owned value that cannot alias one of the targets.
        HirExprKind::MethodCall { .. }
        | HirExprKind::FnCall { .. }
        | HirExprKind::Construct { .. }
        | HirExprKind::EnumVariant { .. } => true,
        // Operator overloads on owned aggregates are not trusted here. The
        // explicit method-call form is checked by move analysis, but the
        // sugared operator reaches MIR as a BinaryOp/UnaryOp and can otherwise
        // forward an assignment target through the operator result.
        // A ternary/`if` or `match` is fresh iff EVERY branch is fresh.
        HirExprKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            multiassign_value_is_fresh(then_branch)
                && else_branch
                    .as_deref()
                    .is_some_and(multiassign_value_is_fresh)
        }
        HirExprKind::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|arm| multiassign_value_is_fresh(&arm.body))
        }
        // A cast reinterprets, and a block forwards its tail — recurse.
        HirExprKind::Cast { expr: inner, .. } => multiassign_value_is_fresh(inner),
        HirExprKind::Block(_, Some(tail)) => multiassign_value_is_fresh(tail),
        // A place read (variable / field / index) aliases an existing owned
        // object; anything else is conservatively treated as aliasing.
        _ => false,
    }
}
