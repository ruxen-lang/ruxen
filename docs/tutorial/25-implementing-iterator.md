# Implementing `Iterator` for Your Own Type

> **Status:** the mixin is registered; user types that `include
> Iterator` typecheck.  Runtime monomorphization of user iterators
> **may be partial** in v1 — most coverage is via the builtin
> `Array.iter()` path.  See
> [iterator.spec.md Gaps](../specs/stdlib/iterator.spec.md).
>
> **See also:** [Spec — std.iter (Iterator)](../specs/stdlib/iterator.spec.md)
> for the full pipeline-method surface.

Riven's `Iterator` mixin is the same shape as Rust's `Iterator`:

```riven
mixin Iterator
  type Item
  def mut next -> Option[Self.Item]
end
```

A type that includes `Iterator` and provides `next` automatically
gains the full pipeline surface (`map`, `filter`, `collect`, `fold`,
…).

---

## 1. A minimal user iterator

```riven
class Counter
  current: Int
  limit: Int

  def init(@limit: Int)
    self.current = 0
  end

  include Iterator

  type Item = Int

  def mut next -> Option[Int]
    if self.current >= self.limit
      return nil
    end
    let n = self.current
    self.current = n + 1
    Some(n)
  end
end
```

Usage:

```riven
def main
  var c = Counter.new(5)
  for n in c
    puts "#{n}"
  end
end
```

Output:

```
0
1
2
3
4
```

---

## 2. What `for` desugars to

The `for x in iter` loop expands roughly to:

```riven
var __iter = iter
loop
  match __iter.next
    Some(x) -> # loop body with x bound
    nil    -> break
  end
end
```

So any type with a mutating `next -> Option[Item]` method can drive
a `for` loop.

---

## 3. Pipeline methods come for free

Because `Counter` includes `Iterator`, you get the whole pipeline:

```riven
let total = Counter.new(10).fold(0, |acc, x| acc + x)
let evens: Array[Int] = Counter.new(10).filter(|x| x % 2 == 0).collect[Array[Int]]()
puts "total=#{total} evens.len=#{evens.len}"
```

No extra code on your side — the pipeline methods are provided by
the mixin's default methods and monomorphisation.

---

## 4. Returning your iterator type

```riven
def count_to(n: Int) -> Counter
  Counter.new(n)
end
```

The concrete type is what callers see.  If you want to hide the
type, return `some Iterator[Item = Int]`:

```riven
def count_to(n: Int) -> some Iterator[Item = Int]
  Counter.new(n)
end
```

The associated type `Item` is bound at the function signature.

---

## 5. Implementing `FromIterator`

The reverse direction — collecting an arbitrary iterator into your
type — is the `FromIterator` mixin:

```riven
mixin FromIterator
  type Item
  def self.from_iter(iter: some Iterator[Item = Self.Item]) -> Self
end
```

```riven
class Stats
  count: USize
  sum: Int

  def init
    self.count = 0
    self.sum = 0
  end

  include FromIterator

  type Item = Int

  def self.from_iter(iter: some Iterator[Item = Int]) -> Stats
    var s = Stats.new
    for n in iter
      s.count = s.count + 1
      s.sum = s.sum + n
    end
    s
  end
end

def main
  let s = Counter.new(5).collect[Stats]()
  puts "n=#{s.count} sum=#{s.sum}"
end
```

The `.collect[Stats]()` call routes through your `from_iter`.

---

## 6. Common pitfalls

- **Forgetting `def mut next`.**  The iterator advances state; the
  method must be a *mutating* method.  A reading-method signature
  won't satisfy the mixin.
- **Forgetting `type Item = ...`** in the type body.  This is an
  associated type that must be bound where you `include Iterator`.
- **Returning `Option[T]` where `T` differs from `Self.Item`.**
  The two have to match; the mixin machinery doesn't widen.
- **Calling `.iter()` on a user type.**  Today only `Array`, `Map`,
  `Set`, and `String` expose `.iter()` as a method.  Your type
  *is* the iterator — there's nothing to "go into iter mode" on.
- **Generic `Iterator`.**  `mixin Iterator[T] ... end` is not
  Riven's shape.  Use the associated-type form above; it matches
  Rust and the builtin pipeline expects it.

---

## 7. When the iterator state is borrowed

If your iterator borrows from another collection, the lifetime
flows through the type's parameter list and out through the
`Item` binding.  Recall that **lowercase identifiers in `[...]`
are lifetimes; uppercase are type parameters**:

```riven
class WindowIter[T, a]
  source: &a Array[T]
  pos: USize

  def init(@source: &a Array[T])
    self.pos = 0
  end

  include Iterator

  type Item = &a T

  def mut next -> Option[&a T]
    if self.pos >= self.source.len
      return nil
    end
    let item = &self.source[self.pos]
    self.pos = self.pos + 1
    Some(item)
  end
end
```

The borrow checker tracks the lifetime `a`; the returned `&a T`
must outlive the iterator value, which itself can't outlive the
source `Array`.

---

## 8. Where this lives in the compiler

- `Iterator` mixin registered in `crates/riven-core/src/resolve/mod.rs`
  (search for `"Iterator"`).
- `FromIterator` mixin registered there too.
- Pipeline method resolution in `crates/riven-core/src/typeck/infer.rs`
  (search for `"sum"`, `"fold"`, `"map"`, `"filter"`).
- Runtime `riven_*_from_iter` helpers in
  `crates/riven-core/runtime/runtime.c`.

---

**Next:** browse the [spec index](../specs/README.md) for every
formal contract Riven currently enforces, or jump to
[chapter 14 — FFI](14-ffi.md) for cross-language interop.
