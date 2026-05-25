# Generics

Generics let you write code that works with multiple types while maintaining type safety.

## Generic Functions

Type parameters go in square brackets. **Uppercase identifiers are type parameters; lowercase identifiers are lifetime parameters** (see "Lifetime Parameters" below).

```ruxen
def identity[T](x: T) -> T
  x
end

let n = identity(42)          # T = Int
let s = identity("hello")    # T = &str
```

## Mixin Bounds

Constrain type parameters with `:`:

```ruxen
def largest[T: Ord](list: &Array[T]) -> &T
  var best = &list[0]
  for item in list
    if item > best
      best = item
    end
  end
  best
end
```

### Multiple Bounds

Use `+` for multiple mixin requirements:

```ruxen
def log_and_save[T: Display + Serializable](item: &T)
  puts "#{item}"
  save(item.serialize)
end
```

## Generic Classes

```ruxen
class Stack[T]
  items: Array[T]

  def init
    self.items = Array.new
  end

  def var push(item: T)
    self.items.push(item)
  end

  def var pop -> Option[T]
    self.items.pop
  end

  def peek -> Option[&T]
    if self.items.is_empty
      nil
    else
      Some(&self.items[self.items.len - 1])
    end
  end

  def is_empty -> Bool
    self.items.is_empty
  end
end
```

## Generic Structs

```ruxen
struct Pair[A, B]
  first: A
  second: B
end

let p = Pair.new(42, "hello")
```

## Generic Enums

```ruxen
enum Either[L, R]
  Left(L)
  Right(R)
end
```

`Option[T]` and `Result[T, E]` are generic enums built into the language.

## Where Clauses

For complex constraints:

<!-- TODO(migration): canonical spec §3.4a discourages per-method `where` clauses on individual `def`s (re-group into an extension block). Top-level functions with `where` are shown here pending a clarifying spec rule. -->

```ruxen
def merge[A, B, C](left: &A, right: &B) -> C
  where A: Iterator[Item = Int],
        B: Iterator[Item = Int],
        C: FromIterator[Item = Int]
  # ...
end
```

## Conditional Methods (Extensions)

Use an `extension` block to add methods to a generic type — optionally gated by a `where` clause. The class body stays focused on the unconditional surface; conditional methods live in `extension` blocks alongside it.

```ruxen
# All Containers get this method (unconditional extension)
extension Container[T]
  def count -> Int
    self.items.len
  end
end

# Only Containers of Display types get print_all
extension Container[T] where T: Display
  def print_all
    for item in self.items
      puts "#{item}"
    end
  end
end
```

## Lifetime Parameters

Lifetime parameters appear in the same `[...]` slot as type parameters. **Lowercase identifiers are lifetimes; uppercase identifiers are types.** No sigil.

```ruxen
def longest[a](x: &a String, y: &a String) -> &a String
  if x.len > y.len
    x
  else
    y
  end
end
```

Lifetimes can mix freely with type parameters:

```ruxen
class Slice[T, a]
  data: &a Array[T]
  start: USize
  len: USize
end
```

Most of the time, lifetime elision rules handle this automatically and you don't need explicit lifetime annotations. Spell them out only when the compiler asks for them.
