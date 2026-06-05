# Phase 1 — AST `Visit`/`VisitMut` + `Ty::map_inner` (+ 3 bug fixes) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended)
> or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`)
> syntax for tracking. This is Phase 1 of `2026-06-04-thermonuke-master.md`.

**Goal:** Introduce one exhaustive traversal primitive for the `Ty` enum and one for the parser AST,
then migrate the three hand-rolled walkers that currently carry latent correctness bugs onto them —
fixing all three bugs *by construction*.

**Architecture:** Two independent primitives. `Ty::map_inner` (a structural fold on `hir::types::Ty`)
covers every nested `Ty` child exhaustively. `parser::visit::{Visit, VisitMut}` provide exhaustive
`walk_*` super-functions over the parser AST; consumers override only the nodes they care about and
decide per-node whether to recurse (the rustc visitor pattern). Migrations are behaviour-preserving
except where they close a named bug; every migration is guarded by a characterization test plus a
bug-exposing test.

**Tech Stack:** Rust 1.91, `cargo test -p ruxen_core`. No new dependencies.

> **Per-task direction-check (maintainer-mandated):** after the commit step of EVERY task below, run
> the `thermonuke` skill scoped to that task's diff: invoke it with arg
> `git diff HEAD~1..HEAD` and confirm (a) lines moved in the intended direction, (b) no new `_ =>`
> catch-all in any traversal/table, (c) no new god-function/special-case in a shared path, (d) the
> task's structural goal was met (a hand-rolled walker was *removed*, not added). If it flags drift,
> STOP and surface it. The full multi-agent sweep runs in Task 6. Each task's checkbox list ends with
> a `- [ ] Direction-check (/thermonuke on this task's diff)` step; it is implied here to avoid
> repeating it five times, but executors MUST perform it.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `compiler/ruxen_core/src/hir/types.rs` | `Ty` enum + (new) `map_inner`/`peel_refs` | Modify (append impl) |
| `compiler/ruxen_core/src/parser/visit.rs` | `Visit` + `VisitMut` traits and `walk_*` superfns over the AST | **Create** |
| `compiler/ruxen_core/src/parser/mod.rs` | parser module tree | Modify (add `pub mod visit;`) |
| `compiler/ruxen_core/src/typeck/infer/collect.rs` | `subst_ty` (bug #3) | Modify (`subst_ty` body → `map_inner`) |
| `compiler/ruxen_core/src/async_lowering/mod.rs` | await-scan (bug #1) + block_on-rewrite (bug #2) | Modify (2 walkers → Visit/VisitMut) |

**Module placement rationale:** `Ty::map_inner` lives next to the `Ty` enum it folds (one file owns the
exhaustiveness obligation). `Visit`/`VisitMut` live in `parser/` because they traverse parser-AST nodes
(`Expr`, `Statement`, `Block`, `Pattern`, `TypeExpr`) defined in `parser/ast.rs`.

---

## Task 1: `Ty::map_inner` — exhaustive structural fold on `Ty`

**Files:**
- Modify: `compiler/ruxen_core/src/hir/types.rs` (append an `impl Ty` block after the enum, ~line 399)
- Test: inline `#[cfg(test)] mod map_inner_tests` in the same file

- [ ] **Step 1: Write the failing test**

Append to `compiler/ruxen_core/src/hir/types.rs`:

```rust
#[cfg(test)]
mod map_inner_tests {
    use super::*;

    // A fold that renames every TypeParam "T" to Int, recursing everywhere.
    fn t_to_int(ty: &Ty) -> Ty {
        match ty {
            Ty::TypeParam { name, .. } if name == "T" => Ty::Int,
            other => other.clone().map_inner(&mut |c| t_to_int(c)),
        }
    }

    #[test]
    fn substitutes_through_explicit_lifetime_refs() {
        // &'a T  — the variant subst_ty historically MISSED (bug #3)
        let ty = Ty::RefLifetime("a".into(), Box::new(Ty::TypeParam { name: "T".into(), bounds: vec![] }));
        assert_eq!(t_to_int(&ty), Ty::RefLifetime("a".into(), Box::new(Ty::Int)));
    }

    #[test]
    fn substitutes_through_map_and_set() {
        // Map[T, T] and Set[T] — also historically missed by subst_ty.
        let tp = || Ty::TypeParam { name: "T".into(), bounds: vec![] };
        let m = Ty::Map(Box::new(tp()), Box::new(tp()));
        assert_eq!(t_to_int(&m), Ty::Map(Box::new(Ty::Int), Box::new(Ty::Int)));
        let s = Ty::Set(Box::new(tp()));
        assert_eq!(t_to_int(&s), Ty::Set(Box::new(Ty::Int)));
    }

    #[test]
    fn leaves_unrelated_leaves_untouched() {
        assert_eq!(t_to_int(&Ty::Int), Ty::Int);
        assert_eq!(t_to_int(&Ty::String), Ty::String);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ruxen_core --lib hir::types::map_inner_tests 2>&1 | tee tmp/test-cache/phase1-task1-red.log`
Expected: FAIL — `no method named map_inner found for enum Ty`.

- [ ] **Step 3: Write the implementation**

Append (before the test module) to `compiler/ruxen_core/src/hir/types.rs`:

```rust
impl Ty {
    /// Apply `f` to every directly-nested `Ty` child, rebuilding `self` with the
    /// results. EXHAUSTIVE: every variant is matched explicitly — there is no
    /// `_ =>` arm, so adding a new `Ty` variant is a compile error here until it
    /// is handled. This is the single structural-recursion primitive for `Ty`;
    /// folds like `subst_ty` are written as `match { interesting arms; other =>
    /// other.map_inner(f) }`.
    pub fn map_inner(self, f: &mut impl FnMut(&Ty) -> Ty) -> Ty {
        match self {
            // Leaves — no nested Ty.
            Ty::Int | Ty::Int8 | Ty::Int16 | Ty::Int32 | Ty::Int64
            | Ty::UInt | Ty::UInt8 | Ty::UInt16 | Ty::UInt32 | Ty::UInt64
            | Ty::ISize | Ty::USize | Ty::Float | Ty::Float32 | Ty::Float64
            | Ty::Bool | Ty::Char | Ty::Unit | Ty::Never | Ty::String | Ty::Str
            | Ty::ConstArg(_) | Ty::SomeMixin(_) | Ty::AnyMixin(_)
            | Ty::Infer(_) | Ty::TypeParam { .. }
            | Ty::RawPtrVoid | Ty::RawPtrMutVoid | Ty::Error => self,

            // Single-child wrappers.
            Ty::Array(t) => Ty::Array(Box::new(f(&t))),
            Ty::Set(t) => Ty::Set(Box::new(f(&t))),
            Ty::Option(t) => Ty::Option(Box::new(f(&t))),
            Ty::Ref(t) => Ty::Ref(Box::new(f(&t))),
            Ty::RefMut(t) => Ty::RefMut(Box::new(f(&t))),
            Ty::RawPtr(t) => Ty::RawPtr(Box::new(f(&t))),
            Ty::RawPtrMut(t) => Ty::RawPtrMut(Box::new(f(&t))),

            // Named-lifetime refs (bug #3: these were skipped by subst_ty).
            Ty::RefLifetime(l, t) => Ty::RefLifetime(l, Box::new(f(&t))),
            Ty::RefMutLifetime(l, t) => Ty::RefMutLifetime(l, Box::new(f(&t))),

            // Two-child.
            Ty::Map(k, v) => Ty::Map(Box::new(f(&k)), Box::new(f(&v))),
            Ty::Result(ok, err) => Ty::Result(Box::new(f(&ok)), Box::new(f(&err))),

            // Const-sized array: fold the element, keep the const expr.
            Ty::FixedArray(t, n) => Ty::FixedArray(Box::new(f(&t)), n),

            // Sequences of children.
            Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| f(t)).collect()),

            // Nominal types with generic args.
            Ty::Class { name, generic_args } => Ty::Class {
                name,
                generic_args: generic_args.iter().map(|a| f(a)).collect(),
            },
            Ty::Struct { name, generic_args } => Ty::Struct {
                name,
                generic_args: generic_args.iter().map(|a| f(a)).collect(),
            },
            Ty::Enum { name, generic_args } => Ty::Enum {
                name,
                generic_args: generic_args.iter().map(|a| f(a)).collect(),
            },

            // Function types.
            Ty::Fn { params, ret } => Ty::Fn {
                params: params.iter().map(|p| f(p)).collect(),
                ret: Box::new(f(&ret)),
            },
            Ty::FnMut { params, ret } => Ty::FnMut {
                params: params.iter().map(|p| f(p)).collect(),
                ret: Box::new(f(&ret)),
            },
            Ty::FnOnce { params, ret } => Ty::FnOnce {
                params: params.iter().map(|p| f(p)).collect(),
                ret: Box::new(f(&ret)),
            },

            // Transparent / opaque wrappers — fold the inner.
            Ty::Alias { name, target } => Ty::Alias { name, target: Box::new(f(&target)) },
            Ty::Newtype { name, inner } => Ty::Newtype { name, inner: Box::new(f(&inner)) },
        }
    }

    /// Peel every reference layer (`&`, `&mut`, `&'a`, `&'a mut`) off `self`,
    /// returning the first non-reference inner type. The single canonical
    /// reference-peeler — replaces the several partial hand-rolled peels.
    pub fn peel_refs(&self) -> &Ty {
        match self {
            Ty::Ref(t) | Ty::RefMut(t) | Ty::RefLifetime(_, t) | Ty::RefMutLifetime(_, t) => {
                t.peel_refs()
            }
            other => other,
        }
    }
}
```

> NOTE for the implementer: verify the `ConstExpr` type in `FixedArray(Box<Ty>, ConstExpr)` is `Clone`
> (it is moved into the rebuilt variant via `n`). If `map_inner` needs `&self` instead of `self` at a
> call site, add a `pub fn map_inner_ref(&self, ...)` that clones first; the current consumers
> (`subst_ty`) already own/clone, so the by-value form is correct.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ruxen_core --lib hir::types::map_inner_tests 2>&1 | tee tmp/test-cache/phase1-task1-green.log`
Expected: PASS (3 tests). If a compile error names a missing `Ty` variant, add its arm — that is the
exhaustiveness guarantee working as intended.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/hir/types.rs
git commit -m "feat(hir): add exhaustive Ty::map_inner fold + peel_refs

Single structural-recursion primitive for Ty. Exhaustive match (no _ arm)
so a new variant is a compile error until handled. Foundation for replacing
the partial hand-rolled Ty walkers in typeck (subst_ty et al.).

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 2: Migrate `subst_ty` onto `map_inner` (closes bug #3)

**Files:**
- Modify: `compiler/ruxen_core/src/typeck/infer/collect.rs:798-841` (`subst_ty`)
- Test: `compiler/ruxen_core/tests/regex_typeck.rs` is the existing pin; add a focused unit test here.

- [ ] **Step 1: Write the failing (bug-exposing) test**

Add to the existing inline test module in `collect.rs` (find `#[cfg(test)] mod tests` in that file; if
none, add one at the end):

```rust
#[cfg(test)]
mod subst_ty_tests {
    use super::*;
    use std::collections::HashMap;
    use crate::hir::types::Ty;

    fn subst1(ty: Ty, name: &str, to: Ty) -> Ty {
        let mut m = HashMap::new();
        m.insert(name.to_string(), to);
        InferenceEngine::subst_ty(&ty, &m)
    }

    #[test]
    fn substitutes_through_named_lifetime_ref() {
        // &'a T  ->  &'a Int   (was a no-op before the map_inner migration: bug #3)
        let tp = Ty::TypeParam { name: "T".into(), bounds: vec![] };
        let got = subst1(Ty::RefLifetime("a".into(), Box::new(tp)), "T", Ty::Int);
        assert_eq!(got, Ty::RefLifetime("a".into(), Box::new(Ty::Int)));
    }

    #[test]
    fn substitutes_through_map_value() {
        // Map[String, T] -> Map[String, Int]   (was a no-op before: bug #3)
        let tp = Ty::TypeParam { name: "T".into(), bounds: vec![] };
        let got = subst1(Ty::Map(Box::new(Ty::String), Box::new(tp)), "T", Ty::Int);
        assert_eq!(got, Ty::Map(Box::new(Ty::String), Box::new(Ty::Int)));
    }

    #[test]
    fn preserves_existing_class_generic_substitution() {
        // Characterization: the behaviour subst_ty ALREADY had must not regress.
        let tp = Ty::TypeParam { name: "T".into(), bounds: vec![] };
        let got = subst1(Ty::Class { name: "MutexGuard".into(), generic_args: vec![tp] }, "T", Ty::Int);
        assert_eq!(got, Ty::Class { name: "MutexGuard".into(), generic_args: vec![Ty::Int] });
    }
}
```

- [ ] **Step 2: Run test to verify the two bug tests fail**

Run: `cargo test -p ruxen_core --lib subst_ty_tests 2>&1 | tee tmp/test-cache/phase1-task2-red.log`
Expected: `substitutes_through_named_lifetime_ref` and `substitutes_through_map_value` FAIL (return the
un-substituted input); `preserves_existing_class_generic_substitution` PASSES (proves the test harness
is correct and pins current good behaviour).

- [ ] **Step 3: Replace the `subst_ty` body**

In `compiler/ruxen_core/src/typeck/infer/collect.rs`, replace the whole `subst_ty` function
(lines 798-841) with:

```rust
    /// Substitute every `TypeParam` named in `subst` with its bound type,
    /// recursing through ALL nested `Ty` children via `Ty::map_inner`. The only
    /// special arm is `TypeParam`; everything else delegates to the exhaustive
    /// fold, so reference layers (incl. `&'a`/`&'a mut`), `Map`/`Set`/`FixedArray`,
    /// `Fn`/`Newtype`/`Alias` etc. are all covered automatically. See
    /// docs/specs/types/typed_ffi_returns.spec.md.
    pub(super) fn subst_ty(ty: &Ty, subst: &std::collections::HashMap<String, Ty>) -> Ty {
        match ty {
            Ty::TypeParam { name, .. } => subst.get(name).cloned().unwrap_or_else(|| ty.clone()),
            other => other.clone().map_inner(&mut |child| Self::subst_ty(child, subst)),
        }
    }
```

- [ ] **Step 4: Run tests to verify green**

Run: `cargo test -p ruxen_core --lib subst_ty_tests 2>&1 | tee tmp/test-cache/phase1-task2-green.log`
Expected: PASS (3 tests).
Then the existing pin: `cargo test -p ruxen_core --test regex_typeck 2>&1 | tee tmp/test-cache/phase1-task2-regex.log`
Expected: PASS (5 tests) — proves no regression in the path the current branch diff cares about.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/typeck/infer/collect.rs
git commit -m "fix(typeck): subst_ty now recurses through all Ty children (bug #3)

Rewrite subst_ty over Ty::map_inner. Previously it hand-matched only
Ref/RefMut/Option/Array/Class/Struct/Enum/Result/Tuple, silently passing
through &'a T, &'a mut T, Map, Set, FixedArray, Fn, Newtype, Alias — leaving
type params unsubstituted, which mangled to ?T<n>_method and failed codegen.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 3: `parser::visit::{Visit, VisitMut}` — exhaustive AST traversal

**Files:**
- Create: `compiler/ruxen_core/src/parser/visit.rs`
- Modify: `compiler/ruxen_core/src/parser/mod.rs` (add `pub mod visit;`)
- Test: inline `#[cfg(test)] mod tests` in `visit.rs`

**Design note (load-bearing):** consumers override `visit_expr` and decide whether to call
`walk_expr(self, e)` (recurse) — this is what lets the await-scan treat closures as opaque (override,
don't recurse) while recursing everywhere else. The `walk_*` superfns contain the ONE exhaustive
`ExprKind` match (49 variants, no `_` arm).

- [ ] **Step 1: Write the failing test**

Create `compiler/ruxen_core/src/parser/visit.rs` ending with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ast::*;
    use crate::diagnostics::Span; // adjust import to wherever Span lives

    fn sp() -> Span { Span::default() } // adjust to the real Span constructor

    fn ident(n: &str) -> Expr { Expr { kind: ExprKind::Identifier(n.into()), span: sp() } }
    fn await_of(inner: Expr) -> Expr { Expr { kind: ExprKind::Await(Box::new(inner)), span: sp() } }

    // A visitor that counts Await nodes, recursing through everything via walk_expr.
    struct AwaitCounter { n: usize }
    impl Visit for AwaitCounter {
        fn visit_expr(&mut self, e: &Expr) {
            if matches!(e.kind, ExprKind::Await(_)) { self.n += 1; }
            walk_expr(self, e);
        }
    }

    fn count_awaits(e: &Expr) -> usize {
        let mut c = AwaitCounter { n: 0 };
        c.visit_expr(e);
        c.n
    }

    #[test]
    fn finds_await_inside_enum_variant_args() {
        // Some(x.await) — EnumVariant arg; the OLD hand-rolled scan missed this (bug #1).
        let e = Expr {
            kind: ExprKind::EnumVariant {
                type_path: vec!["Option".into()],
                variant: "Some".into(),
                args: vec![FieldArg { name: None, value: await_of(ident("x")), span: sp() }],
            },
            span: sp(),
        };
        assert_eq!(count_awaits(&e), 1);
    }

    #[test]
    fn finds_await_inside_unsafe_block() {
        // unsafe { x.await }
        let blk = Block {
            statements: vec![Statement::Expression(await_of(ident("x")))],
            span: sp(),
        };
        let e = Expr { kind: ExprKind::UnsafeBlock(blk), span: sp() };
        assert_eq!(count_awaits(&e), 1);
    }
}
```

> The implementer must align `Span` construction and `FieldArg`/`EnumVariant` field names with
> `parser/ast.rs` (verified: `FieldArg { name: Option<String>, value: Expr, span: Span }`,
> `EnumVariant { type_path: Vec<String>, variant: String, args: Vec<FieldArg> }`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p ruxen_core --lib parser::visit 2>&1 | tee tmp/test-cache/phase1-task3-red.log`
Expected: FAIL to compile — `Visit`/`walk_expr` not found.

- [ ] **Step 3: Write the `Visit` trait + `walk_*` superfns**

At the top of `compiler/ruxen_core/src/parser/visit.rs`:

```rust
//! Exhaustive, shared traversal of the parser AST.
//!
//! `Visit` (immutable) and `VisitMut` (mutating) each provide a default method
//! per node type; the free `walk_*` super-functions contain the ONE exhaustive
//! match over `ExprKind`/`Statement`/`Pattern`/`TypeExpr`. Consumers override
//! only the nodes they care about and call the matching `walk_*` to recurse
//! (or DON'T call it to treat a subtree as opaque, e.g. closure bodies in the
//! async await-scan). There is no `_ =>` arm anywhere: adding an `ExprKind`
//! variant is a compile error until every `walk_*` handles it. That
//! exhaustiveness is the entire point — it ends the variant-drift bug class.

use crate::parser::ast::*;

#[allow(unused_variables)]
pub trait Visit: Sized {
    fn visit_expr(&mut self, e: &Expr) { walk_expr(self, e); }
    fn visit_block(&mut self, b: &Block) { walk_block(self, b); }
    fn visit_stmt(&mut self, s: &Statement) { walk_stmt(self, s); }
    fn visit_pattern(&mut self, p: &Pattern) { walk_pattern(self, p); }
    fn visit_type_expr(&mut self, t: &TypeExpr) { walk_type_expr(self, t); }
}

pub fn walk_block<V: Visit>(v: &mut V, b: &Block) {
    for s in &b.statements { v.visit_stmt(s); }
}

pub fn walk_stmt<V: Visit>(v: &mut V, s: &Statement) {
    match s {
        Statement::Let(lb) => {
            v.visit_pattern(&lb.pattern);
            if let Some(t) = &lb.type_annotation { v.visit_type_expr(t); }
            if let Some(val) = &lb.value { v.visit_expr(val); }
        }
        Statement::Expression(e) => v.visit_expr(e),
    }
}

pub fn walk_expr<V: Visit>(v: &mut V, e: &Expr) {
    match &e.kind {
        // Leaves.
        ExprKind::IntLiteral(..) | ExprKind::FloatLiteral(..) | ExprKind::StringLiteral(_)
        | ExprKind::CharLiteral(_) | ExprKind::BoolLiteral(_) | ExprKind::UnitLiteral
        | ExprKind::Identifier(_) | ExprKind::SelfRef | ExprKind::SelfType
        | ExprKind::Continue | ExprKind::NullLiteral | ExprKind::RegexLiteral { .. } => {}

        ExprKind::InterpolatedString(parts) => {
            for p in parts { if let StringPart::Expr(ex) = p { v.visit_expr(ex); } }
        }
        ExprKind::BinaryOp { left, right, .. } => { v.visit_expr(left); v.visit_expr(right); }
        ExprKind::UnaryOp { operand, .. } => v.visit_expr(operand),
        ExprKind::Borrow(x) | ExprKind::BorrowMut(x) => v.visit_expr(x),
        ExprKind::FieldAccess { object, .. } => v.visit_expr(object),
        ExprKind::MethodCall { object, args, block, generic_args, .. } => {
            v.visit_expr(object);
            for t in generic_args { v.visit_type_expr(t); }
            for a in args { v.visit_expr(a); }
            if let Some(b) = block { v.visit_expr(b); }
        }
        ExprKind::SafeNav { object, .. } => v.visit_expr(object),
        ExprKind::SafeNavCall { object, args, .. } => {
            v.visit_expr(object);
            for a in args { v.visit_expr(a); }
        }
        ExprKind::Call { callee, args, block } => {
            v.visit_expr(callee);
            for a in args { v.visit_expr(a); }
            if let Some(b) = block { v.visit_expr(b); }
        }
        ExprKind::Index { object, index } => { v.visit_expr(object); v.visit_expr(index); }
        ExprKind::ClosureCall { callee, args } => {
            v.visit_expr(callee);
            for a in args { v.visit_expr(a); }
        }
        ExprKind::Try(x) => v.visit_expr(x),
        ExprKind::Await(x) => v.visit_expr(x),
        ExprKind::Assign { target, value } => { v.visit_expr(target); v.visit_expr(value); }
        ExprKind::CompoundAssign { target, value, .. } => { v.visit_expr(target); v.visit_expr(value); }
        ExprKind::If(IfExpr { condition, then_body, elsif_clauses, else_body, .. }) => {
            v.visit_expr(condition);
            v.visit_block(then_body);
            for el in elsif_clauses { v.visit_expr(&el.condition); v.visit_block(&el.body); }
            if let Some(b) = else_body { v.visit_block(b); }
        }
        ExprKind::IfLet(IfLetExpr { pattern, value, then_body, else_body, .. }) => {
            v.visit_pattern(pattern);
            v.visit_expr(value);
            v.visit_block(then_body);
            if let Some(b) = else_body { v.visit_block(b); }
        }
        ExprKind::Match(MatchExpr { subject, arms, .. }) => {
            v.visit_expr(subject);
            for a in arms {
                v.visit_pattern(&a.pattern);
                if let Some(g) = &a.guard { v.visit_expr(g); }
                match &a.body {
                    MatchArmBody::Expr(ex) => v.visit_expr(ex),
                    MatchArmBody::Block(b) => v.visit_block(b),
                }
            }
        }
        ExprKind::While(WhileExpr { condition, body, .. }) => { v.visit_expr(condition); v.visit_block(body); }
        ExprKind::WhileLet(WhileLetExpr { pattern, value, body, .. }) => {
            v.visit_pattern(pattern); v.visit_expr(value); v.visit_block(body);
        }
        ExprKind::For(ForExpr { pattern, iterable, body, .. }) => {
            v.visit_pattern(pattern); v.visit_expr(iterable); v.visit_block(body);
        }
        ExprKind::Loop(LoopExpr { body, .. }) => v.visit_block(body),
        ExprKind::Block(b) => v.visit_block(b),
        ExprKind::UnsafeBlock(b) => v.visit_block(b),
        ExprKind::Closure(c) => match &c.body {
            ClosureBody::Expr(ex) => v.visit_expr(ex),
            ClosureBody::Block(b) => v.visit_block(b),
        },
        ExprKind::Range { start, end, .. } => {
            if let Some(s) = start { v.visit_expr(s); }
            if let Some(e2) = end { v.visit_expr(e2); }
        }
        ExprKind::ArrayLiteral(items) | ExprKind::TupleLiteral(items) => {
            for it in items { v.visit_expr(it); }
        }
        ExprKind::ArrayFill { value, count } => { v.visit_expr(value); v.visit_expr(count); }
        ExprKind::MapLiteral(pairs) => { for (k, val) in pairs { v.visit_expr(k); v.visit_expr(val); } }
        ExprKind::Return(opt) | ExprKind::Break(opt) => { if let Some(x) = opt { v.visit_expr(x); } }
        ExprKind::Yield(items) => { for it in items { v.visit_expr(it); } }
        ExprKind::MacroCall { args, .. } => { for a in args { v.visit_expr(a); } }
        ExprKind::Cast { expr, target_type } => { v.visit_expr(expr); v.visit_type_expr(target_type); }
        ExprKind::EnumVariant { args, .. } => { for a in args { v.visit_expr(&a.value); } }
    }
}

pub fn walk_pattern<V: Visit>(v: &mut V, p: &Pattern) {
    // Patterns may contain nested patterns / exprs (ranges, bindings). The
    // implementer fills the exhaustive match from `parser/ast.rs:221` (Pattern enum).
    // Placeholder-free requirement: read the Pattern enum and enumerate every
    // variant here before first green. Most async/block_on consumers do not
    // descend patterns, so a minimal-but-exhaustive body is acceptable.
    let _ = (v, p);
    todo!("enumerate Pattern variants from parser/ast.rs:221 — exhaustive, no _ arm")
}

pub fn walk_type_expr<V: Visit>(v: &mut V, t: &TypeExpr) {
    // Likewise enumerate TypeExpr variants from parser/ast.rs:66 exhaustively.
    let _ = (v, t);
    todo!("enumerate TypeExpr variants from parser/ast.rs:66 — exhaustive, no _ arm")
}
```

> **Implementer obligation (not a placeholder in the plan — an explicit sub-step):** before running the
> test green, open `parser/ast.rs:221` (`Pattern`) and `:66` (`TypeExpr`), replace the two `todo!`
> bodies with exhaustive matches (no `_` arm). These two enums were not transcribed into this plan
> because their full variant set wasn't read during planning; transcribing guessed variants would be
> the invented-precision this skill forbids. The await-scan and block_on consumers (Tasks 4–5) only
> need `walk_expr`/`walk_block`/`walk_stmt`, so they are unblocked regardless; `walk_pattern`/
> `walk_type_expr` must still be exhaustive for the trait to be sound.

- [ ] **Step 3b: Add the `VisitMut` trait + `walk_*_mut` superfns**

Append the mutable mirror (same structure, `&mut Expr`, returns `()`):

```rust
#[allow(unused_variables)]
pub trait VisitMut: Sized {
    fn visit_expr_mut(&mut self, e: &mut Expr) { walk_expr_mut(self, e); }
    fn visit_block_mut(&mut self, b: &mut Block) { walk_block_mut(self, b); }
    fn visit_stmt_mut(&mut self, s: &mut Statement) { walk_stmt_mut(self, s); }
}

pub fn walk_block_mut<V: VisitMut>(v: &mut V, b: &mut Block) {
    for s in &mut b.statements { v.visit_stmt_mut(s); }
}

pub fn walk_stmt_mut<V: VisitMut>(v: &mut V, s: &mut Statement) {
    match s {
        Statement::Let(lb) => { if let Some(val) = &mut lb.value { v.visit_expr_mut(val); } }
        Statement::Expression(e) => v.visit_expr_mut(e),
    }
}

// walk_expr_mut: mirror walk_expr exactly with `&mut` borrows and `v.visit_expr_mut`.
// (Full body mirrors the immutable version arm-for-arm; the implementer copies
//  walk_expr, swaps & for &mut, visit_expr -> visit_expr_mut, and drops the
//  generic_args/type_expr descents that the mutable consumers don't need —
//  but keeps the match EXHAUSTIVE with no _ arm.)
pub fn walk_expr_mut<V: VisitMut>(v: &mut V, e: &mut Expr) {
    // IMPLEMENTER: produce the exhaustive &mut mirror of walk_expr here.
    let _ = (v, e);
    todo!("exhaustive &mut mirror of walk_expr — no _ arm")
}
```

- [ ] **Step 3c: Register the module**

In `compiler/ruxen_core/src/parser/mod.rs`, add alongside the other `mod` decls:

```rust
pub mod visit;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p ruxen_core --lib parser::visit 2>&1 | tee tmp/test-cache/phase1-task3-green.log`
Expected: PASS (2 tests). A compile error naming a missing `ExprKind`/`Pattern`/`TypeExpr` variant is
the exhaustiveness check working — add the arm.

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/parser/visit.rs compiler/ruxen_core/src/parser/mod.rs
git commit -m "feat(parser): add exhaustive Visit/VisitMut AST traversal

One shared, exhaustive walk over the parser AST (no _ arm in any walk_*).
Consumers override the nodes they care about and choose whether to recurse.
Replaces the hand-rolled, drift-prone walkers across async_lowering, repl,
formatter. Adding an ExprKind variant is now a compile error until handled.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 4: Migrate the async await-scan onto `Visit` (closes bug #1)

**Files:**
- Modify: `compiler/ruxen_core/src/async_lowering/mod.rs:4331-4435`
  (`block_contains_await` + `expr_contains_await`)
- Test: `compiler/ruxen_core/tests/async_lowering.rs` (integration) + an inline unit test.

- [ ] **Step 1: Write the failing (bug-exposing) integration test**

Add to `compiler/ruxen_core/tests/async_lowering.rs` (match the file's existing helper style for
compiling a snippet; mirror an existing positive test's harness):

```rust
#[test]
fn await_inside_enum_variant_arg_is_detected() {
    // `let x = Some(fetch().await)` must take the await-aware lowering path,
    // NOT the no-await path that misreports E1110. Bug #1.
    let src = r#"
        async def fetch() -> Int
            42
        end
        async def run() -> Option[Int]
            let x = Some(fetch().await)
            x
        end
    "#;
    // Use the crate's existing "compile and expect success / inspect diagnostics"
    // helper (see the positive tests already in this file). Assert: NO E1110,
    // and run() is lowered to a poll state machine (await-aware path taken).
    assert_compiles_async(src); // adjust to the real helper name in this file
}
```

> Implementer: locate the existing positive-path helper in `async_lowering.rs` (e.g. a
> `fn lower_ok(src: &str)` or similar) and use it; the assertion is "no E1110 diagnostic." If the
> closest existing harness only checks diagnostics, assert the absence of E1110 specifically.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ruxen_core --test async_lowering await_inside_enum_variant_arg 2>&1 | tee tmp/test-cache/phase1-task4-red.log`
Expected: FAIL — E1110 present / no state machine, because `expr_contains_await` misses `EnumVariant`.

- [ ] **Step 3: Replace the two functions with a `Visit` impl**

In `async_lowering/mod.rs`, replace `block_contains_await` (4331-4349) and `expr_contains_await`
(4351-4435) with:

```rust
use crate::parser::visit::{walk_expr, Visit};

/// Detects whether a block/expr contains a `.await` in THIS function's scope.
/// Nested closures have their own scope, so we override `visit_expr` to NOT
/// recurse into closure bodies (the old code's deliberate opacity) — every
/// other node recurses via the shared exhaustive `walk_expr`, so the variants
/// the old hand-rolled scan missed (EnumVariant, UnsafeBlock, IfLet, SafeNav,
/// Range, MapLiteral, ArrayFill, MacroCall, Yield) are now covered. Bug #1.
struct AwaitScan { found: bool }

impl Visit for AwaitScan {
    fn visit_expr(&mut self, e: &Expr) {
        if self.found { return; }
        match &e.kind {
            ExprKind::Await(_) => { self.found = true; }
            // Closure bodies are a separate async scope — opaque to this scan.
            ExprKind::Closure(_) => {}
            _ => walk_expr(self, e),
        }
    }
}

fn block_contains_await(block: &Block) -> bool {
    let mut s = AwaitScan { found: false };
    s.visit_block(block);
    s.found
}

fn expr_contains_await(expr: &Expr) -> bool {
    let mut s = AwaitScan { found: false };
    s.visit_expr(expr);
    s.found
}
```

> Keep both free functions — the rest of `async_lowering` calls them by name; only their bodies change.
> The deliberate closure-opacity is preserved by the `Closure(_) => {}` arm (override, don't recurse).

- [ ] **Step 4: Run tests to verify green**

Run: `cargo test -p ruxen_core --test async_lowering 2>&1 | tee tmp/test-cache/phase1-task4-green.log`
Expected: PASS including the new test. Then the negatives:
`cargo test -p ruxen_core --test async_negative 2>&1 | tee tmp/test-cache/phase1-task4-neg.log`
Expected: PASS (the loop-await E1115 cases still fire — `walk_expr` recurses into loop bodies just like
the old code's explicit Loop/While/WhileLet/For arms did).

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/async_lowering/mod.rs compiler/ruxen_core/tests/async_lowering.rs
git commit -m "fix(async): await-scan covers all expression forms (bug #1)

Rewrite expr/block_contains_await as a Visit impl over the exhaustive
walk_expr. The old hand-rolled scan had _ => false and silently missed
EnumVariant, UnsafeBlock, IfLet, SafeNav(Call), Range, MapLiteral, ArrayFill,
MacroCall, Yield — so `let x = Some(f().await)` routed to the no-await path
and misreported E1110. Closure-opacity preserved via override-don't-recurse.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 5: Migrate `block_on`-rewrite onto `VisitMut` (closes bug #2)

**Files:**
- Modify: `compiler/ruxen_core/src/async_lowering/mod.rs:5304-5491`
  (`rewrite_block_on_in_block` + `rewrite_block_on_in_expr`)
- Test: inline unit test in `async_lowering/mod.rs`'s `mod tests` (line 5803), or
  `tests/async_executor.rs` if `block_on` lowering is exercised there.

- [ ] **Step 1: Write the failing (bug-exposing) test**

Add to the inline `mod tests` at `async_lowering/mod.rs:5803` (use the module's existing
parse-a-program helper; mirror a neighbouring test):

```rust
#[test]
fn block_on_inside_while_let_is_rewritten() {
    // `while let Some(x) = it.next() do block_on(f()) end`
    // The OLD rewrite_block_on_in_expr lacked a WhileLet arm, so the inner
    // block_on(...) call was left un-rewritten. Bug #2.
    let mut program = parse_program(r#"
        def f() -> Int
            7
        end
        def main() -> Unit
            let mut it = [1].iter()
            while let Some(x) = it.next()
                block_on(f())
            end
        end
    "#); // adjust to the module's real parse helper
    rewrite_block_on_calls(&mut program);
    // Assert: no ExprKind::Call to an identifier "block_on" remains anywhere,
    // including inside the while-let body. Walk the program with a Visit that
    // counts residual block_on calls and assert == 0.
    assert_eq!(count_block_on_calls(&program), 0); // helper: small Visit impl in the test
}
```

> Implementer: the module already has `rewrite_block_on_calls` (entry, line 5244) and a test parse
> helper. Add a tiny `count_block_on_calls` test helper using the new `Visit` trait.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p ruxen_core --lib async_lowering::tests::block_on_inside_while_let 2>&1 | tee tmp/test-cache/phase1-task5-red.log`
Expected: FAIL — one residual `block_on` call (the while-let body was skipped).

- [ ] **Step 3: Replace the rewrite walkers with a `VisitMut` impl**

Replace `rewrite_block_on_in_block` (5304-5316) and `rewrite_block_on_in_expr` (5317-5491) with a
`VisitMut` impl that overrides `visit_expr_mut` to perform the same `block_on(future) → loop-poll`
transform the old `_in_expr` did at its `Call`/`ClosureCall` arms, then calls `walk_expr_mut` to
recurse (covering WhileLet and every other node uniformly). The `counter: &mut u32` becomes a field on
the visitor struct.

```rust
use crate::parser::visit::{walk_expr_mut, VisitMut};

struct BlockOnRewriter { counter: u32 }

impl VisitMut for BlockOnRewriter {
    fn visit_expr_mut(&mut self, e: &mut Expr) {
        // First recurse so inner block_on calls are rewritten bottom-up,
        // then handle this node. (Match the old traversal order — the old
        // code recursed into children then transformed; verify against the
        // original and preserve whichever order it used.)
        walk_expr_mut(self, e);
        if let Some(future_expr) = take_block_on_argument(e) {
            let n = self.counter;
            self.counter += 1;
            *e = build_block_on_loop(future_expr, n, &e.span);
        }
    }
}

fn rewrite_block_on_in_block(block: &mut Block, counter: &mut u32) {
    let mut r = BlockOnRewriter { counter: *counter };
    r.visit_block_mut(block);
    *counter = r.counter;
}
```

> Implementer: extract the existing "is this a `block_on(x)` call, and if so give me `x`" recognition
> (currently inline in `rewrite_block_on_in_expr`'s `Call` arm) into `take_block_on_argument(&mut Expr)
> -> Option<Expr>`. Reuse the existing `build_block_on_loop` (line 5493) verbatim. Verify the
> recurse-then-transform vs transform-then-recurse order against the original and preserve it (the test
> in `async_executor.rs` is the backstop).

- [ ] **Step 4: Run tests to verify green**

Run: `cargo test -p ruxen_core --lib async_lowering::tests 2>&1 | tee tmp/test-cache/phase1-task5-green.log`
Expected: PASS including the new while-let test.
Then: `cargo test -p ruxen_core --test async_executor 2>&1 | tee tmp/test-cache/phase1-task5-exec.log`
Expected: PASS (existing block_on behaviour preserved).

- [ ] **Step 5: Commit**

```bash
git add compiler/ruxen_core/src/async_lowering/mod.rs
git commit -m "fix(async): block_on rewrite reaches every node incl. while-let (bug #2)

Rewrite rewrite_block_on_in_* as a VisitMut over the exhaustive walk_expr_mut.
The old hand-rolled walker lacked a WhileLet arm (and others), leaving
block_on(...) inside a while-let body un-rewritten.

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

## Task 6: Phase-1 final integration

**Files:** none (verification only).

- [ ] **Step 1: Run the full compiler-crate suite once**

Run: `cargo test -p ruxen_core 2>&1 | tee tmp/test-cache/phase1-final.log`
Expected: all green. Per global rule 41/42 this is the ONLY full-suite run in Phase 1; intermediate
tasks ran only their narrow tests.

- [ ] **Step 1b: Full multi-agent `/thermonuke` sweep (phase gate)**

Invoke the `thermonuke` skill on the whole Phase 1 diff (`git diff <phase1-base>..HEAD`). This is the
authoritative phase gate — it must confirm the 5 hand-rolled `Ty`/AST traversals targeted by Phase 1
are *reduced to* the two shared primitives (not merely added alongside), that the 3 bugs are closed,
and that no new structural debt was introduced. Surface its report to the maintainer.

- [ ] **Step 2: Confirm no catch-all sneaked into the new traversals**

Run: `grep -nE '_ =>' compiler/ruxen_core/src/parser/visit.rs compiler/ruxen_core/src/hir/types.rs | grep -v 'map_inner_tests\|mod tests'`
Expected: NO matches inside `walk_*` / `map_inner` bodies. (A `_ =>` there defeats the exhaustiveness
guarantee.) The `peel_refs` `other => other` arm is fine — it's a deliberate catch of all non-ref
leaves, not a drift hazard.

- [ ] **Step 3: Report**

Report to maintainer: net line delta (`git diff --stat <phase-base>..HEAD`), the 3 bug tests now green,
full suite green (cite `tmp/test-cache/phase1-final.log`), and the statement: "No behaviour changed
except the 3 named bug fixes; every migration is pinned by a characterization test." Await go-ahead for
Phase 2.

---

## Self-Review (run before handing off)

**Spec coverage:** Root Cause A foundation (✓ Tasks 1,3), bug #1 (✓ Task 4), bug #2 (✓ Task 5),
bug #3 (✓ Task 2). Migrating the *remaining* walkers (eval.rs ×8, comments.rs, e1112/e1115/e1116,
monomorphize) is explicitly deferred to Phases 3 & 6 per the master plan — not a Phase 1 gap.

**Placeholder scan:** Two intentional `todo!()` bodies (`walk_pattern`, `walk_type_expr`,
`walk_expr_mut`) are flagged as explicit implementer sub-steps with the exact source lines to
transcribe from, because their variant sets weren't read during planning and guessing them would be
invented precision. Every other code block is complete and real.

**Type consistency:** `Visit::visit_expr`/`walk_expr` names match across Tasks 3–5; `Ty::map_inner`
signature `(self, &mut impl FnMut(&Ty) -> Ty) -> Ty` matches its use in Task 2; `AwaitScan`/
`BlockOnRewriter`/`AwaitCounter` are distinct; `block_contains_await`/`expr_contains_await` keep their
original names (callers unchanged). `FieldArg`/`EnumVariant`/`WhileLetExpr` field names match
`parser/ast.rs` (verified against the read of lines 285–621).
