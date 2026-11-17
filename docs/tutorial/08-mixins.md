# Mixins

Some behaviour cuts across types that have nothing else in common. A `User`, a `Robot`, and an `Animal` might all need a "say hello" method, but they aren't really the same kind of thing — making them all inherit from a shared parent would be a stretch. **Mixins** solve exactly this: a way to share method signatures and default bodies across unrelated types.

In Ruby a mixin is a `module` that a class includes, and Ruxen's mixins work the same way. A mixin does two jobs at once:

- **Contract** — declare methods an including type must provide (a signature with no body, the Ruxen addition to Ruby).
- **Provision** — supply default method bodies that the including type gets for free.

The Ruby spelling, `module Greet ... end`, is [planned](../../TODO.md#t-module-mixins). Until it lands, a mixin is declared with the `mixin` keyword, which is [deprecated] (replace with `module`). `include` itself is [implemented]. The runnable examples below use `mixin`, and the classes in them use the constructor `def init`, also [deprecated] until `def initialize` lands [planned](../../TODO.md#t-initialize).

## A first runnable example

The Ruby form [planned](../../TODO.md#t-module-mixins):

```ruby
module Greet
  def hello -> String
end

class Dog
  include Greet

  name: String

  def initialize(@name: String)
  end

  def hello -> String
    "Woof! I'm #{@name}"
  end
end

d = Dog.new("Rex")
puts d.hello
```

Works today, with `mixin` and `def init` [deprecated]:

```ruby
mixin Greet
  def hello -> String
end

class Dog
  name: String

  def init(@name: String)
  end

  include Greet

  def hello -> String
    "Woof! I'm #{@name}"
  end
end

d = Dog.new("Rex")
puts d.hello
```

```bash
ruxen compile mixin_demo.rx
./mixin_demo
```

Output:

```text
Woof! I'm Rex
```

The mixin `Greet` declared one required method (`hello`); the class `Dog` adopted it with `include Greet` and supplied a body. From this point on, anyone holding a `Dog` can call `.hello`.

## Defining a mixin

The Ruby form [planned](../../TODO.md#t-module-mixins):

```ruby
module Renderable
  def render -> String
end
```

A bare signature (no body) is a **required method** — any type that includes `Renderable` must define `render` [implemented] in `mixin` bodies.

### Default methods

A mixin can also supply default bodies. Including types get the defaults for free and may override them. In Ruby a default calls another method of the mixin with a bare `name` [planned](../../TODO.md#t-implicit-self):

```ruby
module Greeter
  def name -> String                  # required
  def greet = "Hello, #{name}!"       # default
end
```

Works today: a `def ... end` default that calls `self.name`, in a `mixin` [deprecated] (replace with `module`; an endless default or a bare `name` call does not compile yet):

```ruby
mixin Greeter
  def name -> String                  # required
  def greet -> String                 # default
    "Hello, #{self.name}!"
  end
end

class Bot
  nm: String

  def init(@nm: String)
  end

  include Greeter

  def name -> String
    @nm.clone
  end
end

b = Bot.new("Riv")
puts b.greet        # Hello, Riv!
```

`Bot` supplied `name`; `greet` came along for the ride from the default in `Greeter`.

### Mixins that build on other mixins

One mixin can require another. A type including the child must satisfy both contracts. The Ruby form includes the parent module inside the child [planned](../../TODO.md#t-module-mixins):

```ruby
module A
  def a_msg -> String
end

module B
  include A
  def b_msg -> String
end
```

Works today: mixin inheritance with `mixin B: A` [deprecated] (replace with `include A` inside `module B`; an `include` inside a `mixin` body does not parse; `module` mixins are [planned](../../TODO.md#t-module-mixins)):

```ruby
mixin A
  def a_msg -> String
end

mixin B: A
  def b_msg -> String
end

class Thing
  name: String

  def init(@name: String)
  end

  include A

  def a_msg -> String
    "A: #{@name}"
  end

  include B

  def b_msg -> String
    "B: #{@name}"
  end
end

t = Thing.new("x")
puts t.a_msg
puts t.b_msg
```

One `include` names one mixin today; `include A, B` is [planned](../../TODO.md#t-include-many).

## Using mixins as bounds on generics

Mixins double as **bounds** — a way to constrain a generic type parameter to "any type that includes this mixin." The Ruby form is the RBS-style bound `[T < Greater]` [planned](../../TODO.md#t-d7-respell), and teaching an existing type a new mixin is done by reopening its class [planned](../../TODO.md#t-reopen-class):

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

puts max(7, 3)        # 7
```

Works today: `extension`, the `[T: Greater]` bound and `mixin` [deprecated] (replace with reopening the class, `[T < Greater]` and `module`):

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

m = max(7, 3)
puts m        # 7
```

Two things happening:

- `extension Int ... include Greater ... end` adds the `Greater` mixin to a type we don't own. Reopening the class (`class Int ... end`) replaces it; today a second `class Int` collides with the stdlib type (E0727).
- `def max[T: Greater]` says "`T` can be any type, as long as it includes `Greater`."

Several bounds combine with `&` in the Ruby form, `[T < Comparable & Hashable]` [planned](../../TODO.md#t-d7-respell); the `T: A + B` spelling is [deprecated].

## Bounds in a `where` clause

When the constraints get long, older code lifts them into a `where` clause. In the Ruby form they stay in the `[...]` list [planned](../../TODO.md#t-d7-respell):

```ruby
def merge[A < Showable, B < Showable](x: A, y: B) -> String
  "#{x.show}+#{y.show}"
end
```

Works today: a `where` clause, with `extension` and `mixin` [deprecated] (replace with bounds in the `[...]` list, reopening and `module`):

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

puts merge(1, 2)         # 1+2
```

## `some Mixin` and `any Mixin`

When a method takes "something that includes a mixin" as a parameter, the language has two flavours (whether both stay is an open question [planned](../../TODO.md#t-oq)):

- **`some Mixin`** — at each call site, the compiler picks one concrete conforming type. Different calls can use different types, but inside one call the type is fixed. Zero runtime cost; the compiler specializes the method per type used.
- **`any Mixin`** — the value carries a small dispatch table at runtime, and one method body handles every conforming type. This is what lets you put different types in the same collection.

Both parse and type-check as parameter types today, but a program that calls a mixin method through them does not build: MIR lowering stops with "cannot lower method `render` on unresolved mixin-bound receiver `some Renderable`: no concrete monomorphization reached this call", a [known issue](../../TODO.md#t-some-any-lower). `some Mixin` as a return type is rejected earlier, in typeck [planned](../../TODO.md#t-some-return). The two shapes, for reference:

```text
def print_it(item: &some Renderable)    # one concrete type per call
def shout(member: &any Greeter)         # runtime dispatch
```

Works today: a generic method with a mixin bound does the `some` job, specializing per type. With `mixin`, the colon bound and `def init` [deprecated] (replace with `module`, `[T < Renderable]` and `def initialize`):

```ruby
mixin Renderable
  def render -> String
end

class User
  name: String
  def init(@name: String)
  end
  include Renderable
  def render -> String
    "user #{@name}"
  end
end

class Robot
  id: Int
  def init(@id: Int)
  end
  include Renderable
  def render -> String
    "robot #{@id}"
  end
end

def print_it[T: Renderable](item: &T)
  puts item.render
end

# Same method, called with two different types — compiler specializes each call:
print_it(&User.new("Alice"))
print_it(&Robot.new(42))
```

Use a bounded generic when you have one concrete type per call (the common case). Runtime dispatch over mixed types in one container (`Box[any Mixin]`) waits on the known issue above.

## Methods that exist only for some type arguments

You can give a generic class a method that exists only when its type parameter meets a bound. The Ruby form reopens the class with a bound [planned](../../TODO.md#t-oq) (the exact rule is open question OQ-6):

```ruby
class Container[T < Showable]
  def print_all
    @items.each { |item| puts item.show }
  end
end
```

Works today: a method `where` bound inside the class, with `extension`, `mixin` and `def init` [deprecated] (replace with the bounded reopen above). A method with a `where` bound needs an explicit return type:

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

A `Container[Int]` (since `Int` includes `Showable`) picks up `print_all`. A `Container[SomeOtherType]` that doesn't include `Showable` doesn't have it. The block-level form `extension Container[T] where T: Showable ... end` does not parse (reopening with a bound is [planned](../../TODO.md#t-reopen-class)); `extension Type ... end` without a bound adds methods unconditionally and is [deprecated] (replace with reopening the class).

## Implicit mixins on value types

Some everyday mixins are **implicitly included** when every field supports them, with no `include` line needed. Today they carry Rust names: `Debug`, `Clone`, `Eq`, `Hashable`, `Default`, `Ord`, `PartialOrd`, and (on structs) `Copy`. Works today: a `struct` [deprecated] (replace with `Data.define`, Chapter 6):

```ruby
struct Point
  x: Float
  y: Float
end

# Point automatically supports copying, equality, ordering, hashing
# and a default value — because every field does.
```

Scalars and `String` will include the Ruby-named `Comparable`, `Hashable` and `Clone` (and `Default` where Ruby has a zero), with `==`, `<=>`, `to_s` and `inspect` in `.rx`; the Rust-named mixins stay as [deprecated] adapters [planned](../../TODO.md#t-d6). A generic bound that needs only equality will check that the type has a `==` method, with no marker mixin [planned](../../TODO.md#t-x1).

Two related markers, `Send` and `Sync`, control whether a value is safe to send between threads or share between them. They belong to ownership, not to the object protocol. Opt out with `include !Send`, or opt in for a hand-checked type with `unsafe include Send` [implemented]; whether the markers are inferred from a type's fields or written by hand is still open (`exclude Send` is a proposed spelling) [planned](../../TODO.md#t-exclude-send). We come back to this in the concurrency chapter (Chapter 21).

## The object protocol: Ruby names

Ruby's method names replace the Rust trait vocabulary. Define the method and the matching behaviour follows:

| Purpose | Ruby form | Status | Replaces ([deprecated]) |
|---------|-----------|--------|-------------------------|
| Friendly text (used by `puts`) | `def to_s -> String` | [implemented]; interpolation `"#{x}"` still prints the default for a user class, a [known issue](../../TODO.md#t-interp-to-s) | `Display` |
| Developer text (used by `p`) | `def inspect -> String` | [planned](../../TODO.md#t-d9) | `Debug` |
| Equality | `def ==(other: &Self) -> Bool` | [implemented] | `Eq`, `PartialEq` |
| Ordering | `include Comparable` and `def <=>` | [implemented] | `Ord`, `PartialOrd`, `cmp` |
| Hash key or `Set` element | `def hash -> Int` and `def eql?` | [planned](../../TODO.md#t-d8) | `Hashable` |
| Explicit copy | `clone` | [implemented] (`Clone` stays as the ownership marker) | — |
| Default value | arguments with defaults | [implemented] | `Default`, `.default` |
| Cleanup | `def drop` | [implemented] | — |
| Errors | `class MyError < StandardError` | [planned](../../TODO.md#t-exceptions) | `Error` with `def message` |
| Iteration | `include Enumerable[T]` and `def each` | [implemented]; plain `include Enumerable` [planned](../../TODO.md#t-enumerable-infer) | `Iterator` |

`Copy`, `Clone`, `Drop`, `Send` and `Sync` are ownership markers (Chapter 4), not part of this list.

## Common mistakes

**Forgetting to implement a required method.** If `include Mixin` declares `def foo` but the class doesn't supply one, the compiler refuses to build. Read the error — it tells you which method is missing.

**Two mixins, one default.** Ruxen will follow Ruby's rule: when two included mixins define the same method, the one included last wins, and the class's own method beats both [planned](../../TODO.md#t-oq). Older drafts made this an error. Until the rule is settled, define the method explicitly in the class.

**Trying to use `any Mixin` for a method that takes `self` by value.** A mixin is usable through `any` only if its methods can be dispatched through a runtime table — that rules out methods that return `Self` by value or consume `self`. Use `some Mixin` (compile-time) when the methods don't fit `any`.

**Reaching for `extension`.** The Ruby way to add methods to an existing class is to reopen it (`class String ... end`) [planned](../../TODO.md#t-reopen-class). Until then, `extension Type ... end` works and is [deprecated].

## Try it

Define a mixin `Animal` with one required method `species -> String` and one default `describe -> String` that returns `"a #{self.species}"`. Then make `class Dog` and `class Spider` both include `Animal`. Print `dog.describe` and `spider.describe` and observe the default doing its job.

Then add a method `def announce[T: Animal](a: &T)` that prints `a.describe`. Call it with both your `Dog` and `Spider` values. (When RBS bounds land, write `[T < Animal]`.)

## Recap

- A mixin is both a contract (required methods) and a provision (default methods). Its Ruby spelling is `module` (planned); `mixin` works today.
- `include MixinName` inside a type body adopts the contract.
- Mixins double as bounds: `[T < Mixin]` (planned; `[T: Mixin]` today) says `T` must include the mixin.
- `some Mixin` is compile-time dispatch and `any Mixin` runtime dispatch; calls through either fail to build today, so use a bounded generic `[T: Mixin]`.
- Value types implicitly pick up common mixins when every field qualifies. The Ruby method names (`to_s`, `==`, `<=>`, `hash`) are the protocol.

**Next:** [Closures and Blocks](09-closures-and-blocks.md) — anonymous functions you pass around as values.
