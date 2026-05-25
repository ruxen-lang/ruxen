# Generics

Imagine you've just written a function that returns the larger of two `Int`s:

```ruxen
def max_int(a: Int, b: Int) -> Int
  if a > b
    a
  else
    b
  end
end
```

Now you want the same thing for `Float`. And for `String`. Copy-paste, change the types, repeat. That's the pain **generics** fix: write the logic once and let the compiler stamp out a version for whichever type you need.

A function or class with type parameters in `[...]` is a **generic** — a recipe. When you use it with a specific type (`max[Int]`, `Box[String]`), the compiler produces the concrete version.

## A first runnable example

```ruxen
def identity[T](x: T) -> T
  x
end

def main
  let n = identity(42)
  puts "#{n}"          # 42
end
```

```bash
ruxen compile generics_demo.rx
./generics_demo
```

Output:

```
42
```

`T` is a **type parameter** — a placeholder. Inside `identity`, `T` could be any type; at the call site, the compiler picks it from the argument.

## Generic functions with bounds

The identity function works on *any* type because it doesn't actually do anything with its argument. To use `>`, `==`, or any method, you need to tell the compiler "T must support this." That's a **mixin bound**, written `[T: Mixin]`:

```ruxen
mixin Greater
  def greater_than(other: &Self) -> Bool
end

extension Int
  include Greater

  def greater_than(other: &Int) -> Bool
    self > *other
  end
end

def max[T: Greater](a: T, b: T) -> T
  if a.greater_than(&b)
    a
  else
    b
  end
end

def main
  puts "#{max(7, 3)}"      # 7
end
```

The bound `T: Greater` says "I'll accept any `T`, as long as it includes the `Greater` mixin." Inside `max` you can call `.greater_than` on any `T`. We taught `Int` the mixin with an `extension` block.

### Multiple bounds

Use `+` to require several mixins:

```ruxen
def log_and_save[T: Display + Serializable](item: &T)
  puts "#{item}"
  save(item.serialize)
end
```

## Generic classes

A class with `[T]` is a recipe — `Box[Int]` and `Box[String]` are concrete types produced from it.

```ruxen
class Box[T]
  value: T

  def init(@value: T)
  end

  def get -> &T
    &self.value
  end
end

def main
  let b = Box[Int].new(42)
  puts "#{b.get}"
end
```

A common pattern is a generic container:

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

  def is_empty -> Bool
    self.items.is_empty
  end
end
```

## Generic structs

Structs follow the same rule:

```ruxen
struct Pair[A, B]
  first: A
  second: B
end

def main
  let p = Pair.new(42, String.from("hello"))
  puts "#{p.first}"
  puts "#{p.second}"
end
```

## Generic enums

You've already used the two most important generic enums in the language:

```ruxen
enum Option[T]
  Some(T)
  nil
end

enum Result[T, E]
  Ok(T)
  Err(E)
end
```

You can define your own:

```ruxen
enum MyOpt[T]
  Has(T)
  Empty
end
```

## `where` clauses

When bounds get long, lift them out of the parameter list with `where`:

```ruxen
mixin Showable
  def to_display -> String
end

extension Int
  include Showable

  def to_display -> String
    "#{self}"
  end
end

def merge[A, B](x: A, y: B) -> String
  where A: Showable,
        B: Showable
  "#{x.to_display}+#{y.to_display}"
end

def main
  puts merge(1, 2)     # 1+2
end
```

## Conditional methods (extensions with `where`)

Add methods to a generic type only when the type parameter satisfies a bound:

```ruxen
extension Container[T]
  def count -> Int
    self.items.len
  end
end

extension Container[T] where T: Showable
  def print_all
    for item in self.items
      puts item.to_display
    end
  end
end
```

`Container[Int]` (since `Int: Showable`) gets `print_all`. A `Container[SomeOtherType]` without `Showable` doesn't have it — calling `print_all` is a compile error explaining the missing bound.

## Lifetime parameters (short preview)

When you start returning references from generic functions, you may see the compiler ask you to name a **lifetime** — a label that says "this reference is valid for at least as long as this other reference." Lifetime parameters share the same `[...]` slot as type parameters:

- **Uppercase identifiers** are type parameters: `T`, `U`, `Item`.
- **Lowercase identifiers** are lifetime parameters: `a`, `input`.

```ruxen
def longest[a](x: &a String, y: &a String) -> &a String
  if x.len > y.len
    x
  else
    y
  end
end
```

The `a` here doesn't mean "duration `a`" — it's just a name. The signature says "both inputs share the same lifetime `a`, and the return value lives at least that long."

Most of the time you won't need to write lifetimes explicitly — the compiler infers them from a small set of common rules. Spell them out only when asked. Chapter 30 goes deeper.

## Common mistakes

**Calling a method on a generic type without a bound.**

```ruxen
def max[T](a: T, b: T) -> T
  if a > b      # ERROR: T doesn't necessarily support >
    a
  else
    b
  end
end
```

Add `T: PartialOrd` (or your custom bound) so the compiler knows `>` is allowed.

**Stamping out the same generic everywhere.** Each distinct concrete type produces a separate compiled version. That's usually fine — it's how generics stay fast — but if you find yourself wanting *runtime* polymorphism (one collection holding many types), use `any Mixin` (see Chapter 8).

**Mistaking `T` for a "magic any-type."** `T` inside a function body has *exactly* the abilities its bounds give it. If `T` has no bounds, you can copy it, move it, store it — and not much else.

**Writing lifetimes too early.** You almost never need lifetime annotations until the compiler asks for them. Wait for the error, then add the smallest annotation that fixes it.

## Try it

Turn the `Stack[T]` example from earlier into a working program:

```ruxen
def main
  var s: Stack[Int] = Stack.new
  s.push(1)
  s.push(2)
  s.push(3)
  match s.pop
    Some(v) -> puts "popped #{v}"
    nil     -> puts "empty"
  end
end
```

Then try `Stack[String]`. Same class, different concrete type.

## Recap

- Generics let you write code once and use it across types.
- Type parameters go in `[...]`. Uppercase = type, lowercase = lifetime.
- A **bound** (`T: Mixin`) constrains what operations `T` supports.
- `where` lifts bounds out of the parameter list when they get long.
- `extension Type[T] where ...` adds methods conditionally.
- For runtime polymorphism (mixed types in one collection), reach for `any Mixin` (Chapter 8).

**Next:** [Collections](13-collections.md) — the standard `Array`, `Map`, and `Set` types in everyday use.
