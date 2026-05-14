# 09 — Phase 3: GATs (T2.05) + `any Mixin` (T2.06)

**Depends on:** prompts 07-08.
**Reads:** `docs/requirements/tier2_05_gats.md`,
`docs/requirements/tier2_06_trait_objects.md` (requirements doc
slug pending separate rename — content concerns the `any Mixin`
feature).

## A. Generic associated types (GATs)

### Goal
Mixin associated types that themselves take generic parameters:

```riven
mixin Lender
  type Borrowed[a]
  def lend(self: &Self) -> Self.Borrowed[a]
end
```

### TDD
- Parser: `type Item[a]` with lifetime arg.
- Typeck: instantiation site supplies the lifetime.
- E2E fixture: streaming-iterator pattern with lifetime-tied items.

### Implementation
- Extend the assoc-type AST node (Rust-side compiler type) with a
  `generic_params: Vec<GenericParamInfo>` field (internal struct
  names preserved pending sweep).
- During unification, generate fresh substitutions for each use site.
- Codegen: monomorphize over each instantiation.

## B. `any Mixin` (existential mixin with vtable)

### Goal
Dynamic dispatch with vtables. A value of type `any Mixin` carries
both a data pointer and a vtable pointer (fat pointer, 16 bytes on
64-bit targets); methods are dispatched indirectly through the
vtable, allowing one function body to handle every conforming type.

```riven
var shapes: Array[Box[any Shape]] = Array.new
shapes.push(Box.new(Circle.new(3)))
shapes.push(Box.new(Square.new(2)))
for s in shapes
  puts "#{s.area}"
end
```

### TDD
- Parser: `any Mixin` and `Box[any Mixin]`.
- Typeck: object-safety check rejects non-object-safe mixins
  (Self-by-value in return / per-method generic params → E-ANY-* —
  e.g. `E-ANY-SELF-BY-VALUE`, `E-ANY-METHOD-GENERIC`).
- Codegen: vtable layout, indirect call.
- E2E fixture: heterogeneous `Array[Box[any Mixin]]`.

### Implementation
- Each mixin gains an optional vtable struct: pointer per method.
- `Box[any Mixin]` is a fat pointer `(data_ptr, vtable_ptr)`.
- Method call lowers to vtable indirect call instead of direct.
- Object-safety check: emit a code in the `E-ANY-*` family (legacy
  `E0707` slot reserved) for mixins that take `Self` by value or
  have per-method generic parameters.
- `any Mixin` requires an explicit `include Mixin` in the
  implementing class — structural satisfaction is accepted only for
  `some Mixin`, never for `any`.

## Reserved error codes

- E0707 — mixin is not object-safe (legacy code; message text and
  documentation use the `E-ANY-*` vocabulary)
- E0708 — GAT lifetime arg missing at use site
- E0709 — `any Mixin` requires `Box` / `&` indirection (unsized)

## Definition of done

- [ ] GATs parse, typecheck, monomorphize.
- [ ] `any Mixin` works with `Box`, `&`, `&mut`.
- [ ] Object-safety errors trigger E0707 with the `E-ANY-*`
      message family.
- [ ] Array of `any Mixin` objects iterates correctly.
- [ ] CI green.
- [ ] CHANGELOG bullet.
