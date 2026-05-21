//! Binary / unary operator inference and concurrency-bound checks.
//!
//! Three methods on `InferenceEngine`:
//! - `check_concurrency_bounds` — enforces `Send` / `Sync` mixin bounds on
//!   any concrete type that flows into a type-parameter / `some Mixin` /
//!   `any Mixin` position.
//! - `infer_binop` — synthesises the result type of every `BinOp` variant.
//! - `infer_unaryop` — synthesises the result type of `Neg` / `Not` /
//!   `Deref`.

use crate::diagnostics::Diagnostic;
use crate::hir::types::Ty;
use crate::lexer::token::Span;
use crate::parser::ast::{BinOp, UnaryOp};

use super::super::unify::unify;
use super::InferenceEngine;

impl<'a> InferenceEngine<'a> {
    pub(super) fn check_concurrency_bounds(&mut self, expected: &Ty, found: &Ty, span: &Span) {
        let expected = self.ctx.resolve(expected);
        let found = self.ctx.resolve(found);

        let bounds: &[crate::hir::types::MixinRef] = match &expected {
            Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                bounds.as_slice()
            }
            _ => return,
        };

        for bound in bounds {
            match bound.name.as_str() {
                "Send" if !found.is_send_with(self.symbols) => {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!("type `{}` does not satisfy `Send`", found),
                        span.clone(),
                        "E1011",
                    ));
                }
                "Sync" if !found.is_sync_with(self.symbols) => {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!("type `{}` does not satisfy `Sync`", found),
                        span.clone(),
                        "E1012",
                    ));
                }
                _ => {}
            }
        }
    }

    // ─── Binary Operation Type Inference ────────────────────────────

    pub(super) fn infer_binop(&mut self, op: BinOp, left: &Ty, right: &Ty, span: &Span) -> Ty {
        match op {
            // Arithmetic: both sides must be numeric, result is same type
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                // String concatenation: any combination of `String`/`&str`
                // on both sides produces a newly-allocated `String`. This
                // has to be checked before the numeric path because `&str`
                // (Ty::Str) is not numeric but will happily unify with
                // itself in the generic fallback below, yielding the wrong
                // type.
                if op == BinOp::Add
                    && matches!(*left, Ty::String | Ty::Str)
                    && matches!(*right, Ty::String | Ty::Str)
                {
                    return Ty::String;
                }

                // Phase 2 stdlib (#06.5 T4): Duration / Instant operator
                // overloads. The mir/lower/expr/binops.rs special-case
                // routes the actual call to `riven_duration_add` /
                // `riven_duration_sub` / `riven_instant_sub`; here we
                // only need typeck to assign the right result Ty so
                // downstream `.as_nanos()` / `.as_secs()` method
                // resolution can find the Duration instance methods.
                //
                // `Duration + Duration` -> Duration
                // `Duration - Duration` -> Duration (saturating in runtime)
                // `Instant - Instant`   -> Duration (duration_since semantics)
                fn class_named(ty: &Ty, target: &str) -> bool {
                    match ty {
                        Ty::Class { name, .. } => name == target,
                        Ty::Ref(inner)
                        | Ty::RefMut(inner)
                        | Ty::RefLifetime(_, inner)
                        | Ty::RefMutLifetime(_, inner) => class_named(inner, target),
                        _ => false,
                    }
                }
                let duration_ty = || Ty::Class {
                    name: "Duration".to_string(),
                    generic_args: vec![],
                };
                if matches!(op, BinOp::Add | BinOp::Sub)
                    && class_named(left, "Duration")
                    && class_named(right, "Duration")
                {
                    return duration_ty();
                }
                if op == BinOp::Sub && class_named(left, "Instant") && class_named(right, "Instant")
                {
                    return duration_ty();
                }

                if left.is_numeric() && right.is_numeric() {
                    // Unify the two sides
                    match unify(left, right, self.ctx, span) {
                        Ok(unified) => unified,
                        Err(_) => {
                            // String + String = String (concatenation)
                            if *left == Ty::String && *right == Ty::String && op == BinOp::Add {
                                return Ty::String;
                            }
                            left.clone()
                        }
                    }
                } else if *left == Ty::String && op == BinOp::Add {
                    Ty::String
                } else {
                    match unify(left, right, self.ctx, span) {
                        Ok(unified) => unified,
                        Err(_) => left.clone(),
                    }
                }
            }

            // Comparison: both sides same type, result is Bool
            BinOp::Eq | BinOp::NotEq | BinOp::Lt | BinOp::Gt | BinOp::LtEq | BinOp::GtEq => {
                let _ = unify(left, right, self.ctx, span);
                Ty::Bool
            }

            // Logical: both sides Bool, result is Bool
            BinOp::And | BinOp::Or => Ty::Bool,

            // Bitwise: both sides integer, result is same type
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => {
                match unify(left, right, self.ctx, span) {
                    Ok(unified) => unified,
                    Err(_) => left.clone(),
                }
            }
        }
    }

    pub(super) fn infer_unaryop(&mut self, op: UnaryOp, operand: &Ty, _span: &Span) -> Ty {
        match op {
            UnaryOp::Neg => operand.clone(),
            UnaryOp::Not => {
                if *operand == Ty::Bool {
                    Ty::Bool
                } else {
                    operand.clone() // bitwise not
                }
            }
            UnaryOp::Deref => {
                // `*x` strips one level of reference.
                let resolved = self.ctx.resolve(operand);
                match resolved {
                    crate::hir::types::Ty::Ref(inner) | crate::hir::types::Ty::RefMut(inner) => {
                        *inner
                    }
                    // Not a reference — pass through (auto-deref is a no-op).
                    other => other,
                }
            }
        }
    }
}
