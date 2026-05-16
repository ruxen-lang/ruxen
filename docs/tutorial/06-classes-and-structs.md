# Classes and Structs

## Classes

Classes are heap-allocated, support inheritance, and have reference semantics for method dispatch.

```riven
class User
  name: String
  age: Int

  def init(@name: String, @age: Int)
  end

  def display -> String
    "#{self.name} (age #{self.age})"
  end
end

let user = User.new("Alice", 30)
puts user.display
```

### Field Visibility

Fields and methods are **public by default**. Use the `private` and `protected` section markers inside the class body to gate subsequent declarations until the next marker.

| Section marker | Access |
|----------------|--------|
| (default) | Public — accessible from anywhere |
| `private` | Private — only accessible within the class |
| `protected` | Accessible from the class and its subclasses |

```riven
class Account
  name: String              # public field

  private
  audit_id: Int             # private field

  protected
  internal_notes: String    # protected field
end
```

### Constructor Auto-Assign

The `@` prefix in constructor parameters automatically assigns to the field:

```riven
def init(@name: String, @age: Int)
end
# Equivalent to:
def init(name: String, age: Int)
  self.name = name
  self.age = age
end
```

### Method Self-Modes

Every method has a relationship to its receiver — *reading*, *writing*, *consuming*, or *class-method* (no receiver).

```riven
class Account
  balance: Int

  def init(@balance: Int) end

  # Reading method — borrows the receiver read-only
  def get_balance -> Int
    self.balance
  end

  # Writing method — borrows the receiver writably
  def var deposit(amount: Int)
    self.balance += amount
  end

  # Consuming method — takes ownership of the receiver
  def consume close -> Int
    puts "Account closed"
    self.balance
  end

  # Class method — no receiver
  def self.create(initial: Int) -> Account
    Account.new(initial)
  end
end
```

### Inheritance

Classes can inherit from one parent with `<`:

```riven
class Animal
  name: String
  def init(@name: String) end
  def speak -> String { "..." }
end

class Dog < Animal
  def speak -> String
    "Woof! I'm #{self.name}"
  end
end

class Cat < Animal
  def speak -> String
    "Meow! I'm #{self.name}"
  end
end
```

### Adopting a Mixin

To adopt a mixin (a contract-and-provision unit — see [Chapter 8](08-mixins.md)), use the `include` directive in the class body. The mixin's required methods become obligations; default methods are pulled in as if defined inline.

```riven
mixin Display
  def fmt(f: &var Formatter) -> Result[(), FmtError]
end

class User
  name: String
  email: String

  def init(@name: String, @email: String) end

  include Display

  def fmt(f: &var Formatter) -> Result[(), FmtError]
    f.write_str("#{self.name} <#{self.email}>")
  end
end
```

## Structs

Structs are lightweight value types. No inheritance, no heap allocation by default.

```riven
struct Point
  x: Float
  y: Float
end

let p = Point.new(3.0, 4.0)
```

### Implicit Structural Mixins

Structs automatically include `Debug`, `Clone`, `Eq`, `Hashable`, `Default`, `Ord`, and `PartialOrd` whenever every field supports the mixin — no declaration needed. `Copy` is implicit when every field is `Copy`. See §3.6 of the syntax spec and [Chapter 23](23-attributes.md) for the loud `include D1, D2` form.

```riven
struct Color
  r: UInt8
  g: UInt8
  b: UInt8
end

let red = Color.new(255, 0, 0)
let also_red = red               # copy, both valid (every field is Copy)
```

### Structs vs Classes

| Feature | Class | Struct |
|---------|-------|--------|
| Allocation | Heap | Stack (by default) |
| Inheritance | Yes (single) | No |
| Copy | No (reference semantics) | Implicit when every field is `Copy` |
| Default semantics | Move | Move (Copy when all fields are) |
| Methods | Yes | Yes |
| Mixin inclusion | Yes | Yes |

## Newtypes

Zero-cost wrapper types that create a distinct type from an existing one:

```riven
newtype UserId(Int)
newtype Email(String)

let id = UserId(42)
let email = Email(String.from("user@example.com"))

# UserId and Int are different types — can't mix them accidentally
```

## Generic Classes and Structs

```riven
class Container[T]
  items: Array[T]

  def init
    self.items = Array.new
  end

  def var add(item: T)
    self.items.push(item)
  end

  def count -> Int
    self.items.len
  end
end

var box = Container[String].new
box.add(String.from("hello"))
```
