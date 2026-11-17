# Spec — User-defined modules

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §6](../../requirements/tier1_01_stdlib.md).

**Status:** parser surface shipped (Phase 1). A qualified path to a class
inside a user module resolves today (`Geometry::Point.new(3, 4)`,
[implemented]; naming spec section 12). The gaps left are listed in B4.

---

## B1 — `module Name ... end` parses

Modules and qualified class paths are [implemented]. The constructor is
spelled `def init` until `initialize` lands [planned](../../../TODO.md#t-initialize), and
the return type is written `Geometry.Point` because `::` in a type position is
[planned](../../../TODO.md#t-type-colons).

Works today, with the [deprecated] `def init` (replacement `def initialize`):

```ruby
module Geometry
  class Point
    x: Int
    y: Int

    def init(@x: Int, @y: Int)
    end
  end

  def origin -> Geometry.Point
    Geometry::Point.new(0, 0)
  end
end

pt = Geometry::Point.new(3, 4)
puts pt.x   # 3
```

The parser produces a `TopLevelItem::Module(ModuleDef { name,
items, span })` (see
[`parser/ast.rs` `ModuleDef`](../../../compiler/ruxen_core/src/parser/ast.rs)).
Nested top-level items inside the module are parsed normally.

## B2 — Nested modules parse

Example [implemented] (a function in a module needs its return type written):

```ruby
module Outer
  module Inner
    def hi -> Int = 1
  end
end
```

Modules nest to arbitrary depth at the parser layer.  Each nested
module produces another `ModuleDef`.

## B3 — Module body accepts every top-level item form

Inside a `module ... end` body, the parser accepts the same items
the top level accepts: `class`, `def`, `lib` and nested `module`
[implemented], plus the [deprecated] `struct`, `enum`, `mixin`, `extension`,
`use`, `type` alias, `newtype` and `let` (module-level constant), each of which
the naming spec gives a Ruby replacement for.

## B4 — What resolves today, and the gaps

- A qualified path to a class in a user module, `Geometry::Point.new(3, 4)`,
  resolves and runs [implemented] (B1 example).
- Inside the module body, a sibling is not visible by its bare name:
  `def origin -> Point` reports "undefined type `Point`". Write the qualified
  `Geometry.Point` today, a [known issue](../../../TODO.md#t-module-self-call).
- Calling a module-level function from outside (`Geometry::origin`) does not
  parse yet, a [known issue](../../../TODO.md#t-module-self-call).

---

## Pin tests

| Behaviour | Test fn                                       | File                          |
|-----------|-----------------------------------------------|-------------------------------|
| B1        | `parse_module_def_basic`                      | `const_generics.rs` neighbour — to be added; see Gaps |
| B2        | gap                                           |                               |
| B3        | gap                                           |                               |
| B4        | (negative — covered by absence of fixtures using user-module paths) | |

---

## Gaps (significant)

- All parser pins for module declarations are unwritten.  The
  feature ships via the parser but with no dedicated integration
  test verifying the AST shape.
- B4 pins: a positive pin for `Geometry::Point.new` and negative pins
  for the two gaps in B4.

## Out of scope until resolver support lands

- `private` visibility markers on user-module items.
- Re-exports (`use ...` re-exports).
- Cross-file module trees.
- Use-paths into user modules (`use geometry.Point`); `use` itself is
  [deprecated] in favour of `require` and qualified names.

When the resolver gains user-module support, this spec gains B-rows
for:
- B5: `use mymod.X` binds `X` in scope.
- B6: `mymod.X` qualified path works.
- B7: public `def f` is reachable from outside; `private def f` is not.
