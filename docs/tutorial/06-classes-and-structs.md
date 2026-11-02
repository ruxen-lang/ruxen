# Classes and Value Types

This chapter is about defining your own types. Ruxen, like Ruby, offers two flavours:

- **Classes** — heap-allocated objects with identity, support inheritance, behave like objects in Ruby.
- **Value types** — small bundles of data without identity: Ruby's `Struct` (mutable) and `Data` (immutable).

Pick a value type when you have a small bundle of data (a `Point`, a `Color`); pick a class when you have an entity with identity, behaviour, and possibly child types.

A note before the examples. Ruby's constructor is `def initialize`, which is [planned](../../TODO.md#t-initialize). Until it lands, the constructor is spelled `def init` [deprecated] (replace with `def initialize`), so the runnable examples in this chapter use `def init`.

## A first runnable example

Save this as `box.rx`. Works today, with the constructor spelled `def init` [deprecated]:

```ruby
class Box
  x: Int

  def init(@x: Int)
  end

  def value -> Int
    @x
  end

  def doubled -> Int
    @x * 2
  end
end

b = Box.new(21)
puts b.value
puts b.doubled
```

```bash
ruxen compile box.rx
./box
```

Output:

```text
21
42
```

You defined a class with one field, gave it a constructor, gave it two methods, built an instance with `Box.new`, and called methods on it with dot syntax.

## Anatomy of a class

The Ruby form, with `initialize` [planned](../../TODO.md#t-initialize) and an `attr_reader` accessor [planned](../../TODO.md#t-attr-accessors):

```ruby
class User
  name: String
  age: Int
  attr_reader :name

  def initialize(@name: String, @age: Int)
  end

  def display -> String
    "#{@name} (age #{@age})"
  end
end

u = User.new("Ada", 36)
puts u.display
puts u.name
```

Works today, with `def init` [deprecated] (replace with `def initialize`) and no `attr_reader`, because fields are readable from outside by default:

```ruby
class User
  name: String
  age: Int

  def init(@name: String, @age: Int)
  end

  def display -> String
    "#{@name} (age #{@age})"
  end
end

u = User.new("Ada", 36)
puts u.display     # Ada (age 36)
puts u.name        # Ada
```

- **Field declarations** sit at the top: `name: String`, `age: Int`. The type annotation is the Ruxen addition to Ruby.
- **The constructor** is called automatically when you write `User.new(...)`. Always write one today. A class with fields and no constructor will get an automatic one that takes the fields positionally (`User.new("Ada", 36)`) or as keywords (`User.new(name: "Ada", age: 36)`) [planned](../../TODO.md#t-positional-new); today the positional call type-checks but fails to link (undefined `_User_init`).
- **`@name` in the parameter list** is shorthand for "assign this parameter to the field `@name`". Without the `@`, you'd write `@name = name` inside the constructor.
- **`@name`** reads or writes the field inside a method. The older `self.name` form is [deprecated]: replace it with `@name`.
- **`self`** inside a method is the receiver — the instance the method was called on.

### Field visibility

In Ruby, instance variables are private and you reach them from outside through accessors such as `attr_reader :name` [planned](../../TODO.md#t-ivar-privacy). Today, declared fields are **public by default** — anyone with a `User` can read `u.name`.

`private` and `protected` section markers inside the class body mark what comes after them, until the next marker. The markers are accepted [implemented]; the access rule is not enforced yet:

```ruby
class Account
  name: String              # public field

  private
  audit_id: Int             # private — for Account's own use

  protected
  internal_notes: String    # shared with related classes
end
```

Today a field or method after `private` or `protected` can still be read or called from outside the class (`a.audit_id` compiles and runs), a [known issue](../../TODO.md#t-visibility-enforce). Ruby's rule for `protected` (callable with an explicit receiver of the same class family, from inside it) is [planned](../../TODO.md#t-protected); older drafts said "subclasses only".

## Method modes (the four flavours)

Every method has a relationship with its receiver — does it read, edit, consume, or skip the receiver entirely? You met these briefly in Chapter 4; here they are in a single class. Works today, with `def init` [deprecated]:

```ruby
class Account
  balance: Int

  def init(@balance: Int)
  end

  # Reading — borrows self read-only
  def get_balance -> Int
    @balance
  end

  # Writing — borrows self writably (allowed to change fields)
  def var deposit(amount: Int)
    @balance = @balance + amount
  end

  # Consuming — takes ownership; the original variable is gone after the call
  def consume close -> Int
    @balance
  end

  # Class method — no receiver, called on the type itself
  def self.create(initial: Int) -> Account
    Account.new(initial)
  end
end

a = Account.create(100)
a.deposit(50)
puts a.get_balance          # 150

final_balance = a.close     # a is consumed here
puts final_balance          # 150
# a is no longer usable
```

Note the call site is identical for reading and writing methods — `a.deposit(50)` looks like `a.get_balance`. The mode is part of the method's *declaration*, not its call.

## Inheritance

A class may inherit from one parent with `<`. The child gets all the parent's fields and methods, and may override any of them. Works today, with `def init` [deprecated]:

```ruby
class Animal
  name: String

  def init(@name: String)
  end

  def speak -> String
    "..."
  end
end

class Cat < Animal
  def init(name: String)
    super(name)
  end

  def speak -> String
    "Meow! I'm #{@name}"
  end
end

c = Cat.new("Whiskers")
puts c.speak
```

`super(...)` inside the child's constructor calls the parent's constructor; `Cat` uses it to delegate name storage back to `Animal` [implemented]. Calling `super` from any other overriding method is [planned](../../TODO.md#t-super) (E1127 today).

Single inheritance only. For sharing behaviour without an inheritance relationship, use a **mixin** ([Chapter 8](08-mixins.md)).

## Value types

A **value type** is a no-frills bundle of fields: no identity, no inheritance, no overrides. The Ruby forms are `Data.define` (immutable) [planned](../../TODO.md#t-data-define) and `Struct.new` (mutable) [planned](../../TODO.md#t-struct-new):

```ruby
Point = Data.define(x: Float, y: Float)     # immutable value
Pixel = Struct.new(x: Int, y: Int)          # mutable value

p = Point.new(x: 3.0, y: 4.0)
puts "#{p.x} #{p.y}"
q = p.with(y: 5.0)                          # a changed copy
```

Works today: the `struct` keyword [deprecated] (replace with `Data.define` or `Struct.new`):

```ruby
struct Point
  x: Float
  y: Float
end

p = Point.new(3.0, 4.0)
puts "#{p.x} #{p.y}"
```

A `struct` gets a constructor named `.new` that takes the fields in declaration order — no constructor needed for the common case. You can still define methods on it with `def` just like a class. Printing a plain `struct` with `p` fails to compile today, a [known issue](../../TODO.md#t-struct-typeck).

### Value types are usually Copy

If every field of a value type is **Copy** (cheap-to-duplicate — see Chapter 4), the value type itself is Copy. That means assigning it duplicates instead of moving. For `Data` this is [planned](../../TODO.md#t-data-define); today Copy requires a `struct` (E0603).

Works today: a `struct` [deprecated] (replace with `Data.define`):

```ruby
struct Color
  r: UInt8
  g: UInt8
  b: UInt8
end

red = Color.new(255u8, 0u8, 0u8)
also_red = red               # both still valid (Color is Copy)
puts "#{red.r} #{also_red.r}"
```

### Value types get the object protocol for free

When every field supports it, a value type automatically includes the matching protocol: copying, equality, ordering, hashing and a default value. You don't need to ask for these by name. Today the implicit includes carry Rust names (`Debug`, `Clone`, `Eq`, `Hashable`, `Default`, `Ord`, `PartialOrd`), which are [deprecated]: their Ruby replacements are `inspect`, `clone`, `==`, `hash` with `eql?`, and `<=>` through `include Comparable`. A structural `==` for value types is [planned](../../TODO.md#t-d8), and a Ruby-shaped default rendering (`#<data Point x=3, y=4>`) is [planned](../../TODO.md#t-d9). See [Chapter 8](08-mixins.md) for what those mixins mean.

## Value types vs. classes — at a glance

| Feature | Class | Value type |
|---------|-------|------------|
| Allocation | Heap | Stack (by default) |
| Inheritance | Yes (single parent) | No |
| Default semantics | Move | Move (Copy when all fields are Copy) |
| Methods | Yes | Yes |
| Implicit protocol (copy, `==`, ordering, …) | No (opt in with `include`) | Yes (when fields support them) |

Rule of thumb: start with a value type. Move to a class if you need inheritance, identity, or large heap-owned state.

## Distinct wrapper types

A one-field value type makes a distinct type from an existing one. The Ruby form [planned](../../TODO.md#t-data-define):

```ruby
UserId = Data.define(value: Int)
Email = Data.define(value: String)

id = UserId.new(42)
# mixed: Int = id    # ERROR: UserId and Int are different types
```

Works today: the `newtype` keyword [deprecated] (replace with a one-field `Data.define`):

```ruby
newtype UserId(Int)
newtype Email(String)

id = UserId(42)
```

Use a wrapper to keep two `Int`s from being silently interchangeable when they mean different things (a user ID vs. a row count, for example).

## Generic classes

Type parameters in `[...]` work the same on classes as they do on methods. Works today, with `def init` [deprecated]:

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

`Box[T]` is a recipe — `Box[Int]` and `Box[String]` are concrete types made from it. Chapter 12 goes deeper.

## Adopting a mixin (preview)

To share behaviour across unrelated types, include a mixin in the class body. A **mixin** is a module of method signatures and default bodies that an including type promises to satisfy. Chapter 8 explains how to define and use them. The Ruby form declares it with `module` [planned](../../TODO.md#t-module-mixins):

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

Works today: the `mixin` keyword and `def init` [deprecated] (replace with `module` and `def initialize`):

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

## Common mistakes

**Leaving out the constructor.** An automatic constructor for a class with fields and no `def init`, taking the fields positionally or as keywords, is [planned](../../TODO.md#t-positional-new). Today the positional `.new(...)` type-checks but fails at link time (undefined `_Name_init`). Write the constructor yourself, even when it only assigns fields: `def init(@x: Int, @y: Int)`.

**Calling a `def var` method through a read-only borrow.** A writing method needs writable access to its receiver: an owned local (`a = Account.new(100)`) or a `&var` borrow, not a `&` borrow. The [deprecated] `let` binding is immutable too; replace it with plain assignment.

**Trying to use a value after a `def consume` method.** Consuming methods (often named `into_*` or `close`) take ownership. After the call, the variable is gone — that's the whole point.

**Inheritance for code reuse.** If two classes don't have a real "is-a" relationship, use a mixin instead. Inheritance is for genuine specialization.

## Giving a method a second name: `alias`

Sometimes a method deserves two names — a Ruby spelling and a longer one, or a `?`-predicate and a plain verb. Rather than writing the method twice, use `alias`. Works today, with `def init` [deprecated]:

```ruby
class Bag
  count: Int
  def init(@count: Int); end

  def size -> Int
    @count
  end

  alias length size   # `length` is now a second name for `size`
end

b = Bag.new(3)
puts b.size       # 3
puts b.length     # 3 — same method, no extra code
```

`alias new_name old_name` is a **pure synonym**: both names resolve to the *one* method body. There is no second method, no extra call frame, and no duplicated machine code — `length` simply *is* `size`.

It works at the top level, too [implemented]:

```ruby
def greet(name: String) -> String
  "hi #{name}"
end

alias hail greet   # both call the same method

puts greet("Ada")  # hi Ada
puts hail("Ada")   # hi Ada
```

A few rules:

- The form is the Ruby space form — `alias new old`, **no comma**. Ruby's `alias_method :new, :old` is [planned](../../TODO.md#t-alias-method).
- `?` and `!` names work: `alias member? include?`, `alias save! commit`.
- The target must already exist; aliasing an unknown name is an error (E1120), and a name that collides with an existing method is an error (E1122).
- **Operator** aliases (`alias << push`) are not supported yet (E1123) — define the operator method directly instead.

## Try it

Extend `Account` from earlier in this chapter:

1. Add a writing method `withdraw(amount: Int)` that subtracts from the balance.
2. Add a class method `def self.zero -> Account` that returns an account with balance 0.
3. Try calling `deposit` after `close` — read the error.

## Recap

- Classes are heap-allocated objects with inheritance. Value types (`Data.define`, `Struct.new`, planned; `struct` today) are lightweight bundles of data.
- `def initialize(@field: T)` is the constructor with automatic field assignment (planned; `def init` today).
- Inside methods, fields are `@name`.
- Method modes: reading (`def`), writing (`def var`), consuming (`def consume`), class (`def self.`).
- Single inheritance with `class Sub < Super` and `super(...)` in the constructor.
- Value types are Copy when all fields are Copy, and pick up the object protocol for free when fields support it.
- A one-field value type wraps an existing type into a distinct one.

**Next:** [Enums and Pattern Matching](07-enums-and-pattern-matching.md) — types that say "this OR that," and how to take them apart.
