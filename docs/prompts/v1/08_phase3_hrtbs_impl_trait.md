# 08 — Phase 3: HRTBs (T2.03) + impl Trait (T2.04, no specialization)

**Depends on:** prompt 07.
**Reads:** `docs/requirements/tier2_03_hrtbs.md`,
`docs/requirements/tier2_04_impl_trait_and_specialization.md`.

## A. Higher-ranked trait bounds (HRTBs)

### Goal
Allow `for[a]` quantification on lifetime params in trait bounds:

```riven
def takes_closure[F: for[a] Fn(&[a] Int) -> Int](f: F) -> Int
  ...
```

### TDD
- Parser test for `for[a]` syntax.
- Typeck test: HRTB unifies against any concrete lifetime.
- E2E fixture invoking with two different lifetimes through one bound.

### Implementation
- Extend `TraitBound` AST with `for_lifetimes: Vec<LifetimeName>`.
- During unification, fresh-instantiate the bound for each call site.

## B. impl Trait (return-position only for v1)

### Goal
Function return position `-> impl Iterator[Item=Int]` (v1 scope:
return-position only; argument-position can wait until v2).

### TDD
- Parser test for `-> impl Trait`.
- Typeck test: caller sees opaque type that satisfies `Trait`.
- E2E fixture returning a `Map`/`Filter` chain without naming the type.

### Implementation
- New `Ty::Opaque { trait_bounds: Vec<TraitBound>, def_id: DefId }`.
- Typeck unifies opaque with concrete only at definition site.
- Codegen monomorphizes over the captured concrete type the same way
  generic returns are handled.

### Out of scope (deferred to v2)
- Min-specialization (`default impl`). Per Open Decision #12, defer.
  Add a parser-level reject with E0710 if `default` keyword appears.

## Reserved error codes

- E0704 — HRTB syntax error
- E0705 — opaque type leaks into auto-trait position
- E0706 — opaque type unifies with two concrete types
- E0710 — `default impl` rejected (specialization deferred)

## Definition of done

- [ ] HRTBs parse, typecheck, and execute.
- [ ] Return-position `impl Trait` works for at least the Iterator
      adapter chain case.
- [ ] `default` keyword rejected with E0710.
- [ ] At least 3 e2e fixtures per sub-feature.
- [ ] CI green.
- [ ] CHANGELOG bullet.
