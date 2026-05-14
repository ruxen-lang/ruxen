# Functions

## Defining Functions

Functions are defined with `def` and terminated with `end`:

```riven
def greet
  puts "Hello!"
end
```

The last expression is the implicit return value (like Ruby):

```riven
def double(x)
  x * 2
end
```

## Parameters and Return Types

Private functions can have fully inferred types:

```riven
def add(a, b)
  a + b
end
```

Public functions **must** have explicit type annotations (design principle P5 — Clarity At The Boundaries):

```riven
def add(a: Int, b: Int) -> Int
  a + b
end
```

## Early Return

Use `return` for early exit:

```riven
def find_positive(nums: &Array[Int]) -> Option[Int]
  for n in nums
    if n > 0
      return Some(n)
    end
  end
  nil
end
```

## Single-Expression Functions

Short functions can use brace syntax:

```riven
def double(x: Int) -> Int { x * 2 }
def is_even(n: Int) -> Bool { n % 2 == 0 }
```

## Visibility

Riven is **public by default**. Section markers (`private`, `protected`) inside a module, class, struct, or mixin body gate subsequent declarations until the next marker.

| Section marker | Scope |
|----------------|-------|
| (default) | Public — accessible from anywhere |
| `private` | Private — accessible only within the current module/type |
| `protected` | Accessible from subclasses |

```riven
module Util
  def public_api(x: Int) -> Int       # public — module default
    helper(x)
  end

  private

  def helper(x: Int) -> Int           # private — until next marker
    x * 2
  end
end
```

## Generic Functions

Use square brackets for type parameters:

```riven
def identity[T](x: T) -> T
  x
end

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

## Where Clauses

For complex generic bounds:

```riven
def merge[A, B, C](left: &A, right: &B) -> C
  where A: Iterable[Item = Int],
        B: Iterable[Item = Int],
        C: FromIterator[Int]
  # ...
end
```

## Class Methods vs Instance Methods

```riven
class User
  name: String

  def init(@name: String) end

  # Reading method — borrows the receiver immutably
  def display -> String
    "User: #{self.name}"
  end

  # Mutating method — borrows the receiver mutably
  def mut rename(name: String)
    self.name = name
  end

  # Consuming method — takes ownership of the receiver
  def consume into_name -> String
    self.name
  end

  # Class method — no receiver
  def self.anonymous -> User
    User.new("Anonymous")
  end
end
```

### Method-Mode Summary

| Declaration | Mode | Meaning |
|-------------|------|---------|
| `def method` | reading | Borrows the receiver immutably |
| `def mut method` | mutating | Borrows the receiver mutably |
| `def consume method` | consuming | Takes ownership of the receiver |
| `def self.method` | class | No receiver — module-style call |
