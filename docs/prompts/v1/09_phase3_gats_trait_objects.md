# 09 — Phase 3: GATs (T2.05) + trait objects (T2.06)

**Depends on:** prompts 07-08.
**Reads:** `docs/requirements/tier2_05_gats.md`,
`docs/requirements/tier2_06_trait_objects.md`.

## A. Generic associated types (GATs)

### Goal
Trait associated types that themselves take generic parameters:

```riven
trait Lender
  type Borrowed[a]
  def lend(self: &Self) -> Self.Borrowed[a]
end
```

### TDD
- Parser: `type Item[a]` with lifetime arg.
- Typeck: instantiation site supplies the lifetime.
- E2E fixture: streaming-iterator pattern with lifetime-tied items.

### Implementation
- Extend `AssocType` AST with `generic_params: Vec<GenericParamInfo>`.
- During unification, generate fresh substitutions for each use site.
- Codegen: monomorphize over each instantiation.

## B. Trait objects (`dyn Trait`)

### Goal
Dynamic dispatch with vtables:

```riven
let shapes: Vec[Box[dyn Shape]] = Vec.new
shapes.push(Box.new(Circle.new(3)))
shapes.push(Box.new(Square.new(2)))
for s in shapes
  puts "#{s.area}"
end
```

### TDD
- Parser: `dyn Trait` and `Box[dyn Trait]`.
- Typeck: object-safety check rejects non-object-safe traits
  (Self in return / generic methods → E0707).
- Codegen: vtable layout, indirect call.
- E2E fixture: heterogeneous `Vec[Box[dyn Trait]]`.

### Implementation
- Each trait gains an optional vtable struct: pointer per method.
- `Box[dyn Trait]` is a fat pointer `(data_ptr, vtable_ptr)`.
- Method call lowers to vtable indirect call instead of direct.
- Object-safety check: error E0707 for traits that take `Self` by
  value or have generic methods.

## Reserved error codes

- E0707 — trait is not object-safe
- E0708 — GAT lifetime arg missing at use site
- E0709 — `dyn Trait` requires `Box`/`&` indirection

## Definition of done

- [ ] GATs parse, typecheck, monomorphize.
- [ ] `dyn Trait` works with `Box`, `&`, `&mut`.
- [ ] Object-safety errors trigger E0707.
- [ ] Vec of dyn objects iterates correctly.
- [ ] CI green.
- [ ] CHANGELOG bullet.
