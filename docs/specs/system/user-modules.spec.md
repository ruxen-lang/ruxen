# Spec — User-defined modules

**Source docs:**
[docs/requirements/tier1_01_stdlib.md §6](../../requirements/tier1_01_stdlib.md).

**Status:** parser surface shipped (Phase 1); **resolver only
handles `std.*` paths** today.  User modules parse and produce
`ModuleDef` AST nodes but don't yet bind into scope.

---

## B1 — `module foo ... end` parses

```ruxen
module geometry
  struct Point
    x: Int
    y: Int
  end

  def origin -> Point
    Point { x: 0, y: 0 }
  end
end
```

The parser produces a `TopLevelItem::Module(ModuleDef { name,
items, span })` (see
[`parser/ast.rs` `ModuleDef`](../../../crates/ruxen-core/src/parser/ast.rs)).
Nested top-level items inside the module are parsed normally.

## B2 — Nested modules parse

```ruxen
module outer
  module inner
    def hi -> Int { 1 }
  end
end
```

Modules nest to arbitrary depth at the parser layer.  Each nested
module produces another `ModuleDef`.

## B3 — Module body accepts every top-level item form

Inside a `module ... end` body, the parser accepts the same items
the top level accepts: `struct`, `class`, `enum`, `mixin`,
`extension`, `def`, `use`, `type` alias, `newtype`, `let` (module-
level constant), `lib`.

## B4 — Resolver currently rejects `outer.inner` paths for user modules

**Given** `module geometry struct Point ... end end`
**When** code outside the module writes `geometry.Point`
**Then** today the resolver emits a "name not in scope" diagnostic.
The path-resolution side of user modules is **not yet implemented**.

This is the explicit gap that distinguishes the parser surface
(shipped) from the resolution surface (pending).  Users who need
modular organisation today should split code across files via the
existing `use` machinery for `std.*` paths.

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
- B4 negative pin: assert that `geometry.Point` fails to resolve
  while inside the same file as the `module geometry ... end`
  declaration, with a clear error mentioning user-module support
  isn't yet wired.

## Out of scope until resolver support lands

- `mod foo;` file-based module declarations (Rust style).
- `private` visibility markers on user-module items.
- Re-exports (`use ...` re-exports).
- Cross-file module trees.
- Use-paths into user modules (`use geometry.Point`).

When the resolver gains user-module support, this spec gains B-rows
for:
- B5: `use mymod.X` binds `X` in scope.
- B6: `mymod.X` qualified path works.
- B7: public `def f` is reachable from outside; `private def f` is not.
