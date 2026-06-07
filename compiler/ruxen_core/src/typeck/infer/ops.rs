//! Binary / unary operator inference and declared-bound checks.
//!
//! Methods on `InferenceEngine`:
//! - `check_declared_bounds` — enforces declared mixin bounds on a
//!   parameter-typed position (`TypeParam` / `some Mixin` / `any Mixin`),
//!   the param-typed enforcement seam. Generalised from the historical
//!   `check_concurrency_bounds` (Send/Sync-only): now dispatches every
//!   declared bound through `MixinResolver::check_satisfaction`, so a
//!   `[T: Bound]` on a parameter rejects an arg that doesn't satisfy
//!   `Bound`. Send/Sync keep their dedicated codes (E1011/E1012); any
//!   other unsatisfied bound surfaces as the general E0277.
//! - `check_generic_param_bounds` — the SECOND enforcement seam: after
//!   `harvest_and_subst_generics` binds `{T → concrete}`, checks each
//!   harvested generic param's *declared* bounds against the concrete
//!   binding. Owner-aware codes (`Mutex[T: Send]` → E1101, …) flow through
//!   `bound_diagnostic_code`.
//! - `infer_binop` — synthesises the result type of every `BinOp` variant.
//! - `infer_unaryop` — synthesises the result type of `Neg` / `Not` /
//!   `Deref`.
//!
//! **Zero-regression rule:** only params/generics that *declare* a bound
//! are checked. An empty-bounds generic is skipped entirely — unbounded
//! generic calls stay unchecked, protecting the whole existing surface.

use crate::diagnostics::Diagnostic;
use crate::hir::types::{MixinRef, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::{BinOp, UnaryOp};
use crate::typeck::mixins::MixinSatisfaction;

use super::super::unify::unify;
use super::InferenceEngine;

/// The diagnostic code a declared bound emits when unsatisfied.
///
/// `owner` is the class / method whose generic param carries the bound
/// (`None` at the param-typed seam, where the bound sits on a `some`/`any`
/// mixin or an inline `TypeParam` with no owning declaration in scope).
///
/// **This is the preserved-code bridge** for Feature B. The whole reason it
/// exists: the SAME `Send` bound must report E1011 on a free function
/// parameter but E1101 when it sits on `class Mutex[T: Send]`, and E1102 on
/// `Arc`/`SharedSync`. The code is therefore a function of *where the bound
/// is declared*, which the `.rx` surface does not (yet) carry — so the small
/// owner→code table lives here. It is deliberately the ONLY place a stdlib
/// class name maps to a diagnostic code in the bound-checker, and it
/// REPLACES (does not add to) the per-arm hardcoding the `concurrency.rs`
/// E1101/E1102 / `collections.rs` E0700 arms used to do. When a future phase
/// gives the `.rx` bound syntax a code annotation, this table dissolves.
fn bound_diagnostic_code(owner: Option<&str>, bound_name: &str) -> &'static str {
    match (owner, bound_name) {
        (Some("Mutex"), "Send") => "E1101",
        (Some("Arc"), "Send") | (Some("SharedSync"), "Send") => "E1102",
        (_, "Send") => "E1011",
        (_, "Sync") => "E1012",
        // `def sum -> T where T: Add` and any other numeric/element `Add` bound.
        (_, "Add") => "E0700",
        // General unsatisfied-bound code for every other mixin bound — in
        // the owned mixin/include band (E1011-E1099), not Rust's E0277.
        _ => "E1015",
    }
}

impl<'a> InferenceEngine<'a> {
    /// Param-typed enforcement seam. `expected` is a parameter / binding /
    /// return type; when it is a bounded `TypeParam` / `some`/`any` mixin,
    /// every declared bound is checked against the concrete `found` type.
    pub(super) fn check_declared_bounds(&mut self, expected: &Ty, found: &Ty, span: &Span) {
        let expected = self.ctx.resolve(expected);
        let found = self.ctx.resolve(found);

        let bounds: &[MixinRef] = match &expected {
            Ty::TypeParam { bounds, .. } | Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                bounds.as_slice()
            }
            _ => return,
        };

        // No owner at the param-typed seam — Send/Sync emit E1011/E1012.
        self.check_bounds_against(bounds, &found, None, span);
    }

    /// Generic-param enforcement seam (point b). Given a function/method/
    /// class generic param list and the `{name → concrete}` bindings
    /// harvested from the call's actual arguments, check each param that
    /// *declares* a bound against its concrete binding. Params with empty
    /// bounds, or with no harvested binding, are skipped (zero-regression).
    ///
    /// `owner` is the declaring class/method name, threaded only so the
    /// preserved-code bridge can pick the class-appropriate code.
    pub(super) fn check_generic_param_bounds(
        &mut self,
        generic_params: &[crate::resolve::symbols::GenericParamInfo],
        bindings: &std::collections::HashMap<String, Ty>,
        owner: Option<&str>,
        span: &Span,
    ) {
        for gp in generic_params {
            if gp.bounds.is_empty() {
                continue;
            }
            let Some(concrete) = bindings.get(&gp.name) else {
                continue;
            };
            let concrete = self.ctx.resolve(concrete);
            // An unresolved / error binding can't be meaningfully checked;
            // a still-generic binding (return-only param) carries no
            // concrete to test.
            if concrete.is_infer()
                || concrete.is_error()
                || matches!(concrete, Ty::TypeParam { .. })
            {
                continue;
            }
            self.check_bounds_against(&gp.bounds, &concrete, owner, span);
        }
    }

    /// Shared core: check `found` against each `MixinRef` in `bounds`,
    /// emitting the owner-appropriate diagnostic for every unsatisfied
    /// bound. `Send`/`Sync` retain their fast structural predicates (kept
    /// in lockstep with `check_satisfaction`, which special-cases them);
    /// every other bound dispatches through `check_satisfaction` with
    /// `require_nominal = false` (static-dispatch / structural acceptance,
    /// the semantics a `[T: Bound]` generic call site has).
    fn check_bounds_against(
        &mut self,
        bounds: &[MixinRef],
        found: &Ty,
        owner: Option<&str>,
        span: &Span,
    ) {
        // Unresolved / error types are unprovable — never flag them.
        if found.is_infer() || found.is_error() {
            return;
        }
        for bound in bounds {
            let satisfied = match bound.name.as_str() {
                // Send/Sync keep their exact historical predicate (which
                // self-satisfies a `TypeParam` that carries the bound, and
                // rejects one that doesn't) — behaviour-identical to the
                // pre-Feature-B `check_concurrency_bounds`.
                "Send" => found.is_send_with(self.symbols),
                "Sync" => found.is_sync_with(self.symbols),
                // A value of abstract generic type `T` carries no concrete
                // to test against a NON-Send/Sync bound: it is satisfied iff
                // `T` itself declares the same bound (definition-site
                // forwarding), and otherwise checked when `T` is bound — not
                // flagged here. This is what keeps a generic fn returning
                // `T where T: Bound` from flagging its own `T` return, and
                // keeps a `[U: Bound]` forwarded into another `[V: Bound]`
                // call site silent (it was silent pre-Feature-B too).
                _ if matches!(found, Ty::TypeParam { .. }) => {
                    let Ty::TypeParam { bounds: tp, .. } = found else {
                        unreachable!()
                    };
                    tp.iter().any(|b| b.name == bound.name)
                }
                _ => !matches!(
                    self.traits
                        .check_satisfaction(found, bound, self.symbols, false),
                    MixinSatisfaction::Unsatisfied { .. }
                ),
            };
            if satisfied {
                continue;
            }
            let code = bound_diagnostic_code(owner, &bound.name);
            let message = match (owner, bound.name.as_str()) {
                (Some(cls), "Send") => format!(
                    "cannot construct `{cls}[{found}]` — payload type `{found}` is not `Send`. \
                     Add `include Send` to the class if it is safe to share across threads."
                ),
                (_, "Add") => format!(
                    "`sum` requires numeric elements that implement `Add`; \
                     `{found}` is not numeric"
                ),
                _ => format!("type `{found}` does not satisfy `{}`", bound.name),
            };
            self.diagnostics
                .push(Diagnostic::error_with_code(message, span.clone(), code));
        }
    }

    // ─── Operator → method desugar (Task OP, Step 3) ────────────────
    //
    // The operator→method-name map is `BinOp::method_name` (parser/ast.rs)
    // — the SINGLE source shared with MIR's `lower_binops`. Only the
    // MIGRATED families (arithmetic `+ - * / %`, bitwise `& | ^ << >>`)
    // return a name; comparison/equality/logical/regex-match return `None`
    // (they keep their existing paths + the later `Comparable` increment).

    /// True when `ty` is a NOMINAL receiver (user/stdlib class, struct, or
    /// enum) — the receivers whose operators are real overridable `.rx`
    /// methods (Duration, user operator classes). Machine primitives
    /// (`Int`/`Float`/`Bool`/widths), `String`/`&str`, and the builtin
    /// collection heads (`Array`/`Set`/`Map`) are NOT nominal here: they
    /// keep their machine-floor / special-case binop lowering. Peels refs.
    pub(super) fn is_nominal_operator_receiver(ty: &Ty) -> bool {
        match ty {
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => Self::is_nominal_operator_receiver(inner),
            Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } => true,
            _ => false,
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

                // Duration / Instant arithmetic MIGRATED (Task OP, Step 3)
                // to overridable `.rx` `def +`/`def -` methods. A nominal
                // receiver routes to the method in the `BinaryOp` handler
                // (`expr.rs`) BEFORE `infer_binop` is reached, so there is
                // no Duration/Instant arm here anymore — the result type
                // comes from the method's declared return.

                if left.is_numeric() && right.is_numeric() {
                    // Unify the two sides
                    match unify(left, right, self.ctx, span) {
                        Ok(unified) => unified,
                        Err(_) => {
                            // Ruxen has no implicit numeric coercion in
                            // arithmetic: the operand types must already
                            // match. A mismatch (e.g. `Int - Float`, or
                            // `Int + Int64`) must surface here as a clean
                            // diagnostic — if it slips through, codegen
                            // selects the instruction from the LHS type
                            // (`isub.i64`) and feeds it the un-coerced RHS
                            // (`f64`), tripping the Cranelift verifier with
                            // an opaque internal error. Make both operands
                            // share a type (e.g. annotate `a: Float` and
                            // use `1.0`, or keep both sides integers).
                            let op_sym = match op {
                                BinOp::Add => "+",
                                BinOp::Sub => "-",
                                BinOp::Mul => "*",
                                BinOp::Div => "/",
                                BinOp::Mod => "%",
                                _ => "<op>",
                            };
                            self.diagnostics.push(Diagnostic::error_with_code(
                                format!(
                                    "binary operator `{op_sym}` cannot be applied to \
                                     mismatched numeric types `{left}` and `{right}`; \
                                     convert one side explicitly so both operands share a type",
                                ),
                                span.clone(),
                                "E0707",
                            ));
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

            // Regex match: `String ~= Regex` -> `Bool`. LHS must be a
            // String / &String / Str; RHS must be a `Regex` class.
            // Mismatch on either side emits E1702.
            BinOp::MatchOp => {
                fn is_string_like(ty: &Ty) -> bool {
                    match ty {
                        Ty::String | Ty::Str => true,
                        Ty::Ref(inner)
                        | Ty::RefMut(inner)
                        | Ty::RefLifetime(_, inner)
                        | Ty::RefMutLifetime(_, inner) => is_string_like(inner),
                        _ => false,
                    }
                }
                fn is_regex(ty: &Ty) -> bool {
                    match ty {
                        Ty::Class { name, .. } => name == "Regex",
                        Ty::Ref(inner)
                        | Ty::RefMut(inner)
                        | Ty::RefLifetime(_, inner)
                        | Ty::RefMutLifetime(_, inner) => is_regex(inner),
                        _ => false,
                    }
                }
                // Tolerate unresolved Infer vars: they may not be
                // pinned yet at first inference visit. Emit E1702
                // only when both sides have settled to a concrete
                // (non-Infer, non-Error, non-Never) type AND at
                // least one side is wrong.
                let lhs_resolved = !matches!(left, Ty::Infer(_) | Ty::Error | Ty::Never);
                let rhs_resolved = !matches!(right, Ty::Infer(_) | Ty::Error | Ty::Never);
                if lhs_resolved && rhs_resolved && (!is_string_like(left) || !is_regex(right)) {
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "`~=` operands must be String and Regex, got `{}` and `{}`",
                            left, right
                        ),
                        span.clone(),
                        "E1702",
                    ));
                }
                Ty::Bool
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
