# 08 — Phase 3: HRTBs (T2.03) + `some Mixin` (T2.04, no specialization)

> **Status: 🟡 Partial / deferred** (audited 2026-05-21). `some Mixin`
> token + parser + typeck path present (`SomeBound` lexer token,
> `parser/types.rs:47`, `Ty::SomeMixin(bounds)`). Used in
> `async def foo() -> some Future[Output=T]` lowering paths.
> Full HRTB semantics (`for<'a>` quantified bounds) NOT shipped —
> deferred to v1.5/v2 per the original prompt header below.

> **DEFERRED 2026-05-17 → v1.5 or v2**
>
> Per `docs/STRATEGY.md`, higher-ranked trait bounds are a pure
> power-user feature. Crystal shipped 1.0 without HRTBs. Not blocking
> any flagship app, Ruby-FFI use case, or WASM use case. **Do not
> work on this prompt during v1.** Skip directly from #07 closure
> to #10 (LSP). Re-evaluate after a wedge is chosen and one full
> v1 release ships.

**Depends on:** prompt 07.
**Reads:** `docs/requirements/tier2_03_hrtbs.md`,
`docs/requirements/tier2_04_some_mixin_and_specialization.md`.

## A. Higher-ranked mixin bounds (HRTBs)

### Goal
Allow `for[a]` quantification on lifetime params in mixin bounds:

```riven
def takes_closure[F: for[a] Fn(&a Int) -> Int](f: F) -> Int
  ...
end
```

### TDD
- Parser test for `for[a]` syntax.
- Typeck test: HRTB unifies against any concrete lifetime.
- E2E fixture invoking with two different lifetimes through one bound.

### Implementation
- Extend the mixin-bound AST node (Rust-side compiler type) with a
  `for_lifetimes: Vec<LifetimeName>` field.
- During unification, fresh-instantiate the bound for each call site.

## B. `some Mixin` (return-position only for v1)

### Goal
Function return position `-> some Iterator[Item=Int]` (v1 scope:
return-position only; argument-position can wait until v2). One
concrete conforming type per definition; compiler picks it, caller
sees the opaque view.

### TDD
- Parser test for `-> some Mixin`.
- Typeck test: caller sees opaque type that satisfies the mixin.
- E2E fixture returning a `Map`/`Filter` chain without naming the type.

### Implementation
- New internal `Ty::Opaque { bounds: Vec<MixinBound>, def_id: DefId }`
  Rust enum variant (compiler-internal; surface vocabulary is
  `some Mixin`).
- Typeck unifies opaque with concrete only at definition site.
- Codegen monomorphizes over the captured concrete type the same way
  generic returns are handled.

### Out of scope (deferred to v2)
- Min-specialization (`default` directive on a mixin include).
  Per Open Decision #12, defer. Add a parser-level reject with E0710
  if `default` keyword appears.

## Reserved error codes

- E0704 — HRTB syntax error
- E0705 — opaque type leaks into auto-mixin position
- E0706 — opaque type unifies with two concrete types
- E0710 — `default` directive rejected (specialization deferred)

## Definition of done

- [ ] HRTBs parse, typecheck, and execute.
- [ ] Return-position `some Mixin` works for at least the Iterator
      adapter chain case.
- [ ] `default` keyword rejected with E0710.
- [ ] At least 3 e2e fixtures per sub-feature.
- [ ] CI green.
- [ ] CHANGELOG bullet.
