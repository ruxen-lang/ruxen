# Implementing `Iterator` for Your Own Type

You've written a custom collection — a counter, a tree, a streaming buffer — and now you want `.map`, `.filter`, `.collect`, `.fold`, and `for x in ...` to just work on it. That's what the `Iterator` mixin is for. Implement one tiny method (`next`) and you get the whole pipeline for free.

An **iterator** is anything that knows how to produce the next value in a sequence — and how to say "I'm done". You make your type into one by including the `Iterator` mixin and implementing the `next` method. After that, the entire iterator surface (`map`, `filter`, `fold`, `collect`, `sum`, `count`, …) is available because the mixin provides those as defaults.

---

## 1. A minimal iterator

Save as `counter.rx`:

```ruxen
class Counter
  current: Int
  limit: Int

  def init(@limit: Int)
    self.current = 0
  end

  include Iterator

  type Item = Int

  def var next -> Option[Int]
    if self.current >= self.limit
      return nil
    end
    let n = self.current
    self.current = n + 1
    Some(n)
  end
end

def main
  var c = Counter.new(5)
  for n in c
    puts "#{n}"
  end
end
```

Run it:

```bash
ruxen run counter.rx
```

Output:

```
0
1
2
3
4
```

That's the whole pattern. Three pieces inside the class body:

1. `include Iterator` — opts in to the mixin.
2. `type Item = Int` — declares what kind of value the iterator produces. (`Item` is an **associated type** — a type the mixin requires you to bind, much like a required method.)
3. `def var next -> Option[Int]` — returns `Some(value)` for each step, then `nil` to signal end-of-sequence.

The `def var` part says this method mutates `self`. It has to — the iterator's job is to advance its own state.

## 2. What `for` actually does

A `for x in iter` loop expands roughly to:

```ruxen
var __iter = iter
loop
  match __iter.next
    Some(x) -> # loop body with x bound
    nil     -> break
  end
end
```

So anything with a writing `next` method that returns `Option[Item]` can drive a `for` loop. The compiler handles the desugaring; you just write the method.

## 3. The pipeline comes for free

Because `Counter` includes `Iterator`, you immediately get the whole pipeline:

```ruxen
let total = Counter.new(10).fold(0, |acc, x| acc + x)
let evens: Array[Int] = Counter.new(10).filter(|x| x % 2 == 0).collect[Array[Int]]()
puts "total=#{total} evens.size=#{evens.size}"
```

Output:

```
total=45 evens.size=5
```

You wrote `next`; the mixin gives you `map`, `filter`, `fold`, `collect`, `take`, `skip`, `count`, `sum`, `min`, `max`, and more.

## 4. Returning your iterator from a function

The simplest shape — name the type:

```ruxen
def count_to(n: Int) -> Counter
  Counter.new(n)
end
```

If you don't want callers to know the concrete type, return `some Iterator[Item = Int]`:

```ruxen
def count_to(n: Int) -> some Iterator[Item = Int]
  Counter.new(n)
end
```

`some` here means "I return *some* concrete type that satisfies `Iterator[Item = Int]`, but I'm not telling you which one." Callers can still loop over the result and call pipeline methods — they just can't name the type.

## 5. Implementing `FromIterator` — the reverse direction

If you want `.collect[MyType]()` to produce *your* type, implement the `FromIterator` mixin. It's the mirror of `Iterator`: instead of pulling values out one at a time, you take an iterator and consume it.

```ruxen
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

Output:

```
n=5 sum=10
```

`.collect[Stats]()` routes through your `from_iter` automatically.

## 6. When the iterator borrows from a collection

If your iterator hands out references *into* some backing collection, you need a **lifetime parameter** — a placeholder for "how long the borrow is valid". (Lifetimes get a proper treatment in [Chapter 30](30-lifetimes-advanced.md); here's a short preview.)

Lowercase identifiers in `[...]` are lifetimes; uppercase are type parameters:

```ruxen
class WindowIter[T, a]
  source: &a Array[T]
  pos: USize

  def init(@source: &a Array[T])
    self.pos = 0
  end

  include Iterator

  type Item = &a T

  def var next -> Option[&a T]
    if self.pos >= self.source.size
      return nil
    end
    let item = &self.source[self.pos]
    self.pos = self.pos + 1
    Some(item)
  end
end
```

The compiler enforces that `WindowIter` can't outlive the array it points into — `&a T` means "a reference whose lifetime is the same as `a`".

## 7. Common mistakes

- **Forgetting `def var` on `next`.** The iterator has to mutate its state (advance `pos`, etc.). Without `var`, you'll get a type-mismatch error against the mixin's required signature.
- **Forgetting `type Item = ...`** in the class body. `Iterator` won't know what your sequence produces and you'll see "associated type Item is unbound" at compile time.
- **Mismatched return type.** `def var next -> Option[String]` paired with `type Item = Int` is an immediate error — the two must agree.
- **Trying to call `.iter()` on your own type.** Only `Array`, `Map`, `Set`, and `String` expose `.iter()`. Your type *is* the iterator — there's nothing to "enter iter mode" for.
- **Using the wrong shape for the mixin.** It's an associated type (`type Item`), not a generic parameter (`mixin Iterator[T]`). Match what the standard library expects.

> **Try it:** modify `Counter` to count *downward* from `limit` to zero. Then use it with `.collect[Array[Int]]()` and `puts` the result.

---

## Recap

- An iterator is anything with a `def var next -> Option[Item]`.
- `include Iterator` plus `type Item = T` plus `next` is the whole shape.
- You get `map`, `filter`, `collect`, `fold`, `sum`, etc. for free.
- Return your concrete iterator type, or hide it with `some Iterator[Item = ...]`.
- For `.collect[MyType]()` support, implement the `FromIterator` mixin.

**Next:** [Chapter 24 — Async](24-async.md).
