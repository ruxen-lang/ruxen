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

        // Phase 2 #06.9: closure-literal → dyn-Fn coercion at unification
        // time. A `Ty::Fn { params, ret }` (the static type of every
        // closure literal and named function value) unifies with the
        // dyn-erased `Ty::AnyMixin([MixinRef { name: "Fn",
        // generic_args: [Ty::Fn { params, ret }] }])` when the inner
        // signature unifies. The AnyMixin shape wins (the dyn-erased
        // type is the more abstract target). This is what lets
        //
        //   let h: any Fn(Int) -> Int = { |n: Int| n + 1 }
        //   var hs: Array[any Fn(Int) -> Int] = Array.new
        //   hs.push({ |n: Int| n + 10 })
        //   def make_adder(n: Int) -> any Fn(Int) -> Int = …
        //
        // typecheck. The layout invariant is documented in
        // `compiler/ruxen_core/tests/closures_dyn_dispatch.rs`: both
        // sides are physically a 16-byte `(fn_ptr, captures_ptr)`
        // pair, so the runtime representation is identical and the
        // method-call lowerer can emit the same indirect call against
        // slot 0 / slot 1. See `mir/lower/expr/method_call.rs`
        // `is_fn_call` for the matching dispatch-side change.
        (Ty::Fn { .. }, Ty::AnyMixin(bounds)) => {
            if let Some(inner) = fn_bound_signature(bounds) {
                let _ = unify(&a, &inner, ctx, span)?;
                Ok(b)
            } else {
                Err(TypeError::mismatch(&a, &b, span))
            }
        }
        (Ty::AnyMixin(bounds), Ty::Fn { .. }) => {
            if let Some(inner) = fn_bound_signature(bounds) {
                let _ = unify(&inner, &b, ctx, span)?;
                Ok(a)
            } else {
                Err(TypeError::mismatch(&a, &b, span))
            }
        }

        // TODO (quality review §1.3 — soundness gap):
        //
        // The two arms below let `&T` unify with `T` at the TOP level,
        // which is unsound: `&Int` should NOT silently unify with `Int`.
        // The comment claims they exist to handle `Vec[&T]` vs `Vec[T]`
        // element auto-deref — but that case is already handled by the
        // structural Ty::Array(elem) arm above, which recurses
        // correctly.
        //
        // Attempted removal of these arms (this session) surfaces real
        // iter-dispatch bugs in `sample_program.rx`:
        //   - `data.split("|").to_vec` resolves to `Array[&str]` but
        //     callers expected `Array[&&str]`
        //   - `tasks.iter.partition` returns one ref-level less than
        //     declared
        //   - `list.repo.all.iter.to_vec` resolves to `Array[Todo]` not
        //     `Array[&Todo]`
        //
        // i.e. the auto-deref is masking an iter return-type bug. Fix
        // requires landing the iter ref-level correction in lockstep,
        // which is beyond the scope of this commit. Leaving the
        // unsound auto-deref in place but documented as a tracked gap.
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

/// If `bounds` looks like the dyn-erased spelling of a single `Fn` /
/// `FnMut` / `FnOnce` trait object — i.e. `[MixinRef { name: "Fn"|…,
/// generic_args: [Ty::Fn { … }] }]` — return that inner function
/// type. Otherwise None.
///
/// The parser stashes the closure signature as a single
/// `TypeExpr::Function` inside the bound's `generic_args` (see
/// `parser/types.rs::parse_single_trait_bound` Fn-trait sugar arm), so
/// the inner shape is exactly `Ty::Fn { params, ret }`.
///
/// Used by the unification cases above so a concrete closure /
/// function value flows into a `any Fn(...)` slot without the
/// caller having to write an explicit conversion.
fn fn_bound_signature(bounds: &[crate::hir::types::MixinRef]) -> Option<Ty> {
    if bounds.len() != 1 {
        return None;
    }
    let b = &bounds[0];
    if !matches!(b.name.as_str(), "Fn" | "FnMut" | "FnOnce") {
        return None;
    }
    let inner = b.generic_args.first()?;
    match inner {
        Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. } => Some(inner.clone()),
        _ => None,
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

        // (The `&String → &str` deref-coercion arm was removed with the `&str`
        // type — there is one string borrow type now, `&String`.)

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
