# Functions

A **function** is a named piece of code you can call. Functions let you give a name to "this is the operation we do here," reuse it, and test it in isolation. Every Ruxen program starts at a function called `main`.

## A first runnable example

```ruxen
def double(x: Int) -> Int
  x * 2
end

def main
  puts "#{double(5)}"
  puts "#{double(21)}"
end
```

```bash
ruxen compile doubler.rx
./doubler
```

Output:

```
10
42
```

You defined a function `double`, called it twice, and printed the results.

## The shape of a function

```ruxen
def name(param1: Type1, param2: Type2) -> ReturnType
  # ... body ...
end
```

- `def` introduces a function.
- The parameters list types after a colon.
- `-> ReturnType` says what comes back.
- The body is everything up to `end`.
- The **last expression in the body is the return value** — no `return` keyword needed for the normal case (familiar to Ruby readers).

```ruxen
def add(a: Int, b: Int) -> Int
  a + b
end
```

A function that returns nothing uses `nil` as its return type — and can omit the arrow altogether:

```ruxen
def greet
  puts "Hello!"
end

# same thing, spelled out:
def greet -> nil
  puts "Hello!"
end
```

## Inference for private helpers

A function used only inside the current file (a private helper) can leave its types blank — Ruxen will infer them:

```ruxen
def add(a, b)
  a + b
end
```

When a function is part of your **public surface** — anything callers outside the file rely on — write the types explicitly. This is a friendliness rule: callers should be able to learn how to use your function by reading its signature, not its body.

## Early return

The last expression returns automatically, but sometimes you want to bail out early. Use `return`:

```ruxen
def first_positive(a: Int, b: Int) -> Int
  if a > 0
    return a
  end
  if b > 0
    return b
  end
  0
end
```

## Single-expression form

Short functions can use braces instead of `... end`:

```ruxen
def double(x: Int) -> Int { x * 2 }
def is_even(n: Int) -> Bool { n % 2 == 0 }
```

Same meaning, just compact.

## Recursion

Functions can call themselves:

```ruxen
def factorial(n: Int) -> Int
  if n <= 1
    1
  else
    n * factorial(n - 1)
  end
end

def main
  puts "#{factorial(5)}"     # 120
end
```

## Visibility

Ruxen is **public by default** — anything you define is callable from outside its module unless you say otherwise. Use a `private` section marker inside a module body to gate everything that comes after it (until the next marker):

```ruxen
module Util
  def public_api(x: Int) -> Int      # public — module default
    helper(x)
  end

  private

  def helper(x: Int) -> Int          # private — only Util can call this
    x * 2
  end
end
```

A `protected` marker is also available; it scopes declarations to subclasses (relevant inside class bodies — see [Chapter 6](06-classes-and-structs.md)).

## Generic functions

Sometimes a function does the same thing regardless of the type of its argument — an identity function returns whatever you give it. **Generics** let you express that without copy-pasting the function for every type. Type parameters go in square brackets:

```ruxen
def identity[T](x: T) -> T
  x
end

def main
  let n = identity(42)         # T = Int
  let s = identity("hello")    # T = &str
  puts "#{n} #{s}"
end
```

Chapter 12 covers generics in depth — including how to require that `T` supports a particular operation.

## Class methods vs. instance methods (preview)

Once you start writing classes (Chapter 6), you'll see four flavours of method. Here they are at a glance:

```ruxen
class User
  name: String

  def init(@name: String) end

  # Reading — borrows the receiver, doesn't change it
  def display -> String
    "User: #{self.name}"
  end

  # Writing — borrows the receiver writably (allowed to change fields)
  def var rename(name: String)
    self.name = name
  end

  # Consuming — takes ownership of the receiver; the receiver can't be used after
  def consume into_name -> String
    self.name
  end

  # Class method — no receiver, called on the type itself
  def self.anonymous -> User
    User.new(String.from("Anonymous"))
  end
end
```

| Form | Mode | Meaning |
|------|------|---------|
| `def method` | reading | Reads `self`, doesn't change it |
| `def var method` | writing | Allowed to change `self`'s fields |
| `def consume method` | consuming | Takes `self` by value — caller loses access |
| `def self.method` | class | Called on the type, not on an instance |

Don't worry about the distinction yet — it'll click in Chapter 4 (borrowing) and Chapter 6 (classes).

## Common mistakes

**Forgetting the return type on a public function.**

```ruxen
def add(a: Int, b: Int)        # ERROR: public function needs -> ReturnType
  a + b
end
```

Fix: add `-> Int`.

**Putting a statement after the last expression.**

```ruxen
def double(x: Int) -> Int
  x * 2
  puts "computed"     # ERROR: last expression must be Int, not nil
end
```

Fix: swap the order, or store the value first: `let result = x * 2; puts "computed"; result` — though splitting into two lines is more idiomatic.

**Confusing `def var foo` with a mutable parameter.** `def var` controls how the *receiver* (`self`) is borrowed, not how parameters are borrowed. Parameters are immutable bindings inside the function — to get a writable parameter, use a writable borrow: `def append(s: &var String)`.

## Try it

Write a function `triple(x: Int) -> Int` and call it from `main`. Then write a private helper `square(x)` (no types — let inference do the work) and a public `sum_of_squares(a: Int, b: Int) -> Int` that uses it. Compile and run.

## Recap

- `def name(...) -> Type ... end` defines a function. The last expression is the return value.
- Private helpers can skip type annotations; public functions must spell them out.
- `return` is for early exit only.
- `[T]` introduces a type parameter — preview of generics in [Chapter 12](12-generics.md).
- Method receivers come in four flavours: reading, writing, consuming, class — full details in [Chapter 6](06-classes-and-structs.md).

**Next:** [Ownership and Borrowing](04-ownership-and-borrowing.md) — the rule that makes Ruxen safe without a garbage collector.
