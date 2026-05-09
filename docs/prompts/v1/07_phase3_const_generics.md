# 07 — Phase 3: const generics (T2.02)

**Depends on:** Phase 2 stdlib complete.
**Reads:** `docs/requirements/tier2_02_const_generics.md`.

## Goal

Allow types to be parameterized by `Int` (and `Bool`) compile-time
values, e.g. `Array[T, N: Int]`, `BitSet[N: Int]`.

## Surface

```riven
struct Array[T, N: Int]
  data: [T; N]
  ...
end

def main
  let a = Array[Int, 4].new
  puts "#{a.len}"
end
```

Const expressions allowed in type positions: literal, named const,
simple arithmetic over const params (`N + 1`).

## TDD

1. Failing parser test: `Array[Int, 4]` parses.
2. Failing typeck test: `Array[Int, 4]` is distinct from
   `Array[Int, 5]`.
3. Failing monomorphization test: `Array[Int, 4]::new` and
   `Array[Int, 5]::new` lower to two distinct MIR functions with
   correct array sizes.
4. E2E fixture exercising stack-allocated fixed-size array.
5. Negative test: `Array[Int, "hello"]` — type mismatch (E0701).

## Implementation steps

1. **Parser**: extend generic param syntax to allow `N: Int` (kind
   annotation). Update `GenericParamInfo`.
2. **Resolve**: distinguish type-kind vs const-kind generic args.
3. **Typeck**: const args participate in unification; two types
   differ if their const args differ. Add evaluator for simple const
   exprs.
4. **Monomorphization**: generate one MIR fn per const arg
   combination.
5. **MIR**: extend `Ty::Array { elem, size: ConstValue }` and
   `MirInst::Alloc` to honor compile-time size.
6. **Codegen**: stack-allocate fixed-size arrays via Cranelift /
   LLVM `alloca`.

## Reserved error codes

- E0700 — kind mismatch on generic arg
- E0701 — wrong const-arg type
- E0702 — non-const expression in const-arg position
- E0703 — const-arg expression overflows during evaluation

## Definition of done

- [ ] Const generics parse + resolve + typecheck + monomorphize.
- [ ] Fixed-size array `[T; N]` works end-to-end.
- [ ] At least 5 e2e fixtures covering monomorphization,
      negative cases, nested const generics.
- [ ] CI green.
- [ ] CHANGELOG bullet.
