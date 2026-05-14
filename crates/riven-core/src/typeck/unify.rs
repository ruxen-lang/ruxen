//! Type unification — constraint solving for the type inference engine.
//!
//! Unification determines whether two types can be made equal by binding
//! inference variables. This is the core algorithm behind type inference.

use crate::hir::context::TypeContext;
use crate::hir::types::Ty;
use crate::lexer::token::Span;

/// A type error produced during unification.
#[derive(Debug, Clone)]
pub struct TypeError {
    pub message: String,
    pub expected: Ty,
    pub found: Ty,
    pub span: Span,
}

impl TypeError {
    pub fn mismatch(expected: &Ty, found: &Ty, span: &Span) -> Self {
        Self {
            message: format!("type mismatch: expected `{}`, found `{}`", expected, found),
            expected: expected.clone(),
            found: found.clone(),
            span: span.clone(),
        }
    }
}

/// Attempt to unify two types. If successful, binds inference variables
/// in the TypeContext so that the types become equal. Returns the unified
/// (most specific) type.
pub fn unify(a: &Ty, b: &Ty, ctx: &mut TypeContext, span: &Span) -> Result<Ty, TypeError> {
    let a = ctx.resolve(a);
    let b = ctx.resolve(b);

    // If both are the same concrete type, they unify trivially
    if a == b {
        return Ok(a);
    }

    match (&a, &b) {
        // Inference variables unify with anything
        (Ty::Infer(id), _) => {
            ctx.bind(*id, b.clone()).map_err(|msg| TypeError {
                message: msg,
                expected: a.clone(),
                found: b.clone(),
                span: span.clone(),
            })?;
            Ok(b)
        }
        (_, Ty::Infer(id)) => {
            ctx.bind(*id, a.clone()).map_err(|msg| TypeError {
                message: msg,
                expected: a.clone(),
                found: b.clone(),
                span: span.clone(),
            })?;
            Ok(a)
        }

        // Never unifies with anything (it's the bottom type)
        (Ty::Never, _) => Ok(b),
        (_, Ty::Never) => Ok(a),

        // Error type unifies with anything (for error recovery)
        (Ty::Error, _) => Ok(b),
        (_, Ty::Error) => Ok(a),

        // Tuples: element-wise unification
        (Ty::Tuple(a_elems), Ty::Tuple(b_elems)) => {
            if a_elems.len() != b_elems.len() {
                return Err(TypeError::mismatch(&a, &b, span));
            }
            let unified: Result<Vec<Ty>, TypeError> = a_elems
                .iter()
                .zip(b_elems.iter())
                .map(|(ae, be)| unify(ae, be, ctx, span))
                .collect();
            Ok(Ty::Tuple(unified?))
        }

        // Arrays: same element type and size
        (Ty::FixedArray(a_elem, a_size), Ty::FixedArray(b_elem, b_size)) => {
            if a_size != b_size {
                return Err(TypeError::mismatch(&a, &b, span));
            }
            let elem = unify(a_elem, b_elem, ctx, span)?;
            Ok(Ty::FixedArray(Box::new(elem), a_size.clone()))
        }

        // Vec
        (Ty::Array(a_elem), Ty::Array(b_elem)) => {
            let elem = unify(a_elem, b_elem, ctx, span)?;
            Ok(Ty::Array(Box::new(elem)))
        }

        // HashMap
        (Ty::Map(ak, av), Ty::Map(bk, bv)) => {
            let k = unify(ak, bk, ctx, span)?;
            let v = unify(av, bv, ctx, span)?;
            Ok(Ty::Map(Box::new(k), Box::new(v)))
        }

        // Set
        (Ty::Set(a_elem), Ty::Set(b_elem)) => {
            let elem = unify(a_elem, b_elem, ctx, span)?;
            Ok(Ty::Set(Box::new(elem)))
        }

        // Option
        (Ty::Option(a_inner), Ty::Option(b_inner)) => {
            let inner = unify(a_inner, b_inner, ctx, span)?;
            Ok(Ty::Option(Box::new(inner)))
        }

        // Result
        (Ty::Result(a_ok, a_err), Ty::Result(b_ok, b_err)) => {
            let ok = unify(a_ok, b_ok, ctx, span)?;
            let err = unify(a_err, b_err, ctx, span)?;
            Ok(Ty::Result(Box::new(ok), Box::new(err)))
        }

        // References
        (Ty::Ref(a_inner), Ty::Ref(b_inner)) => {
            let inner = unify(a_inner, b_inner, ctx, span)?;
            Ok(Ty::Ref(Box::new(inner)))
        }
        (Ty::RefMut(a_inner), Ty::RefMut(b_inner)) => {
            let inner = unify(a_inner, b_inner, ctx, span)?;
            Ok(Ty::RefMut(Box::new(inner)))
        }

        // Class types
        (
            Ty::Class {
                name: an,
                generic_args: aa,
            },
            Ty::Class {
                name: bn,
                generic_args: ba,
            },
        ) => {
            if an != bn {
                return Err(TypeError::mismatch(&a, &b, span));
            }
            if aa.len() != ba.len() {
                return Err(TypeError::mismatch(&a, &b, span));
            }
            let args: Result<Vec<Ty>, TypeError> = aa
                .iter()
                .zip(ba.iter())
                .map(|(x, y)| unify(x, y, ctx, span))
                .collect();
            Ok(Ty::Class {
                name: an.clone(),
                generic_args: args?,
            })
        }

        // Struct types
        (
            Ty::Struct {
                name: an,
                generic_args: aa,
            },
            Ty::Struct {
                name: bn,
                generic_args: ba,
            },
        ) => {
            if an != bn {
                return Err(TypeError::mismatch(&a, &b, span));
            }
            if aa.len() != ba.len() {
                return Err(TypeError::mismatch(&a, &b, span));
            }
            let args: Result<Vec<Ty>, TypeError> = aa
                .iter()
                .zip(ba.iter())
                .map(|(x, y)| unify(x, y, ctx, span))
                .collect();
            Ok(Ty::Struct {
                name: an.clone(),
                generic_args: args?,
            })
        }

        // Enum types
        (
            Ty::Enum {
                name: an,
                generic_args: aa,
            },
            Ty::Enum {
                name: bn,
                generic_args: ba,
            },
        ) => {
            if an != bn {
                return Err(TypeError::mismatch(&a, &b, span));
            }
            if aa.len() != ba.len() {
                return Err(TypeError::mismatch(&a, &b, span));
            }
            let args: Result<Vec<Ty>, TypeError> = aa
                .iter()
                .zip(ba.iter())
                .map(|(x, y)| unify(x, y, ctx, span))
                .collect();
            Ok(Ty::Enum {
                name: an.clone(),
                generic_args: args?,
            })
        }

        // Function types
        (
            Ty::Fn {
                params: ap,
                ret: ar,
            },
            Ty::Fn {
                params: bp,
                ret: br,
            },
        ) => {
            if ap.len() != bp.len() {
                return Err(TypeError::mismatch(&a, &b, span));
            }
            let params: Result<Vec<Ty>, TypeError> = ap
                .iter()
                .zip(bp.iter())
                .map(|(x, y)| unify(x, y, ctx, span))
                .collect();
            let ret = unify(ar, br, ctx, span)?;
            Ok(Ty::Fn {
                params: params?,
                ret: Box::new(ret),
            })
        }

        // TypeParam: unify if same name
        (Ty::TypeParam { name: an, .. }, Ty::TypeParam { name: bn, .. }) if an == bn => Ok(a),

        // TypeParam unifies with any concrete type (the concrete type wins).
        // In a generic context, T can be instantiated to any type that satisfies bounds.
        // Bound checking is done elsewhere; here we just allow structural unification.
        (Ty::TypeParam { .. }, _) => Ok(b),
        (_, Ty::TypeParam { .. }) => Ok(a),

        // Reference coercion: &T can unify with T (auto-deref) and
        // T can unify with &T (auto-ref). This handles cases like
        // Vec[&&T] vs Vec[&T] or Vec[&T] vs Vec[T].
        (Ty::Ref(inner_a), _) => match unify(inner_a, &b, ctx, span) {
            Ok(_) => Ok(a),
            Err(_) => Err(TypeError::mismatch(&a, &b, span)),
        },
        (_, Ty::Ref(inner_b)) => match unify(&a, inner_b, ctx, span) {
            Ok(_) => Ok(b),
            Err(_) => Err(TypeError::mismatch(&a, &b, span)),
        },

        // No match
        _ => Err(TypeError::mismatch(&a, &b, span)),
    }
}

/// Check if type `a` can be coerced to type `b` (weaker than unification).
/// This is used for assignment checking where certain implicit conversions
/// are allowed (e.g., &mut T → &T, integer widening).
pub fn can_coerce(from: &Ty, to: &Ty, ctx: &TypeContext) -> bool {
    let from = ctx.resolve(from);
    let to = ctx.resolve(to);

    if from == to {
        return true;
    }

    match (&from, &to) {
        // Inference variables can always coerce
        (Ty::Infer(_), _) | (_, Ty::Infer(_)) => true,

        // Never coerces to anything
        (Ty::Never, _) => true,

        // Error coerces to anything (error recovery)
        (Ty::Error, _) | (_, Ty::Error) => true,

        // &mut T → &T (always allowed)
        (Ty::RefMut(inner_from), Ty::Ref(inner_to)) => can_coerce(inner_from, inner_to, ctx),

        // &String → &str (string deref coercion)
        (Ty::Ref(inner), Ty::Str) => matches!(&**inner, Ty::String),

        // Integer widening: smaller → larger
        (from_ty, to_ty) if from_ty.is_integer() && to_ty.is_integer() => {
            match (from_ty.bit_width(), to_ty.bit_width()) {
                (Some(fw), Some(tw)) => {
                    // Same sign family and wider
                    fw <= tw && from_ty.is_signed_integer() == to_ty.is_signed_integer()
                }
                _ => false,
            }
        }

        // Float widening: Float32 → Float64/Float
        (Ty::Float32, Ty::Float64) | (Ty::Float32, Ty::Float) => true,

        // Int literal → Float (special case for `let x: Float = 42`)
        (Ty::Int, Ty::Float) | (Ty::Int, Ty::Float64) | (Ty::Int, Ty::Float32) => true,

        // Option covariance: Option[&Child] → Option[&Parent]
        (Ty::Option(a_inner), Ty::Option(b_inner)) => can_coerce(a_inner, b_inner, ctx),

        _ => false,
    }
}
