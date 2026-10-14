# Spec — Mixin default method bodies

**Source docs:** continuation of the trait/mixin work begun during
the multithreading round (`send_sync_enforcement.spec.md`) and async
sub-phase 1 (`async.spec.md`).

**Status:** new language feature. Today `mixin Foo; def bar; end`
only declares a REQUIRED method — implementors must supply the
body. There's no support for DEFAULT bodies on mixin methods. This
spec adds that.

The need was surfaced by `library/std/core/src/lib.rx`'s comment:
"aspirational broader surfaces are intentionally left out — adding
required methods today would break every user impl that doesn't
provide them. They'll expand once default-method lowering for
mixins ships." This spec is the "ships" half.

---

## B1 — Mixin can declare a method with a body

```rx
mixin Comparable
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

```rx
class Money
  amount: Int
  include Comparable

  def compare(other: &Money) -> Int      # required — must implement
    self.amount - other.amount
  end
  # No less_than / greater_than — inherited from Comparable.
end

def main
  let m1 = Money.new(10)
  let m2 = Money.new(20)
  puts "#{m1.less_than(m2)}"    # → true (uses default body)
end
```

The class doesn't redeclare `less_than`; typeck and codegen treat
calls to `m1.less_than(m2)` as dispatching to Comparable's default
body, with `self.compare(other)` resolved to `Money.compare`.

## B3 — Implementor can override defaults

```rx
class FastInt
  value: Int
  include Comparable

  def compare(other: &FastInt) -> Int
    self.value - other.value
  end

  # Override less_than with a more efficient implementation:
  def less_than(other: &FastInt) -> Bool
    self.value < other.value    # skips the .compare indirection
  end
end
```

Typeck verifies the override's signature matches the mixin's
declared signature (same param types, same return type). Codegen
emits a call to FastInt.less_than, not the inherited default.

## B4 — Missing required method still rejected (E0612)

```rx
class Broken
  include Comparable
  # No def compare — error.
end
```

→ `[E0612] class Broken includes mixin Comparable but does not
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
synthesis:

```rx
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

```rx
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
| B1, B2    | `mixin_default_body_with_string_interp_typechecks_and_runs` | `tests/mixin_defaults.rs` |
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

That's task #19's polish-pass.

## Out of scope

- **Mixin generics** (`mixin Container[T]`) — not in this task.
- **Mixin associated types** (`type Output` on Future) — already supported.
- **Specialization** — overriding a default with a different
  signature variant (e.g. for performance) is allowed via B3; full
  specialization (multiple defaults selected by type) is v2.
- **Mixin inheritance** (`mixin Foo: Bar end`) — not in this task.
