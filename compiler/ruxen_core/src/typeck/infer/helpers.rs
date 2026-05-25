//! Free helper functions used by the inference engine.
//!
//! These do not need (and historically did not have) access to
//! `InferenceEngine`'s private state — they are pure functions on `Ty`
//! and HIR nodes, plus two helpers that take `&mut InferenceEngine` so
//! they can synthesise inference variables.

use crate::hir::nodes::{DefId, *};
use crate::hir::types::Ty;
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
            let mut field_tys: std::collections::HashMap<String, Ty> =
                std::collections::HashMap::new();
            for fid in &field_def_ids {
                if let Some(fdef) = symbols.get(*fid) {
                    if let DefKind::Field { ty, .. } = &fdef.kind {
                        let subst_ty =
                            substitute_generic_params(ty, &generic_params, &generic_args);
                        field_tys.insert(fdef.name.clone(), subst_ty);
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
            for (field_pat, declared_ty) in fields.iter().zip(payload_tys.iter()) {
                let subst_ty =
                    substitute_generic_params(declared_ty, &generic_param_names, &generic_args);
                propagate_pattern_types(field_pat, &subst_ty, symbols);
            }
        }
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

/// Walk `ty` replacing every `Ty::Param(name, _)` with the matching
/// argument from `(generic_params, generic_args)`. When the lookup
/// fails the param is left in place — typeck will surface the
/// mismatch via the downstream unify.
fn substitute_generic_params(ty: &Ty, generic_params: &[String], generic_args: &[Ty]) -> Ty {
    match ty {
        Ty::TypeParam { name, .. } => generic_params
            .iter()
            .position(|p| p == name)
            .and_then(|i| generic_args.get(i).cloned())
            .unwrap_or_else(|| ty.clone()),
        Ty::Ref(inner) => Ty::Ref(Box::new(substitute_generic_params(
            inner,
            generic_params,
            generic_args,
        ))),
        Ty::RefMut(inner) => Ty::RefMut(Box::new(substitute_generic_params(
            inner,
            generic_params,
            generic_args,
        ))),
        Ty::Option(inner) => Ty::Option(Box::new(substitute_generic_params(
            inner,
            generic_params,
            generic_args,
        ))),
        Ty::Result(ok, err) => Ty::Result(
            Box::new(substitute_generic_params(ok, generic_params, generic_args)),
            Box::new(substitute_generic_params(err, generic_params, generic_args)),
        ),
        Ty::Array(elem) => Ty::Array(Box::new(substitute_generic_params(
            elem,
            generic_params,
            generic_args,
        ))),
        Ty::Set(elem) => Ty::Set(Box::new(substitute_generic_params(
            elem,
            generic_params,
            generic_args,
        ))),
        Ty::Map(k, v) => Ty::Map(
            Box::new(substitute_generic_params(k, generic_params, generic_args)),
            Box::new(substitute_generic_params(v, generic_params, generic_args)),
        ),
        Ty::Tuple(slots) => Ty::Tuple(
            slots
                .iter()
                .map(|s| substitute_generic_params(s, generic_params, generic_args))
                .collect(),
        ),
        Ty::Class {
            name,
            generic_args: ga,
        } => Ty::Class {
            name: name.clone(),
            generic_args: ga
                .iter()
                .map(|g| substitute_generic_params(g, generic_params, generic_args))
                .collect(),
        },
        Ty::Enum {
            name,
            generic_args: ga,
        } => Ty::Enum {
            name: name.clone(),
            generic_args: ga
                .iter()
                .map(|g| substitute_generic_params(g, generic_params, generic_args))
                .collect(),
        },
        Ty::Struct {
            name,
            generic_args: ga,
        } => Ty::Struct {
            name: name.clone(),
            generic_args: ga
                .iter()
                .map(|g| substitute_generic_params(g, generic_params, generic_args))
                .collect(),
        },
        other => other.clone(),
    }
}

/// Whether `*Iter[T].sum` should be accepted at typeck.
///
/// The runtime path is `ruxen_vec_sum`, an integer-only summation over
/// the raw 64-bit slot. Allowing only `Add`-style numeric Items here
/// matches that runtime contract: any pointer-shaped Item (String,
/// Vec, HashMap, custom class) would silently produce nonsense.
///
/// `Ty::Infer` and `Ty::Error` pass through — they will be either
/// pinned to a numeric type by a downstream constraint or already
/// surfaced as a separate diagnostic. Phase 2 stdlib (#05 batch 3).
///
/// Extract the element type from a container type produced by a v1
/// collection macro: `Array[T]`, `Set[T]`, or `FixedArray[T; N]`.
pub(super) fn container_elem_ty(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Array(elem) | Ty::Set(elem) | Ty::Option(elem) => Some((**elem).clone()),
        Ty::FixedArray(elem, _) => Some((**elem).clone()),
        _ => None,
    }
}

/// Extract `(K, V)` from a `Map[K, V]` type.
pub(super) fn map_kv_tys(ty: &Ty) -> Option<(Ty, Ty)> {
    match ty {
        Ty::Map(k, v) => Some(((**k).clone(), (**v).clone())),
        _ => None,
    }
}

/// Phase 2 #06.5 T6: BufReader[R] / BufWriter[W] restrict R/W to the
/// closed set {File, TcpStream}. v1 deliberately ships no formal
/// Read/Write trait (deferred to v1.5 with the Iterator mixin); the
/// runtime branches on a 1-byte `kind` tag baked into the spine. Any
/// other inner type at typeck → E0714. `Ty::Infer` and `Ty::Error`
/// pass through so we don't double-report when the inner is already a
/// type-error.
pub(in crate::typeck) fn is_bufio_inner_supported(ty: &Ty) -> bool {
    let peeled = peel_refs(ty);
    matches!(
        peeled,
        Ty::Class { name, .. } if name == "File" || name == "TcpStream"
    ) || matches!(peeled, Ty::Infer(_) | Ty::Error)
}

/// Strip outer reference layers (`&T`, `&var T`) so the wrapped type
/// can be matched directly. Returns the inner Ty by reference.
fn peel_refs(ty: &Ty) -> &Ty {
    let mut cur = ty;
    loop {
        match cur {
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => cur = inner,
            _ => return cur,
        }
    }
}

pub(in crate::typeck) fn is_iter_sum_compatible(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Int
            | Ty::Int8
            | Ty::Int16
            | Ty::Int32
            | Ty::Int64
            | Ty::ISize
            | Ty::UInt
            | Ty::UInt8
            | Ty::UInt16
            | Ty::UInt32
            | Ty::UInt64
            | Ty::USize
            | Ty::Float
            | Ty::Float32
            | Ty::Float64
            | Ty::Infer(_)
            | Ty::Error
    )
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
/// `For` bodies — those breaks belong to the inner loop, not ours.
pub(super) fn collect_break_types(
    expr: &HirExpr,
    ctx: &mut crate::hir::context::TypeContext,
    acc: &mut Option<Ty>,
    loop_span: &Span,
) {
    match &expr.kind {
        HirExprKind::Break(value) => {
            let t = match value {
                Some(v) => ctx.resolve(&v.ty),
                None => Ty::Unit,
            };
            match acc {
                Some(prev) => {
                    let _ = unify(prev, &t, ctx, loop_span);
                }
                None => *acc = Some(t),
            }
        }
        // Nested loops own their own breaks — do not descend.
        HirExprKind::Loop { .. } | HirExprKind::While { .. } | HirExprKind::For { .. } => {}
        // Returns never flow into our loop's result either; skip entirely.
        HirExprKind::Return(_) => {}

        // Structural recursion through every expression kind that can
        // syntactically contain a `break`.
        HirExprKind::Block(stmts, tail) => {
            for s in stmts {
                match s {
                    HirStatement::Let { value: Some(v), .. } => {
                        collect_break_types(v, ctx, acc, loop_span);
                    }
                    HirStatement::Expr(e) => {
                        collect_break_types(e, ctx, acc, loop_span);
                    }
                    _ => {}
                }
            }
            if let Some(t) = tail {
                collect_break_types(t, ctx, acc, loop_span);
            }
        }
        HirExprKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_break_types(cond, ctx, acc, loop_span);
            collect_break_types(then_branch, ctx, acc, loop_span);
            if let Some(e) = else_branch {
                collect_break_types(e, ctx, acc, loop_span);
            }
        }
        HirExprKind::Match { scrutinee, arms } => {
            collect_break_types(scrutinee, ctx, acc, loop_span);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_break_types(g, ctx, acc, loop_span);
                }
                collect_break_types(&arm.body, ctx, acc, loop_span);
            }
        }
        HirExprKind::BinaryOp { left, right, .. } => {
            collect_break_types(left, ctx, acc, loop_span);
            collect_break_types(right, ctx, acc, loop_span);
        }
        HirExprKind::UnaryOp { operand, .. } => {
            collect_break_types(operand, ctx, acc, loop_span);
        }
        HirExprKind::Borrow { expr: inner, .. } => {
            collect_break_types(inner, ctx, acc, loop_span);
        }
        HirExprKind::Assign { target, value, .. }
        | HirExprKind::CompoundAssign { target, value, .. } => {
            collect_break_types(target, ctx, acc, loop_span);
            collect_break_types(value, ctx, acc, loop_span);
        }
        HirExprKind::FnCall { args, .. } => {
            for a in args {
                collect_break_types(a, ctx, acc, loop_span);
            }
        }
        HirExprKind::MethodCall {
            object,
            args,
            block,
            ..
        } => {
            collect_break_types(object, ctx, acc, loop_span);
            for a in args {
                collect_break_types(a, ctx, acc, loop_span);
            }
            if let Some(b) = block {
                collect_break_types(b, ctx, acc, loop_span);
            }
        }
        HirExprKind::FieldAccess { object, .. } => {
            collect_break_types(object, ctx, acc, loop_span);
        }
        HirExprKind::Interpolation { parts } => {
            for p in parts {
                if let HirInterpolationPart::Expr { expr: e, .. } = p {
                    collect_break_types(e, ctx, acc, loop_span);
                }
            }
        }
        // Other expression kinds cannot contain a break that targets
        // our loop (they are leaves, closures, or type-level nodes).
        _ => {}
    }
}
