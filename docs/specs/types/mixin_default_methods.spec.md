# Spec — Mixin default method bodies

**Source docs:** continuation of the trait/mixin work begun during
the multithreading round (`send_sync_enforcement.spec.md`) and async
sub-phase 1 (`async.spec.md`).

**Status:** [implemented] (pins below). Before this spec, `mixin Foo; def bar;
end` only declared a REQUIRED method; this spec added DEFAULT bodies.

Spelling today: a mixin is declared with `mixin`, which is [deprecated]; the
Ruby form `module Foo ... end` with `include Foo` is
[planned](../../../TODO.md#t-module-mixins). A default body is a full `def ... end` and
calls another method of the mixin as `self.name`; an endless `def` or a bare
`name` call in a default does not compile yet [planned](../../../TODO.md#t-implicit-self).
The examples below use a mixin named `Rankable`, because the stdlib already
defines `Comparable` (with `<=>`).

The need was surfaced by `library/std/core/src/lib.rx`'s comment:
"aspirational broader surfaces are intentionally left out — adding
required methods today would break every user impl that doesn't
provide them. They'll expand once default-method lowering for
mixins ships." This spec is the "ships" half.

---

## B1 — Mixin can declare a method with a body

Works today, [deprecated] `mixin` (replacement `module`, [planned](../../../TODO.md#t-module-mixins)):

```ruby
mixin Rankable
  def compare(other: &Self) -> Int        # required, no body
  def less_than(other: &Self) -> Bool
    self.compare(other) < 0
  end
  def greater_than(other: &Self) -> Bool
    self.compare(other) > 0
  end
end
```

`compare` is required; `less_than` and `greater_than` carry default
bodies that call `compare`. Parser accepts both shapes inside the
mixin block — `def name` (no body) for required, `def name ...
end` (with body) for default.

## B2 — Implementor inherits defaults automatically

Works today, with the [deprecated] `def init` (replacement `def initialize`, [planned](../../../TODO.md#t-initialize)):

```ruby
class Money
  amount: Int
  include Rankable

  def init(@amount: Int)
  end

  def compare(other: &Money) -> Int      # required — must implement
    @amount - other.amount
  end
  # No less_than / greater_than — inherited from Rankable.
end

m1 = Money.new(10)
m2 = Money.new(20)
puts "#{m1.less_than(m2)}"    # → true (uses default body)
```

The class doesn't redeclare `less_than`; typeck and codegen treat
calls to `m1.less_than(m2)` as dispatching to Rankable's default
body, with `self.compare(other)` resolved to `Money.compare`.

## B3 — Implementor can override defaults

Example [implemented]:

```ruby
class FastInt
  value: Int
  include Rankable

  def compare(other: &FastInt) -> Int
    @value - other.value
  end

  # Override less_than with a more efficient implementation:
  def less_than(other: &FastInt) -> Bool
    @value < other.value    # skips the .compare indirection
  end
end
```

Typeck verifies the override's signature matches the mixin's
declared signature (same param types, same return type). Codegen
emits a call to FastInt.less_than, not the inherited default.

## B4 — Missing required method still rejected (E0612)

Example [implemented], rejected on purpose:

```ruby
class Broken
  include Rankable
  # No def compare — error.
end
```

→ `[E0612] class Broken includes mixin Rankable but does not
implement required method 'compare'`. Same diagnostic the existing
mixin satisfaction check emits today; defaults don't change that
contract.

## B5 — Lowering

For each `include MixinFoo` on a class, the resolver:
1. Walks MixinFoo's method declarations.
2. For each method with a body: if the class doesn't have a
   same-named method, synthesize one on the class with the mixin's
   body. The synthesised method's `self` references substitute to
   the class type.
3. For each method without a body: ensure the class provides one;
   if not, emit E0612.

The synthesised methods get the same dispatch shape as user-written
methods (FFI alias map, MIR dispatch). They go through the
"synthesise an AST item on the class" pattern established by
async-lowering (commit `74c846d`) — prepend to the class's method
list so resolution sees them at the same time as user-written ones.

## B6 — Default body can call other mixin methods

A default body can reference any method on the mixin's contract —
required OR default. Resolution happens at the class level after
synthesis.

Works today, [deprecated] `mixin` (replacement `module`, [planned](../../../TODO.md#t-module-mixins)):

```ruby
mixin Describable
  def name -> String        # required
  def title -> String       # default — uses name
    "Dr. #{self.name}"
  end
  def greeting -> String    # default — uses title
    "Hello, #{self.title}"
  end
end
```

Resolution chain: `greeting` → `title` → `name`. As long as `name` is
provided by the implementor (or by another default), every default
resolves cleanly.

## B7 — Negative: signature mismatch on override (E0613)

Rejected on purpose, [deprecated] `mixin` (replacement `module`, [planned](../../../TODO.md#t-module-mixins)):

```ruby
mixin Foo
  def bar(x: Int) -> Bool
    true
  end
end

class Bad
  include Foo
  def bar(x: String) -> Bool   # wrong param type
    true
  end
end
```

→ `[E0613] override of mixin method 'bar' signature mismatches:
expected (x: Int) -> Bool, found (x: String) -> Bool`.
Existing E0613 — reused.

---

## Pin tests

| Behaviour | Test fn                                              | File                            |
|-----------|------------------------------------------------------|---------------------------------|
| B1, B2    | e2e `cases/86_mixin_default_method_used.rx` (default body interpolates `self`) | release-e2e |
| B2 e2e    | e2e `cases/22_mixin_default.rx`, `cases/86_mixin_default_method_used.rx` | release-e2e |
| B2 e2e    | e2e `cases/980_mixin_default_block_self_param.rx`, `cases/981_mixin_default_block_generic_return.rx` | release-e2e |

B3, B4, B6 and B7 have no dedicated pin yet.

---

## Stdlib impact

Once this ships, `library/std/core/src/lib.rx`'s mixin declarations
can grow richer defaults. Candidates (separate prompt, not this
task's scope):
- `PartialOrd`: `<`, `<=`, `>`, `>=` on top of `partial_cmp`.
- `Ord`: `min`, `max`, `clamp` on top of `cmp`.
- `Comparable`: `less_than`, `greater_than`, `between` on top of `compare`.
- `Hash`: `hash_bytes` (default) on top of `hash`.

That's task #19's polish-pass. (The stdlib `Comparable` now provides `<`,
`<=`, `==`, `>`, `>=`, `between?` and `clamp` on top of `<=>` [implemented].)

## Owner decisions (2026-10-25)

- D5 a: an `include` keeps its generic arguments in HIR (`include Into[X]`
  loses `X` today) [planned](../../../TODO.md#t-d5).
- D7 b: a method that needs a bound on its class's type parameter uses the
  method `where` bound (`def sum -> T where T: Add`), the form that works
  today and is [deprecated]; the RBS respelling `[T < Bound]` is
  [planned](../../../TODO.md#t-d7-respell). OQ-6 stays open.
- D11 a, X3 a: no associated types in mixin defaults are built for the async
  step futures; they stay shared functions, the floor until Fibers
  [planned](../../../TODO.md#t-d11). `Self::Output` projections resolve declared
  equalities [planned](../../../TODO.md#t-x3).

## Out of scope

- **Mixin generics** (`mixin Container[T]`) — not in this task.
- **Mixin associated types** (`type Output` on Future) — already supported.
- **Specialization** — overriding a default with a different
  signature variant (e.g. for performance) is allowed via B3; full
  specialization (multiple defaults selected by type) is v2.
- **Mixin inheritance** (`mixin Foo: Bar end`) — not in this task; that
  spelling is [deprecated], replaced by `include Bar` inside `module Foo`.
