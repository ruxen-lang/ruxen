# Classes and Structs

This chapter is about defining your own types. Ruxen offers two flavours:

- **Classes** — heap-allocated, support inheritance, behave like objects in Ruby or Java.
- **Structs** — lightweight values, no inheritance, often live on the stack.

Pick a struct when you have a small bundle of data (a `Point`, a `Color`); pick a class when you have an entity with identity, behaviour, and possibly child types.

## A first runnable example

```ruxen
class Box
  x: Int

  def init(@x: Int)
  end

  def value -> Int
    self.x
  end

  def doubled -> Int
    self.x * 2
  end
end

def main
  let b = Box.new(21)
  puts "#{b.value}"
  puts "#{b.doubled}"
end
```

```bash
ruxen compile box.rx
./box
```

Output:

```
21
42
```

You defined a class with one field, gave it a constructor, gave it two methods, built an instance with `Box.new`, and called methods on it with dot syntax.

## Anatomy of a class

```ruxen
class User
  name: String
  age: Int

  def init(@name: String, @age: Int)
  end

  def display -> String
    "#{self.name} (age #{self.age})"
  end
end
```

- **Field declarations** sit at the top: `name: String`, `age: Int`.
- **`init` is the constructor**. It's called automatically when you write `User.new(...)`.
- **`@name` in the parameter list** is shorthand for "assign this parameter to `self.name`". Without the `@`, you'd write `self.name = name` manually inside `init`.
- **`self`** inside a method is the receiver — the instance the method was called on.

### Field visibility

Fields and methods are **public by default** — anyone with a `User` can read its `name`. Use `private` and `protected` section markers inside the class body to gate what comes after:

```ruxen
class Account
  name: String              # public field

  private
  audit_id: Int             # private — only Account can see it

  protected
  internal_notes: String    # subclasses can see this too
end
```

Subsequent declarations stay in the marked section until a new marker appears.

## Method modes (the four flavours)

Every method has a relationship with its receiver — does it read, edit, consume, or skip the receiver entirely? You met these briefly in Chapter 4; here they are in a single class.

```ruxen
class Account
  balance: Int

  def init(@balance: Int)
  end

  # Reading — borrows self read-only
  def get_balance -> Int
    self.balance
  end

  # Writing — borrows self writably (allowed to change fields)
  def var deposit(amount: Int)
    self.balance = self.balance + amount
  end

  # Consuming — takes ownership; the original binding is gone after the call
  def consume close -> Int
    self.balance
  end

  # Class method — no receiver, called on the type itself
  def self.create(initial: Int) -> Account
    Account.new(initial)
  end
end

def main
  var a = Account.new(100)
  a.deposit(50)
  puts "#{a.get_balance}"     # 150

  let final_balance = a.close # a is consumed here
  puts "#{final_balance}"     # 150
  # a is no longer usable
end
```

Note the call site is identical for reading and writing methods — `a.deposit(50)` looks like `a.get_balance`. The mode is part of the method's *declaration*, not its call. (`a` does need to be `var` for a writing method to be called on it.)

## Inheritance

A class may inherit from one parent with `<`. The child gets all the parent's fields and methods, and may override any of them:

```ruxen
class Animal
  name: String

  def init(@name: String)
  end

  def speak -> String
    String.from("...")
  end
end

class Cat < Animal
  def init(name: String)
    super(name)
  end

  def speak -> String
    "Meow! I'm #{self.name}"
  end
end

def main
  let c = Cat.new(String.from("Whiskers"))
  puts c.speak
end
```

`super(...)` inside an overridden method calls the parent's version. `Cat.init` uses it to delegate name storage back to `Animal.init`.

Single inheritance only. For sharing behaviour without an inheritance relationship, use a **mixin** ([Chapter 8](08-mixins.md)).

## Structs

A **struct** is a no-frills value type — a stack-friendly bundle of fields, no inheritance, no overrides.

```ruxen
struct Point
  x: Float
  y: Float
end

def main
  let p = Point.new(3.0, 4.0)
  puts "#{p.x} #{p.y}"
end
```

Structs get a constructor named `.new` that takes the fields in declaration order — no `init` needed for the common case. You can still define methods on them with `def` just like a class.

### Structs are usually Copy

If every field of a struct is **Copy** (cheap-to-duplicate — see Chapter 4), the struct itself is Copy. That means assigning it duplicates instead of moving:

```ruxen
struct Color
  r: UInt8
  g: UInt8
  b: UInt8
end

def main
  let red = Color.new(255, 0, 0)
  let also_red = red               # both still valid (Color is Copy)
  puts "#{red.r} #{also_red.r}"
end
```

### Structs implicitly include common behaviour

When every field supports it, structs automatically gain `Debug`, `Clone`, `Eq`, `Hashable`, `Default`, `Ord`, and `PartialOrd`. You don't need to ask for these by name — they appear when the structure supports them. See [Chapter 8](08-mixins.md) for what those mean. A later chapter covers the loud `include` form that fails fast if a field doesn't fit.

## Structs vs. classes — at a glance

| Feature | Class | Struct |
|---------|-------|--------|
| Allocation | Heap | Stack (by default) |
| Inheritance | Yes (single parent) | No |
| Default semantics | Move | Move (Copy when all fields are Copy) |
| Methods | Yes | Yes |
| Implicit Debug/Clone/Eq/… | No (opt in with `include`) | Yes (when fields support them) |

Rule of thumb: start with a struct. Move to a class if you need inheritance, identity, or large heap-owned state.

## Newtypes

A **newtype** is a zero-cost wrapper that makes a distinct type from an existing one:

```ruxen
newtype UserId(Int)
newtype Email(String)

def main
  let id = UserId(42)
  # let mixed: Int = id    # ERROR: UserId and Int are different types
end
```

Use newtypes to keep two `Int`s from being silently interchangeable when they mean different things (a user ID vs. a row count, for example).

## Generic classes and structs

Type parameters in `[...]` work the same on classes and structs as they do on functions:

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

`Box[T]` is a recipe — `Box[Int]` and `Box[String]` are concrete types made from it. Chapter 12 goes deeper.

## Adopting a mixin (preview)

To share behaviour across unrelated types, include a mixin in the class body. A **mixin** is a bundle of method signatures and default bodies that an including type promises to satisfy. Chapter 8 explains how to define and use them; here's what it looks like in a class:

```ruxen
mixin Greet
  def hello -> String
end

class Dog
  name: String

  def init(@name: String)
  end

  include Greet

  def hello -> String
    "Woof! I'm #{self.name}"
  end
end

def main
  let d = Dog.new(String.from("Rex"))
  puts d.hello
end
```

## Common mistakes

**Forgetting `def init` on a class.** Classes don't get an automatic constructor — write `def init(@field1: T, @field2: T) end` or `Class.new(...)` will fail to compile.

**Calling a `def var` method on a `let`.** Writing methods need a `var` binding:

```ruxen
let c = Counter.new      # let — immutable
c.inc                    # ERROR: inc is a writing method
```

Change `let` to `var`.

**Trying to use a value after a `def consume` method.** Consuming methods (often named `into_*` or `close`) take ownership. After the call, the binding is gone — that's the whole point.

**Inheritance for code reuse.** If two classes don't have a real "is-a" relationship, use a mixin instead. Inheritance is for genuine specialization.

## Giving a method a second name: `alias`

Sometimes a method deserves two names — a Ruby spelling and a longer one, or a `?`-predicate and a plain verb. Rather than writing the method twice, use `alias`:

```rx
class Bag
  count: Int
  def init(@count: Int); end

  def size -> Int
    self.count
  end

  alias length size   # `length` is now a second name for `size`
end

let b = Bag.new(3)
puts "#{b.size}"      # 3
puts "#{b.length}"    # 3 — same method, no extra code
```

`alias new_name old_name` is a **pure synonym**: both names resolve to the *one* method body. There is no second function, no extra call frame, and no duplicated machine code — `length` simply *is* `size`.

It works at the top level for free functions, too:

```rx
def greet(name: String) -> String
  "hi #{name}"
end

alias hail greet   # both call the same function

puts greet("Ada")  # hi Ada
puts hail("Ada")   # hi Ada
```

A few rules:

- The form is the Ruby space form — `alias new old`, **no comma**.
- `?` and `!` names work: `alias member? include?`, `alias save! commit`.
- The target must already exist; aliasing an unknown name is an error (E1120), and a name that collides with an existing method is an error (E1122).
- **Operator** aliases (`alias << push`) are not supported yet (E1123) — define the operator method directly instead.

## Try it

Extend `Account` from earlier in this chapter:

1. Add a writing method `withdraw(amount: Int)` that subtracts from the balance.
2. Add a class method `def self.zero -> Account` that returns an account with balance 0.
3. Try calling `deposit` on a `let` binding — read the error.

## Recap

- Classes are heap-allocated objects with inheritance. Structs are lightweight value types.
- `def init(@field: T)` is a constructor with automatic field assignment.
- Method modes: reading (`def`), writing (`def var`), consuming (`def consume`), class (`def self.`).
- Single inheritance with `class Sub < Super` and `super(...)`.
- Structs are Copy when all fields are Copy, and pick up `Debug`/`Clone`/etc. for free when fields support them.
- Newtypes wrap an existing type into a distinct one with no runtime cost.

**Next:** [Enums and Pattern Matching](07-enums-and-pattern-matching.md) — types that say "this OR that," and how to take them apart.
