# Mixins

A **mixin** is Riven's contract-and-provision unit. The same construct does two jobs:

- **Contract** — it declares required methods that an including type must provide.
- **Provision** — it can supply default method bodies that the including type gets for free.

Mixins are how shared behaviour is expressed in Riven.

## Defining a Mixin

```riven
mixin Renderable
  def to_display -> String
end
```

That mixin declares one required method (a signature with no body). Including types must provide `to_display`. (`Renderable` here is a tutorial example; the real builtin `Display` uses a different method shape — see [Chapter 17](17-string-formatting-and-interpolation.md).)

### Default Methods

A mixin can supply default implementations:

```riven
mixin Greetable
  def name -> String                  # required
  def greet -> String                 # default
    "Hello, #{self.name}!"
  end
end
```

Any class that includes `Greetable` must supply `name` but gets `greet` for free.

### Associated Types

A mixin can declare types that the including type binds:

```riven
mixin Iterator
  type Item
  def var next -> Option[Self.Item]
end
```

### Mixin Inheritance

A mixin can extend another mixin's contract:

```riven
mixin Serializable: Renderable
  def serialize -> String
  def self.deserialize(data: &str) -> Result[Self, Error]
end
```

A class that includes `Serializable` must satisfy both `Serializable` *and* `Renderable`.

## Including a Mixin

Adopt a mixin with the `include` directive in the type body. Required methods become obligations the compiler checks. Default methods are pulled into the class as if defined inline; a class-defined method with the same name overrides the mixin's default.

```riven
class User
  name: String
  email: String

  def init(@name: String, @email: String) end

  include Renderable

  def to_display -> String
    "#{self.name} <#{self.email}>"
  end
end
```

Multiple `include` directives stack in source order. The class's own definition always wins over any included mixin's default. If two included mixins each supply a *default* body for the same method and the class itself has no override, the compiler rejects with `E-MIX-AMBIGUOUS-DEFAULT` — the class must define its own implementation to disambiguate.

### Overriding a Default

```riven
class FormalUser
  name: String

  def init(@name: String) end

  include Greetable

  def greet -> String                 # overrides the mixin's default
    "Good day, #{self.name}."
  end
end
```

## Using Mixins as Bounds

### Generic Functions with Bounds

Constrain a type parameter with `:`. Multiple bounds use `+`:

```riven
def largest[T: Ord](list: &Array[T]) -> &T
  # ...
end

def process[T: Renderable + Serializable](item: &T)
  # ...
end
```

### Existential Receivers: `some Mixin` and `any Mixin`

A function parameter or return type may name a mixin in one of two ways.

**`some Mixin`** — the compiler picks one concrete conforming type per call site. The function body is monomorphized; the receiver type is fixed for any given call but invisible to callers. Zero runtime cost; methods may inline.

```riven
def print_it(item: &some Renderable)
  puts item.to_display
end

print_it(&User.new("Alice", "a@example.com"))   # specialized for User
print_it(&Robot.new(42))                         # specialized for Robot
```

**`any Mixin`** — the value carries a vtable at runtime; one function body handles every conforming type. This is what enables heterogeneous collections.

```riven
def shout_all(crowd: &Array[Box[any Greetable]])
  for member in crowd
    puts member.greet.upcase
  end
end
```

Coercions `&T -> &some Mixin` and `&T -> &any Mixin` are implicit at assignment and call boundaries when `T` includes the mixin. `Box[T] -> Box[any Mixin]` is the unsized owning coercion.

A mixin is **object-safe** (usable through `any`) when every method satisfies:

- No `Self`-by-value in argument or return position
- No per-method generic parameters
- No class-method (`def self.foo`) entries
- No consuming receiver

Structural satisfaction is accepted for `some Mixin` only. `any Mixin` requires an explicit `include Mixin` directive in the implementing class.

## Built-in Mixins

| Mixin        | Purpose                                                      |
|--------------|--------------------------------------------------------------|
| `Display`    | Convert to display string (via `fmt(f: &var Formatter)`)     |
| `Debug`      | Debug representation                                         |
| `Ord`        | Total ordering (`<`, `>`, `<=`, `>=`, `cmp`)                  |
| `PartialOrd` | Partial ordering                                             |
| `Eq`         | Equality                                                     |
| `PartialEq`  | Partial equality                                             |
| `Hashable`   | Hash computation (for `Map` keys)                            |
| `Iterator`   | Can yield successive items via `def var next`                |
| `Copy`       | Assignment duplicates the value                              |
| `Clone`      | Explicit `.clone` deep copy                                  |
| `Default`    | Type has a `.default` no-arg constructor                     |
| `Drop`       | Custom destructor logic                                      |
| `Error`      | Error type with `.message`                                   |

Note: there is no separate `Iterable` mixin — a type that includes `Iterator` *is* iterable by virtue of providing `def var next`.

## Conditional Methods

Use an `extension` block to add methods only when a type parameter satisfies a bound:

```riven
extension Container[T] where T: Renderable
  def print_all
    for item in self.items
      puts item.to_display
    end
  end
end
```

Without a `where` clause, an `extension` block simply adds methods to the named type unconditionally — useful when extending a foreign type without re-opening its original definition.

## Implicit Structural Mixins

A handful of standard mixins — `Debug`, `Clone`, `Eq`, `PartialEq`, `Hashable`, `Default`, `Ord`, `PartialOrd`, and (on structs) `Copy` — are **implicitly included** when every field supports the mixin. No declaration is required:

```riven
struct Point
  x: Float
  y: Float
end

# Point implicitly includes Debug, Clone, Eq, Hashable, Default, Ord, PartialOrd, Copy.
```

The loud form `include Debug, Clone, Eq, Hashable` is also accepted when you want a use-site error to fire early if the structural precondition breaks.

`Send` and `Sync` are **auto-mixins** — the compiler infers them per the field rule and you never write `include Send` / `include Sync`. Opt out with `exclude Send` / `exclude Sync`; opt in for an inference-incompatible structure with `unsafe include Send` / `unsafe include Sync`.

See [Chapter 23](23-attributes.md) for the full structural-mixin table and the diagnostics that fire when an implicit-include's preconditions aren't met.
