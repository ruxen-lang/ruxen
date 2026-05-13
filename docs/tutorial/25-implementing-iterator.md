# Implementing `Iterator` for Your Own Type

> **Status:** trait is registered; user-side `impl Iterator for T`
> typechecks.  Runtime monomorphization of user iterators **may be
> partial** in v1 — most coverage is via the builtin `Vec.iter()`
> path.  See [iterator.spec.md Gaps](../specs/stdlib/iterator.spec.md).
>
> **See also:** [Spec — std::iter (Iterator)](../specs/stdlib/iterator.spec.md)
> for the full pipeline-method surface.

Riven's `Iterator` trait is the same shape as Rust's:

```riven
trait Iterator
  type Item
  def next(&mut self) -> Option[Self::Item]
end
```

A type that implements `next` automatically gains the full
pipeline surface (`map`, `filter`, `collect`, `fold`, …).

---

## 1. A minimal user iterator

```riven
class Counter
  current: Int
  limit: Int

  def init(@limit: Int)
    self.current = 0
  end
end

impl Iterator for Counter
  type Item = Int

  def next(&mut self) -> Option[Int]
    if self.current >= self.limit
      return None
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
  let mut c = Counter.new(5)
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
let mut __iter = iter
loop
  match __iter.next()
    Some(x) -> # loop body with x bound
    None    -> break
  end
end
```

So any type with a `next() -> Option[Item]` method can drive a `for`
loop.

---

## 3. Pipeline methods come for free

Because `Counter` implements `Iterator`, you get the whole pipeline:

```riven
let total = Counter.new(10).fold(0, |acc, x| acc + x)
let evens: Vec[Int] = Counter.new(10).filter(|x| x % 2 == 0).collect[Vec[Int]]()
puts "total=#{total} evens.len=#{evens.len}"
```

No extra code on your side — the pipeline methods are provided by
the trait + monomorphisation.

---

## 4. Returning your iterator type

```riven
def count_to(n: Int) -> Counter
  Counter.new(n)
end
```

The concrete type is what callers see.  If you want to hide the
type, return `impl Iterator[Item = Int]`:

```riven
def count_to(n: Int) -> impl Iterator[Item = Int]
  Counter.new(n)
end
```

The associated type `Item` is bound at the function signature.

---

## 5. Implementing `FromIterator`

The reverse direction — collecting an arbitrary iterator into your
type — is the `FromIterator` trait:

```riven
trait FromIterator
  type Item
  def from_iter(iter: impl Iterator[Item = Self::Item]) -> Self
end
```

```riven
class Stats
  count: USize
  sum: Int
end

impl FromIterator for Stats
  type Item = Int

  def from_iter(iter: impl Iterator[Item = Int]) -> Stats
    let mut s = Stats { count: 0, sum: 0 }
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

- **Forgetting `&mut self` on `next`.**  The iterator advances
  state; the method must take a mutable receiver.  A `&self`
  signature won't satisfy the trait.
- **Forgetting `type Item = ...`** in the impl block.  This is an
  associated type that must be bound.
- **Returning `Option[T]` where `T` differs from `Self::Item`.**
  The two have to match; the trait machinery doesn't widen.
- **Calling `.iter()` on a user type.**  Today only `Vec`, `HashMap`,
  `HashSet`, and `String` expose `.iter()` as a method.  Your type
  *is* the iterator — there's nothing to "go into iter mode" on.
- **Generic `Iterator`** — `trait Iterator[T] { def next ... }` is
  not Riven's shape.  Use the associated-type form above; it
  matches Rust and the builtin pipeline expects it.

---

## 7. When the iterator state is borrowed

If your iterator borrows from another collection, the lifetime
flows through the impl block:

```riven
class WindowIter['a, T]
  source: &'a Vec[T]
  pos: USize
end

impl['a, T] Iterator for WindowIter['a, T]
  type Item = &'a T

  def next(&mut self) -> Option[&'a T]
    if self.pos >= self.source.len
      return None
    end
    let item = &self.source[self.pos]
    self.pos = self.pos + 1
    Some(item)
  end
end
```

The borrow checker tracks the lifetime of `'a`; the returned `&'a T`
must outlive the iterator value, which itself can't outlive the
source `Vec`.

---

## 8. Where this lives in the compiler

- `Iterator` trait registered in `crates/riven-core/src/resolve/mod.rs`
  (search for `"Iterator"`).
- `FromIterator` trait registered there too.
- Pipeline method resolution in `crates/riven-core/src/typeck/infer.rs`
  (search for `"sum"`, `"fold"`, `"map"`, `"filter"`).
- Runtime `riven_*_from_iter` helpers in
  `crates/riven-core/runtime/runtime.c`.

---

**Next:** browse the [spec index](../specs/README.md) for every
formal contract Riven currently enforces, or jump to
[chapter 14 — FFI](14-ffi.md) for cross-language interop.
