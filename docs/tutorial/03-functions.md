# Methods

A **method** is a named piece of code you can call. Methods let you give a name to "this is the operation we do here," reuse it, and test it in isolation. As in Ruby, a `def` at the top level of a file defines a method you can call from anywhere in the program, and the program itself is the file's top-level statements, run from top to bottom.

## A first runnable example

Save this as `doubler.rx` [implemented]:

```ruby
def double(x: Int) -> Int
  x * 2
end

puts double(5)
puts double(21)
```

```bash
ruxen compile doubler.rx
./doubler
```

Output:

```text
10
42
```

You defined a method `double`, called it twice, and printed the results.

## The shape of a method

The general shape, with placeholder names:

```text
def name(param1: Type1, param2: Type2) -> ReturnType
  # ... body ...
end
```

- `def` introduces a method.
- Each parameter has a name and a type after a colon. The annotations are the Ruxen addition to Ruby.
- `-> ReturnType` says what comes back.
- The body is everything up to `end`.
- The **last expression in the body is the return value** — no `return` keyword needed for the normal case (familiar to Ruby readers).

[implemented]:

```ruby
def add(a: Int, b: Int) -> Int
  a + b
end
```

A method that returns nothing uses `nil` as its return type — and can omit the arrow altogether [implemented]:

```ruby
def greet
  puts "Hello!"
end

# same thing, spelled out:
def greet_again -> nil
  puts "Hello!"
end

greet()
greet_again()
```

Call a method that takes no arguments with empty parentheses, `greet()`,
for now: a bare zero-argument call used as a statement or a value is
[planned](../../TODO.md#t-f45).

## Inferred return types

Every parameter needs a type. The return type can be left out when the body
makes it obvious; Ruxen infers it from the last expression [implemented]:

```ruby
def add(a: Int, b: Int)
  a + b
end

puts add(1, 2)
```

When a method is part of your **public surface** — anything callers outside the file rely on — write the return type explicitly. This is a friendliness rule: callers should be able to learn how to use your method by reading its signature, not its body.

## Early return

The last expression returns automatically, but sometimes you want to bail out early. Use `return`, often with a statement modifier [implemented]:

```ruby
def first_positive(a: Int, b: Int) -> Int
  return a if a > 0
  return b if b > 0
  0
end

puts first_positive(-1, 3)    # 3
```

Statement modifiers (`return a if a > 0`) work inside a method body; at the
top level of a file they are [planned](../../TODO.md#t-toplevel-stmts).

## Single-expression form

Short methods can use Ruby's endless form [implemented]:

```ruby
def double(x: Int) -> Int = x * 2
def even?(n: Int) -> Bool = n % 2 == 0

puts double(4)
puts even?(4)
```

Same meaning, just compact. A method name may end in `?` (a predicate) or
`!` (a "dangerous" variant), as in Ruby. The older brace body,
`def double(x: Int) -> Int { x * 2 }`, is [deprecated]: replace it with the
endless form.

## Recursion

Methods can call themselves [implemented]:

```ruby
def factorial(n: Int) -> Int
  if n <= 1
    1
  else
    n * factorial(n - 1)
  end
end

puts factorial(5)     # 120
```

## Visibility

Ruxen is **public by default** — anything you define is callable from outside its class unless you say otherwise. Use a `private` section marker inside a class body to mark everything that comes after it (until the next marker) as private. The marker is accepted [implemented]; the access rule is not enforced yet (see below). The constructor here is the [deprecated] `def init`, see below:

```ruby
class Calc
  factor: Int

  def init(@factor: Int)
  end

  def public_api(x: Int) -> Int      # public — the default
    self.helper(x)
  end

  private

  def helper(x: Int) -> Int          # private — for Calc's own use
    x * @factor
  end
end

c = Calc.new(2)
puts c.public_api(21)    # 42
```

Two things in this example will change. The constructor is still spelled
`def init`, [deprecated] until `def initialize` lands
[planned](../../TODO.md#t-initialize). And `public_api` calls `helper`
through `self.helper(x)`: calling another method of the same class with a
bare `helper(x)` is [planned](../../TODO.md#t-implicit-self).

`private :a, :b` marks named methods private, and a `protected` marker is also accepted. What works today is only the marking: neither marker is enforced, so `c.helper(3)` from outside `Calc` still compiles and runs, a [known issue](../../TODO.md#t-visibility-enforce). Treat the markers as documentation for now. Ruby's rule for `protected` (callable with an explicit receiver of the same class family, from inside it) is [planned](../../TODO.md#t-protected). See [Chapter 6](06-classes-and-structs.md).

## Generic methods

Sometimes a method does the same thing regardless of the type of its argument — an identity method returns whatever you give it. **Generics** let you express that without copy-pasting the method for every type. Type parameters go in square brackets [implemented]:

```ruby
def identity[T](x: T) -> T
  x
end

n = identity(42)         # T = Int
s = identity("hello")    # T = String
puts "#{n} #{s}"
```

Chapter 12 covers generics in depth — including how to require that `T` supports a particular operation.

## Class methods vs. instance methods (preview)

Once you start writing classes (Chapter 6), you'll see four flavours of method. Here they are at a glance, in the Ruby form; the constructor `initialize` is [planned](../../TODO.md#t-initialize):

```ruby
class User
  name: String

  def initialize(@name: String)
  end

  # Reading — borrows the receiver, doesn't change it
  def display -> String
    "User: #{@name}"
  end

  # Writing — borrows the receiver writably (allowed to change fields)
  def var rename(name: String)
    @name = name
  end

  # Consuming — takes ownership of the receiver; the receiver can't be used after
  def consume into_name -> String
    @name.clone
  end

  # Class method — no receiver, called on the type itself
  def self.anonymous -> User
    User.new("Anonymous")
  end
end
```

Works today, with the constructor spelled `def init` [deprecated] (replace with `def initialize`):

```ruby
class User
  name: String

  def init(@name: String)
  end

  def display -> String
    "User: #{@name}"
  end

  def var rename(name: String)
    @name = name
  end

  def consume into_name -> String
    @name.clone
  end

  def self.anonymous -> User
    User.new("Anonymous")
  end
end

u = User.anonymous
u.rename("Ada")
puts u.display       # User: Ada
puts u.into_name     # Ada
```

| Form | Mode | Meaning |
|------|------|---------|
| `def method` | reading | Reads `self`, doesn't change it |
| `def var method` | writing | Allowed to change `self`'s fields |
| `def consume method` | consuming | Takes `self` by value — caller loses access |
| `def self.method` | class | Called on the type, not on an instance |

Don't worry about the distinction yet — it'll click in Chapter 4 (borrowing) and Chapter 6 (classes).

## Common mistakes

**Leaving out a parameter type.** Rejected today: the parser expects `name: Type` [implemented]:

```ruby
def add(a, b)        # ERROR: expected Colon, found Comma
  a + b
end
```

Fix: `def add(a: Int, b: Int)`.

**Putting a statement after the last expression.** Rejected today [implemented]:

```ruby
def double(x: Int) -> Int
  x * 2
  puts "computed"     # ERROR: type mismatch: expected `Int`, found `()`
end

puts double(2)
```

Fix: swap the order, or store the value first [implemented]:

```ruby
def double(x: Int) -> Int
  result = x * 2
  puts "computed"
  result
end

puts double(2)
```

**Confusing `def var foo` with a mutable parameter.** `def var` controls how the *receiver* (`self`) is borrowed, not how parameters are borrowed. To let a method change an argument, take a writable borrow.

Works today, with the [deprecated] `push_str` (replace with `<<` once it appends):

```ruby
def exclaim(s: &var String)
  s.push_str("!")
end

text = "hi"
exclaim(&var text)
puts text     # hi!
```

The writable borrow `&var` is [implemented]. `push_str` is the Rust-named
method; the Ruby name `<<` does not append to a `String` yet, a
[known issue](../../TODO.md#t-string-shovel).

## Try it

Write a method `triple(x: Int) -> Int` and call it at the top level. Then write a helper `square(x: Int)` (leave out the return type — let inference do the work) and a `sum_of_squares(a: Int, b: Int) -> Int` that uses it. Compile and run.

## Recap

- `def name(...) -> Type ... end` defines a method. The last expression is the return value.
- Every parameter has a type; the return type can be inferred, but public methods should spell it out.
- `def f(x: Int) = expr` is the one-line form.
- `return` is for early exit only.
- `[T]` introduces a type parameter — preview of generics in [Chapter 12](12-generics.md).
- Method receivers come in four flavours: reading, writing, consuming, class — full details in [Chapter 6](06-classes-and-structs.md).

**Next:** [Ownership and Borrowing](04-ownership-and-borrowing.md) — the rule that makes Ruxen safe without a garbage collector.
