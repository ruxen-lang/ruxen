# Generics

Generics let you write code that works with multiple types while maintaining type safety.

## Generic Functions

Type parameters go in square brackets. **Uppercase identifiers are type parameters; lowercase identifiers are lifetime parameters** (see "Lifetime Parameters" below).

```riven
def identity[T](x: T) -> T
  x
end

let n = identity(42)          # T = Int
let s = identity("hello")    # T = &str
```

## Mixin Bounds

Constrain type parameters with `:`:

```riven
def largest[T: Comparable](list: &Array[T]) -> &T
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

```riven
def log_and_save[T: Displayable + Serializable](item: &T)
  puts item.to_display
  save(item.serialize)
end
```

## Generic Classes

```riven
class Stack[T]
  items: Array[T]

  def init
    self.items = Array.new
  end

  def mut push(item: T)
    self.items.push(item)
  end

  def mut pop -> Option[T]
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

```riven
struct Pair[A, B]
  first: A
  second: B
end

let p = Pair.new(42, "hello")
```

## Generic Enums

```riven
enum Either[L, R]
  Left(L)
  Right(R)
end
```

`Option[T]` and `Result[T, E]` are generic enums built into the language.

## Where Clauses

For complex constraints:

```riven
def merge[A, B, C](left: &A, right: &B) -> C
  where A: Iterable[Item = Int],
        B: Iterable[Item = Int],
        C: FromIterator[Int]
  # ...
end
```

## Conditional Methods (Extensions)

Use an `extension` block to add methods to a generic type — optionally gated by a `where` clause. The class body stays focused on the unconditional surface; conditional methods live in `extension` blocks alongside it.

```riven
# All Containers get this method (unconditional extension)
extension Container[T]
  def count -> Int
    self.items.len
  end
end

# Only Containers of Displayable types get print_all
extension Container[T] where T: Displayable
  def print_all
    for item in self.items
      puts item.to_display
    end
  end
end
```

## Lifetime Parameters

Lifetime parameters appear in the same `[...]` slot as type parameters. **Lowercase identifiers are lifetimes; uppercase identifiers are types.** No sigil.

```riven
def longest[a](x: &a String, y: &a String) -> &a String
  if x.len > y.len
    x
  else
    y
  end
end
```

Lifetimes can mix freely with type parameters:

```riven
class Slice[T, a]
  data: &a Array[T]
  start: USize
  len: USize
end
```

Most of the time, lifetime elision rules handle this automatically and you don't need explicit lifetime annotations. Spell them out only when the compiler asks for them.
