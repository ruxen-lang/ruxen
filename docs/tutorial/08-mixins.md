# Mixins

Some behaviour cuts across types that have nothing else in common. A `User`, a `Robot`, and an `Animal` might all need a "say hello" method, but they aren't really the same kind of thing — making them all inherit from a shared parent would be a stretch. **Mixins** solve exactly this: a way to share method signatures and default bodies across unrelated types.

If you've used Ruby's modules or Rust's traits, this will feel familiar. The vocabulary is "mixin," and it does two jobs at once:

- **Contract** — declare methods an including type must provide.
- **Provision** — supply default method bodies that the including type gets for free.

## A first runnable example

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

```bash
ruxen compile mixin_demo.rx
./mixin_demo
```

Output:

```
Woof! I'm Rex
```

The mixin `Greet` declared one required method (`hello`); the class `Dog` adopted it with `include Greet` and supplied a body. From this point on, anyone holding a `Dog` can call `.hello`.

## Defining a mixin

```ruxen
mixin Renderable
  def to_display -> String
end
```

A bare signature (no body) is a **required method** — any type that includes `Renderable` must define `to_display`.

### Default methods

A mixin can also supply default bodies. Including types get the defaults for free and may override them:

```ruxen
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
    self.nm.clone
  end
end

def main
  let b = Bot.new(String.from("Riv"))
  puts "#{b.greet}"        # Hello, Riv!
end
```

`Bot` supplied `name`; `greet` came along for the ride from the default in `Greeter`.

### Mixin inheritance

One mixin can require another. A type including the child must satisfy both contracts:

```ruxen
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
    "A: #{self.name}"
  end

  include B

  def b_msg -> String
    "B: #{self.name}"
  end
end

def main
  let t = Thing.new(String.from("x"))
  puts "#{t.a_msg}"
  puts "#{t.b_msg}"
end
```

## Using mixins as bounds on generics

Mixins double as **bounds** — a way to constrain a generic type parameter to "any type that includes this mixin." Constrain with `:`:

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
  let m = max(7, 3)
  puts "#{m}"        # 7
end
```

Two things happening:

- `extension Int ... include Greater ... end` adds the `Greater` mixin to a type we don't own. This is the everyday way to teach an existing type a new mixin.
- `def max[T: Greater]` says "`T` can be any type, as long as it includes `Greater`."

Use `+` to combine bounds: `def log_and_save[T: Display + Serializable](item: &T)`.

## `where` clauses for complex bounds

When the constraints get long, `where` lifts them out of the parameter list:

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
  puts merge(1, 2)         # 1+2
end
```

## `some Mixin` and `any Mixin`

When a function takes "something that includes a mixin" as a parameter, there are two flavours:

- **`some Mixin`** — at each call site, the compiler picks one concrete conforming type. Different calls can use different types, but inside one call the type is fixed. Zero runtime cost; the compiler specializes the function per type used.
- **`any Mixin`** — the value carries a small dispatch table at runtime, and one function body handles every conforming type. This is what lets you put different types in the same collection.

```ruxen
def print_it(item: &some Renderable)
  puts item.to_display
end

# Same function, called with two different types — compiler specializes each call:
print_it(&User.new("Alice", "a@example.com"))
print_it(&Robot.new(42))
```

```ruxen
# Heterogeneous collection — needs `any` for runtime dispatch:
def shout_all(crowd: &Array[Box[any Greeter]])
  for member in crowd
    puts member.greet
  end
end
```

Use `some` when you have one concrete type per call (the common case). Reach for `any` only when you need to mix types in the same container.

## Conditional methods with `extension`

You can add methods to a type only when its type parameter satisfies some bound:

```ruxen
extension Container[T] where T: Showable
  def print_all
    for item in self.items
      puts item.to_display
    end
  end
end
```

A `Container[Int]` (since `Int: Showable`) picks up `print_all`. A `Container[SomeOtherType]` that doesn't include `Showable` doesn't have it.

Without `where`, an `extension` block adds methods unconditionally — useful for teaching an existing type a new trick.

## Implicit mixins on structs

Some everyday mixins — `Debug`, `Clone`, `Eq`, `Hashable`, `Default`, `Ord`, `PartialOrd`, and (on structs) `Copy` — are **implicitly included** when every field supports them. No `include` line needed:

```ruxen
struct Point
  x: Float
  y: Float
end

# Point automatically supports Debug, Clone, Eq, Hashable, Default,
# Ord, PartialOrd, and Copy — because every field does.
```

Two related "auto-mixins," `Send` and `Sync`, control whether a value is safe to send between threads or share between them. The compiler infers them from a type's fields — you never write `include Send`. We come back to this in the concurrency chapter (Chapter 21).

## Built-in mixins worth knowing

| Mixin        | Purpose                                                |
|--------------|--------------------------------------------------------|
| `Display`    | Friendly string for users (used by `puts`, `#{...}`)   |
| `Debug`      | Developer-facing string for debugging                  |
| `Ord`        | Total ordering (`<`, `>`, `<=`, `>=`, `cmp`)           |
| `PartialOrd` | Partial ordering (for types like `Float` with NaN)     |
| `Eq`         | Equality                                               |
| `Hashable`   | Can be a `Map` or `Set` key                            |
| `Iterator`   | Can yield successive items via `def var next`          |
| `Copy`       | Assignment duplicates instead of moving                |
| `Clone`      | Explicit `.clone` deep copy                            |
| `Default`    | Has a no-argument `.default` constructor               |
| `Drop`       | Custom cleanup logic when a value goes out of scope    |
| `Error`      | Standard error type with `.message`                    |

## Common mistakes

**Forgetting to implement a required method.** If `include Mixin` declares `def foo` but the class doesn't supply one, the compiler refuses to build. Read the error — it tells you which method is missing.

**Two mixins, one default — no override.** If two included mixins each provide a default for the same method, the compiler asks you to disambiguate by defining the method explicitly in the class. (The error code is `E-MIX-AMBIGUOUS-DEFAULT`.)

**Trying to use `any Mixin` for a method that takes `self` by value.** A mixin is usable through `any` only if its methods can be dispatched through a runtime table — that rules out methods that return `Self` by value or consume `self`. Use `some Mixin` (compile-time) when the methods don't fit `any`.

**Re-opening a class to add methods.** Don't. Use an `extension Type ... end` block instead. It keeps additions discoverable and never conflicts with the original definition.

## Try it

Define a mixin `Animal` with one required method `species -> &str` and one default `describe -> String { "a #{self.species}" }`. Then make `class Dog` and `class Spider` both include `Animal`. Print `dog.describe` and `spider.describe` and observe the default doing its job.

Then add a function `def announce[T: Animal](a: &T)` that prints `a.describe`. Call it with both your `Dog` and `Spider` values.

## Recap

- A mixin is both a contract (required methods) and a provision (default methods).
- `include MixinName` inside a type body adopts the contract.
- Mixins double as bounds: `[T: Mixin]` says `T` must include the mixin.
- `some Mixin` is compile-time dispatch (one concrete type per call site); `any Mixin` is runtime dispatch (one function body handles many types).
- Structs implicitly pick up common mixins when every field qualifies.

**Next:** [Closures and Blocks](09-closures-and-blocks.md) — anonymous functions you pass around as values.
