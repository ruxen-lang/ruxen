//! Type coercions for the Ruxen type system.
//!
//! Coercions are implicit type conversions that the compiler performs
//! automatically. They are weaker than full unification — they only
//! apply in specific contexts (assignment, function arguments, etc.)

use crate::hir::context::TypeContext;
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::resolve::symbols::SymbolTable;

use super::unify::TypeError;

/// Attempt to coerce `from` to `to`. Returns the coerced type if successful.
pub fn try_coerce(
    from: &Ty,
    to: &Ty,
    ctx: &mut TypeContext,
    _symbols: &SymbolTable,
    span: &Span,
) -> Result<Ty, TypeError> {
    let from = ctx.resolve(from);
    let to = ctx.resolve(to);

    if from == to {
        return Ok(to);
    }

    match (&from, &to) {
        // Inference variables — bind
        (Ty::Infer(id), _) => {
            ctx.bind(*id, to.clone()).map_err(|msg| TypeError {
                message: msg,
                expected: to.clone(),
                found: from.clone(),
                span: span.clone(),
            })?;
            Ok(to)
        }
        (_, Ty::Infer(id)) => {
            ctx.bind(*id, from.clone()).map_err(|msg| TypeError {
                message: msg,
                expected: to.clone(),
                found: from.clone(),
                span: span.clone(),
            })?;
            Ok(from)
        }

        // Never coerces to anything
        (Ty::Never, _) => Ok(to),

        // Error type
        (Ty::Error, _) => Ok(to),
        (_, Ty::Error) => Ok(from),

        // &mut T → &T
        (Ty::RefMut(inner), Ty::Ref(target_inner)) => {
            let coerced_inner = try_coerce(inner, target_inner, ctx, _symbols, span)?;
            Ok(Ty::Ref(Box::new(coerced_inner)))
        }

        // &String → &str
        (Ty::Ref(inner), Ty::Str) if **inner == Ty::String => Ok(Ty::Str),

        // Auto-deref: &&T → &T (for method calls and field access)
        (Ty::Ref(inner), target) if inner.is_ref() => {
            try_coerce(inner, target, ctx, _symbols, span)
        }

        // Integer widening
        (from_ty, to_ty) if from_ty.is_integer() && to_ty.is_integer() => {
            match (from_ty.bit_width(), to_ty.bit_width()) {
                (Some(fw), Some(tw))
                    if fw <= tw && from_ty.is_signed_integer() == to_ty.is_signed_integer() =>
                {
                    Ok(to)
                }
                _ => Err(TypeError::mismatch(&from, &to, span)),
            }
        }

        // Float widening
        (Ty::Float32, Ty::Float64) | (Ty::Float32, Ty::Float) => Ok(to),

        // Int → Float coercion (backward inference: `let x: Float = 42`)
        (Ty::Int, Ty::Float) | (Ty::Int, Ty::Float64) => Ok(to),
        (Ty::Int, Ty::Float32) => Ok(to),

        // Option covariance: Option[&Child] → Option[&Parent]
        (Ty::Option(inner_from), Ty::Option(inner_to)) => {
            let inner = try_coerce(inner_from, inner_to, ctx, _symbols, span)?;
            Ok(Ty::Option(Box::new(inner)))
        }

        // Result covariance on Ok type
        (Ty::Result(ok_from, err_from), Ty::Result(ok_to, err_to)) => {
            let ok = try_coerce(ok_from, ok_to, ctx, _symbols, span)?;
            let err = try_coerce(err_from, err_to, ctx, _symbols, span)?;
            Ok(Ty::Result(Box::new(ok), Box::new(err)))
        }

        // Inheritance subtyping: &Child → &Parent (references only)
        (Ty::Ref(inner_from), Ty::Ref(inner_to)) => {
            if is_subtype_class(inner_from, inner_to, _symbols) {
                Ok(to)
            } else {
                // Try regular unification
                let inner = try_coerce(inner_from, inner_to, ctx, _symbols, span)?;
                Ok(Ty::Ref(Box::new(inner)))
            }
        }

        // Vec, HashMap, Set — invariant (no coercion)
        // &mut T — invariant (no coercion to &mut U)

        // No coercion possible
        _ => Err(TypeError::mismatch(&from, &to, span)),
    }
}

/// Check if `child` is a subtype of `parent` through class inheritance.
fn is_subtype_class(child: &Ty, parent: &Ty, symbols: &SymbolTable) -> bool {
    match (child, parent) {
        (
            Ty::Class {
                name: child_name, ..
            },
            Ty::Class {
                name: parent_name, ..
            },
        ) => {
            if child_name == parent_name {
                return true;
            }
            // Walk the inheritance chain
            for def in symbols.iter() {
                if def.name == *child_name {
                    if let crate::resolve::symbols::DefKind::Class { info } = &def.kind {
                        if let Some(parent_id) = info.parent {
                            if let Some(parent_def) = symbols.get(parent_id) {
                                if parent_def.name == *parent_name {
                                    return true;
                                }
                                // Recurse up the chain
                                let parent_ty = Ty::Class {
                                    name: parent_def.name.clone(),
                                    generic_args: vec![],
                                };
                                return is_subtype_class(&parent_ty, parent, symbols);
                            }
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// Apply auto-deref chain: follow references until reaching a non-reference type.
/// Returns the deref chain depth and the final type.
pub fn auto_deref(ty: &Ty, ctx: &TypeContext) -> (usize, Ty) {
    let resolved = ctx.resolve(ty);
    let mut current = resolved;
    let mut depth = 0;
    loop {
        match &current {
            Ty::Ref(inner) | Ty::RefMut(inner) => {
                current = ctx.resolve(inner);
                depth += 1;
            }
            Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => {
                current = ctx.resolve(inner);
                depth += 1;
            }
            _ => break,
        }
    }
    (depth, current)
}
