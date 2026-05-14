//! Integration tests pinning the variance rules for built-in type
//! constructors (P0.15).
//!
//! Variance is the rule that decides whether a coercion through a generic
//! constructor is allowed. The Riven compiler currently encodes these
//! rules in [`riven_core::typeck::coerce::try_coerce`] and the matching
//! [`riven_core::typeck::unify::can_coerce`] predicate, but as of the
//! P0.15 audit two of the three rules existed only as a comment near
//! `coerce.rs:108-109`:
//!
//! ```text
//! // Vec, Hash, Set — invariant (no coercion)
//! // &mut T — invariant (no coercion to &mut U)
//! ```
//!
//! A future refactor of `coerce.rs` could silently regress those rules
//! and nothing in the test suite would notice. This file pins the three
//! P0.15 rules so a regression turns red:
//!
//! 1. `&mut T` is **invariant** in `T` — no implicit coercion through it.
//! 2. `Vec[T]` is **invariant** in `T` (matches Rust's `Vec<T>`).
//! 3. `Option[T]` is **covariant** in `T`.
//!
//! # Why these tests poke `can_coerce` directly
//!
//! The Riven surface syntax does not (yet) expose explicit subtype
//! lifetimes or other constructions that would let us write a plain
//! `.rvn` source whose ill-typedness depends *only* on a variance rule.
//! In particular:
//!
//! - There is no way to spell two distinct types `T1`, `T2` with a
//!   subtype relationship between them other than reference-to-class
//!   inheritance, and class inheritance only fires through `&Child →
//!   &Parent` (see `coerce.rs:97-106`).
//! - The unification fallback at `unify.rs:198-209` accepts an extra
//!   layer of `&` on either side, which means surface-level negative
//!   tests against Vec/Option through `&` get hidden behind that rule
//!   rather than the variance rule we want to exercise.
//!
//! Calling `can_coerce` directly with hand-built `Ty` values sidesteps
//! both issues and exercises *only* the constructor's variance rule.
//! The corresponding positive (compilable) cases live in the e2e
//! fixtures `tests/release-e2e/cases/136_variance_option_covariant.rvn`
//! and `137_variance_array_same_type.rvn`.

use riven_core::hir::context::TypeContext;
use riven_core::hir::types::Ty;
use riven_core::typeck::unify::can_coerce;

// ─── helpers ────────────────────────────────────────────────────────

/// A class-typed value named `n`. Used as a stand-in for an opaque
/// nominal type with no subtype relationship to other classes.
fn class(n: &str) -> Ty {
    Ty::Class {
        name: n.to_string(),
        generic_args: vec![],
    }
}

// ─── Rule 1: `&mut T` is invariant in T ─────────────────────────────

#[test]
fn mut_ref_no_coerce_to_different_inner_type() {
    // `&mut Int` must NOT coerce to `&mut Bool`. Even if some other
    // rule made `Int → Bool` legal (it doesn't, but the rule is what
    // we're pinning), the invariance of `&mut` should reject it.
    let ctx = TypeContext::new();
    let from = Ty::RefMut(Box::new(Ty::Int));
    let to = Ty::RefMut(Box::new(Ty::Bool));
    assert!(
        !can_coerce(&from, &to, &ctx),
        "&mut Int must not coerce to &mut Bool — &mut is invariant in T",
    );
}

#[test]
fn mut_ref_no_coerce_widening_inner() {
    // `&mut Int8` must NOT coerce to `&mut Int64` even though
    // `Int8 → Int64` is a legal integer widening. Mutable references
    // are invariant: widening through them would let a writer store
    // an Int64 into the original Int8 location.
    let ctx = TypeContext::new();
    let from = Ty::RefMut(Box::new(Ty::Int8));
    let to = Ty::RefMut(Box::new(Ty::Int64));
    assert!(
        !can_coerce(&from, &to, &ctx),
        "&mut Int8 must not coerce to &mut Int64 — invariance forbids widening through &mut",
    );
}

#[test]
fn mut_ref_no_coerce_to_different_class() {
    // `&mut Cat` must NOT coerce to `&mut Animal` even when class
    // inheritance would make that legal for a shared `&` reference.
    // `&mut` is invariant; only the *immutable* reference is covariant
    // through inheritance.
    let ctx = TypeContext::new();
    let from = Ty::RefMut(Box::new(class("Cat")));
    let to = Ty::RefMut(Box::new(class("Animal")));
    assert!(
        !can_coerce(&from, &to, &ctx),
        "&mut Cat must not coerce to &mut Animal — &mut is invariant in T",
    );
}

#[test]
fn mut_ref_to_immut_ref_still_works() {
    // Sanity check: invariance of `&mut T` in T must NOT block the
    // separate, well-known coercion `&mut T → &T`. Same `T` on both
    // sides; this is a borrow-strength downgrade, not variance.
    let ctx = TypeContext::new();
    let from = Ty::RefMut(Box::new(Ty::Int));
    let to = Ty::Ref(Box::new(Ty::Int));
    assert!(
        can_coerce(&from, &to, &ctx),
        "&mut Int → &Int must still be allowed",
    );
}

// ─── Rule 2: `Vec[T]` is invariant in T ─────────────────────────────

#[test]
fn vec_no_coerce_to_different_inner_type() {
    // `Vec[Int]` must NOT coerce to `Vec[Bool]`. The constructor is
    // invariant — the underlying buffer's element type is fixed.
    let ctx = TypeContext::new();
    let from = Ty::Array(Box::new(Ty::Int));
    let to = Ty::Array(Box::new(Ty::Bool));
    assert!(
        !can_coerce(&from, &to, &ctx),
        "Vec[Int] must not coerce to Vec[Bool] — Vec is invariant in T",
    );
}

#[test]
fn vec_no_coerce_widening_inner() {
    // `Vec[Int8]` must NOT coerce to `Vec[Int64]` even though
    // `Int8 → Int64` is a legal integer widening. Coercing through
    // Vec would let a writer push Int64 values into a buffer whose
    // memory layout assumes Int8 elements.
    let ctx = TypeContext::new();
    let from = Ty::Array(Box::new(Ty::Int8));
    let to = Ty::Array(Box::new(Ty::Int64));
    assert!(
        !can_coerce(&from, &to, &ctx),
        "Vec[Int8] must not coerce to Vec[Int64] — invariance forbids widening through Vec",
    );
}

#[test]
fn vec_no_coerce_through_inheritance() {
    // `Vec[&Cat]` must NOT coerce to `Vec[&Animal]` even though
    // `&Cat → &Animal` is legal. Vec invariance forbids coercion of
    // its element type through the constructor.
    let ctx = TypeContext::new();
    let from = Ty::Array(Box::new(Ty::Ref(Box::new(class("Cat")))));
    let to = Ty::Array(Box::new(Ty::Ref(Box::new(class("Animal")))));
    assert!(
        !can_coerce(&from, &to, &ctx),
        "Vec[&Cat] must not coerce to Vec[&Animal] — Vec is invariant in T",
    );
}

#[test]
fn vec_same_type_works() {
    // Sanity: `Vec[Int] → Vec[Int]` succeeds. This pins the legal
    // direction so a regression that broke same-type identity would
    // also fail this test — the e2e fixture
    // `137_variance_array_same_type.rvn` covers the same case at the
    // surface-syntax level.
    let ctx = TypeContext::new();
    let v = Ty::Array(Box::new(Ty::Int));
    assert!(
        can_coerce(&v, &v, &ctx),
        "Vec[Int] → Vec[Int] must succeed (identity)",
    );
}

// ─── Rule 3: `Option[T]` is covariant in T ──────────────────────────

#[test]
fn option_covariant_through_mut_to_immut_ref() {
    // `Option[&mut Int] → Option[&Int]` is allowed: the inner
    // coercion `&mut Int → &Int` is legal, and Option is covariant
    // in T, so it propagates through the constructor.
    let ctx = TypeContext::new();
    let from = Ty::Option(Box::new(Ty::RefMut(Box::new(Ty::Int))));
    let to = Ty::Option(Box::new(Ty::Ref(Box::new(Ty::Int))));
    assert!(
        can_coerce(&from, &to, &ctx),
        "Option[&mut Int] → Option[&Int] must work — Option is covariant in T",
    );
}

#[test]
fn option_covariant_through_integer_widening() {
    // `Option[Int8] → Option[Int64]` is allowed: integer widening
    // is a legal inner coercion and Option is covariant.
    let ctx = TypeContext::new();
    let from = Ty::Option(Box::new(Ty::Int8));
    let to = Ty::Option(Box::new(Ty::Int64));
    assert!(
        can_coerce(&from, &to, &ctx),
        "Option[Int8] → Option[Int64] must work — Option is covariant in T",
    );
}

#[test]
fn option_no_coerce_when_inner_incompatible() {
    // Counter-check: covariance does not invent coercions. If the
    // inner types are unrelated, `Option[T1] → Option[T2]` fails.
    // Ensures covariance is delegated to the inner `can_coerce` call,
    // not a blanket "any Option is any Option".
    let ctx = TypeContext::new();
    let from = Ty::Option(Box::new(Ty::Int));
    let to = Ty::Option(Box::new(Ty::Bool));
    assert!(
        !can_coerce(&from, &to, &ctx),
        "Option[Int] must not coerce to Option[Bool] — covariance still requires inner coercion",
    );
}

// ─── Pin: variance machinery is reachable from typeck ───────────────

#[test]
fn variance_module_is_wired_into_typeck() {
    // This test exists purely as a structural pin: if a refactor
    // accidentally removes the `coerce` module from typeck, or
    // breaks `can_coerce`'s signature so it no longer takes a
    // `TypeContext`, the rest of this file will fail to compile and
    // this test will fail to link. That's the point — the variance
    // rules listed at `coerce.rs:108-109` are kept aspirational by
    // a comment alone, so we pin existence at compile time.
    let ctx = TypeContext::new();
    assert!(can_coerce(&Ty::Int, &Ty::Int, &ctx));
}
