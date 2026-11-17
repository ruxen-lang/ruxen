# Generics

Imagine you've just written a method that returns the larger of two `Int`s [implemented]:

```ruby
def max_int(a: Int, b: Int) -> Int
  if a > b
    a
  else
    b
  end
end

puts max_int(7, 3)
```

Now you want the same thing for `Float`. And for `String`. Copy-paste, change the types, repeat. That's the pain **generics** fix: write the logic once and let the compiler stamp out a version for whichever type you need.

A method or class with type parameters in `[...]` is a **generic** — a recipe. When you use it with a specific type (`max(7, 3)`, `Box[String]`), the compiler produces the concrete version. Type parameters are the Ruxen addition to Ruby: Ruby has no annotations, and Ruxen adds only the ones it needs.

## A first runnable example

Save this as `generics_demo.rx` [implemented]:

```ruby
def identity[T](x: T) -> T
  x
end

n = identity(42)
puts n          # 42
```

```bash
ruxen compile generics_demo.rx
./generics_demo
```

Output:

```text
42
```

`T` is a **type parameter** — a placeholder. Inside `identity`, `T` could be any type; at the call site, the compiler picks it from the argument.

## Generic methods with bounds

The identity method works on *any* type because it doesn't actually do anything with its argument. To call a method on a `T`, you need to tell the compiler "T must support this." That's a **mixin bound**. The Ruby form is the RBS-style bound `[T < Mixin]` [planned](../../TODO.md#t-d7-respell), with the mixin declared as a `module` [planned](../../TODO.md#t-module-mixins) and `Int` taught the mixin by reopening its class [planned](../../TODO.md#t-reopen-class):

```ruby
module Greater
  def greater_than(other: &Self) -> Bool
end

class Int
  include Greater

  def greater_than(other: &Int) -> Bool
    self > *other
  end
end

def max[T < Greater](a: T, b: T) -> T
  a.greater_than(&b) ? a : b
end

puts max(7, 3)      # 7
```

Works today: `mixin`, `extension` and the colon bound `[T: Greater]` [deprecated] (replace with `module`, reopening and `[T < Greater]`):

```ruby
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

puts max(7, 3)      # 7
```

The bound says "I'll accept any `T`, as long as it includes the `Greater` mixin." Inside `max` you can call `.greater_than` on any `T`. We taught `Int` the mixin with an `extension` block.

The everyday bound for ordering will be Ruby's `Comparable`. Today `Int` and `String` do not include it, so `[T: Comparable]` rejects `max(7, 3)` (E1015); scalars and `String` will include `Comparable`, `Hashable` and `Clone` [planned](../../TODO.md#t-d6).

### Multiple bounds

The Ruby form joins bounds with `&`: `[T < Greater & Clone]` [planned](../../TODO.md#t-d7-respell). Works today: `+` in a colon bound [deprecated] (replace with `[T < Greater & Clone]`):

```ruby
mixin Greater
  def greater_than(other: &Self) -> Bool
end

extension Int
  include Greater

  def greater_than(other: &Int) -> Bool
    self > *other
  end
end

def max[T: Greater + Clone](a: T, b: T) -> T
  a.greater_than(&b) ? a : b
end

puts max(7, 3)
```

## Generic classes

A class with `[T]` is a recipe — `Box[Int]` and `Box[String]` are concrete types produced from it. Works today, with the constructor spelled `def init` [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class Box[T]
  value: T

  def init(@value: T)
  end

  def get -> &T
    &@value
  end
end

b = Box[Int].new(42)
puts b.get
```

A common pattern is a generic container. Works today, with `def init` [deprecated]:

```ruby
class Stack[T]
  items: Array[T]

  def init
    @items = []
  end

  def var push(item: T)
    @items.push(item)
  end

  def var pop -> T?
    @items.pop
  end

  def empty? -> Bool
    @items.empty?
  end
end
```

## Generic value types

Value types follow the same rule. The Ruby form for value types is `Data.define` [planned](../../TODO.md#t-data-define) (its generic spelling is still open). Works today: a generic `struct` [deprecated] (replace with `Data.define`):

```ruby
struct Pair[A, B]
  first: A
  second: B
end

p = Pair.new(42, "hello")
puts p.first
puts p.second
```

## Generic tagged types

`T?` is the generic "value or nothing" type built into the language (Chapter 7), and exceptions replace the generic `Result[T, E]` [planned](../../TODO.md#t-exceptions). You can define your own generic `enum`. Works today [deprecated] (replace with `T?` or `Data` variants):

```ruby
enum MyOpt[T]
  Has(T)
  Empty
end

a = MyOpt.Has(42)
```

## `where` clauses

Older code lifts long bounds out of the parameter list with `where`. In the Ruby form they stay in the `[...]` list [planned](../../TODO.md#t-d7-respell):

```ruby
def merge[A < Showable, B < Showable](x: A, y: B) -> String
  "#{x.show}+#{y.show}"
end
```

Works today: a `where` clause, with `mixin` and `extension` [deprecated] (replace with bounds in the `[...]` list):

```ruby
mixin Showable
  def show -> String
end

extension Int
  include Showable

  def show -> String
    "#{self}"
  end
end

def merge[A, B](x: A, y: B) -> String
  where A: Showable,
        B: Showable
  "#{x.show}+#{y.show}"
end

puts merge(1, 2)     # 1+2
```

## Conditional methods

You can add a method to a generic class that exists only when the type parameter satisfies a bound. The Ruby form reopens the class with a bound [planned](../../TODO.md#t-oq) (open question OQ-6):

```ruby
class Container[T < Showable]
  def print_all
    @items.each { |item| puts item.show }
  end
end
```

Works today: a method `where` bound inside the class, with `mixin`, `extension` and `def init` [deprecated] (replace with the bounded reopen). It needs an explicit return type:

```ruby
mixin Showable
  def show -> String
end

extension Int
  include Showable

  def show -> String
    "#{self}"
  end
end

class Container[T]
  items: Array[T]

  def init(@items: Array[T])
  end

  def print_all -> nil where T: Showable
    @items.each { |item| puts item.show }
  end
end

c = Container[Int].new([1, 2])
c.print_all()
```

`Container[Int]` (since `Int` includes `Showable`) gets `print_all`. A `Container[SomeOtherType]` without `Showable` doesn't have it. The block form `extension Container[T] where T: Showable ... end` does not parse (reopening with a bound is [planned](../../TODO.md#t-reopen-class)); use the method bound above.

## Lifetime parameters (short preview)

When you start returning references from generic methods, you may see the compiler ask you to name a **lifetime** — a label that says "this reference is valid for at least as long as this other reference." Lifetime parameters share the same `[...]` slot as type parameters:

- **Uppercase identifiers** are type parameters: `T`, `U`, `Item`.
- **Lowercase identifiers** are lifetime parameters: `a`, `input`.

[implemented]:

```ruby
def longest[a](x: &a String, y: &a String) -> &a String
  if x.size > y.size
    x
  else
    y
  end
end

puts longest("hello", "hi")
```

The `a` here doesn't mean "duration `a`" — it's just a name. The signature says "both inputs share the same lifetime `a`, and the return value lives at least that long." The Rust sigil `'a` is [retired]: it lexes as an unterminated string (E0002).

Most of the time you won't need to write lifetimes explicitly — the compiler infers them from a small set of common rules. Spell them out only when asked. Chapter 30 goes deeper.

## Common mistakes

**Calling a method on a generic type without a bound.** Rejected today [implemented]:

```ruby
def describe[T](x: T) -> String
  x.label       # ERROR: no field or method `label` on type `T`
end

puts describe(3)
```

Add a bound on a mixin that declares `label` (`[T: Labeled]` today, `[T < Labeled]` once RBS bounds land) so the compiler knows the method is there.

**Stamping out the same generic everywhere.** Each distinct concrete type produces a separate compiled version. That's usually fine — it's how generics stay fast — but if you find yourself wanting *runtime* polymorphism (one collection holding many types), `any Mixin` is the planned tool, but calls through it fail to build today (see Chapter 8).

**Mistaking `T` for a "magic any-type."** `T` inside a method body has *exactly* the abilities its bounds give it. If `T` has no bounds, you can copy it, move it, store it — and not much else. Today a call with arguments on an unbounded `T` is not rejected by the type checker [known issue](../../TODO.md#t-unbounded-generic-call).

**Writing lifetimes too early.** You almost never need lifetime annotations until the compiler asks for them. Wait for the error, then add the smallest annotation that fixes it.

## Try it

Turn the `Stack[T]` example from earlier into a working program by adding these lines below it. Works today, with `if let` [deprecated] (replace with `if (v = s.pop)` [planned](../../TODO.md#t-narrowing)):

```ruby
class Stack[T]
  items: Array[T]

  def init
    @items = []
  end

  def var push(item: T)
    @items.push(item)
  end

  def var pop -> T?
    @items.pop
  end

  def empty? -> Bool
    @items.empty?
  end
end

s = Stack[Int].new
s.push(1)
s.push(2)
s.push(3)
if let Some(v) = s.pop
  puts "popped #{v}"
end
puts s.empty?
```

Then try `Stack[String]`. Same class, different concrete type.

## Recap

- Generics let you write code once and use it across types.
- Type parameters go in `[...]`. Uppercase = type, lowercase = lifetime.
- A **bound** constrains what operations `T` supports: `[T < Mixin]` is planned, `[T: Mixin]` works today.
- `where` clauses are deprecated; bounds belong in the `[...]` list.
- Conditional methods use a method `where` bound today, a bounded reopen later.
- Runtime polymorphism (mixed types in one collection) is `any Mixin`, which has a known build failure today (Chapter 8).

**Next:** [Collections](13-collections.md) — the standard `Array`, `Hash`, and `Set` types in everyday use.
