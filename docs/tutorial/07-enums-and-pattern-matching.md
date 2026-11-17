# Enums and Pattern Matching

Many values are "one of a fixed set of cases": a `TrafficLight` is red, yellow, or green; a shape is a circle or a rectangle. Ruby has no `enum` keyword, and neither will Ruxen. The Ruby way is:

- **Symbols** for plain tags: `:red`, `:yellow`, `:green` [planned](../../TODO.md#t-symbols).
- **One `Data` type per case** when a case carries values, grouped in a module [planned](../../TODO.md#t-data-define).
- **`T?` and `nil`** for "a value or nothing" [implemented].
- **Exceptions** for "a value or an error" [planned](../../TODO.md#t-exceptions), covered in [Chapter 11](11-error-handling.md).

You match all of them with `case/in`. Until symbols and `Data` land, the working form is the `enum` keyword, which is [deprecated] (replace with symbols or `Data` variants). This chapter shows each Ruby form and the form that works today.

## A first runnable example

The Ruby form: symbols matched with `case/in` [planned](../../TODO.md#t-symbols):

```ruby
def describe(c: Symbol) -> String
  case c
  in :red then "red"
  in :green then "green"
  in :blue then "blue"
  end
end

puts describe(:red)
puts describe(:green)
puts describe(:blue)
```

Works today: an `enum` [deprecated] (replace with symbols):

```ruby
enum Color
  Red
  Green
  Blue
end

def describe(c: Color) -> String
  case c
  in Color.Red then "red"
  in Color.Green then "green"
  in Color.Blue then "blue"
  end
end

puts describe(Color.Red)
puts describe(Color.Green)
puts describe(Color.Blue)
```

```bash
ruxen compile colors.rx
./colors
```

Output:

```text
red
green
blue
```

The compiler does not check today that the arms cover every case, so add an `in _` arm when you leave cases out.

## Cases that carry data

A case can carry values — different cases can carry different things. The Ruby form is one `Data` type per case, grouped in a module and used as one type (`s: Shape`) [planned](../../TODO.md#t-data-define). How the module declares its closed set of cases is still an open question [planned](../../TODO.md#t-oq):

```ruby
module Shape
  Circle = Data.define(radius: Int)
  Rectangle = Data.define(width: Int, height: Int)
end

def area(s: Shape) -> Int
  case s
  in Shape::Circle(radius:) then radius * radius * 3
  in Shape::Rectangle(width:, height:) then width * height
  end
end

puts area(Shape::Circle.new(radius: 5))
puts area(Shape::Rectangle.new(width: 4, height: 6))
```

Works today: an `enum` with payload variants [deprecated] (replace with `Data` variants):

```ruby
enum Shape
  Circle(radius: Int)
  Rectangle(width: Int, height: Int)
end

def area(s: Shape) -> Int
  case s
  in Shape.Circle(r) then r * r * 3
  in Shape.Rectangle(w, h) then w * h
  end
end

c = Shape.Circle(radius: 5)
r = Shape.Rectangle(width: 4, height: 6)
puts area(c)
puts area(r)
```

You can construct a variant with positional arguments (`Shape.Circle(5)`) or named arguments (`Shape.Circle(radius: 5)`). The pattern in `case/in` binds the payload to a name you choose.

## Methods on tagged types

A `Data` type takes methods in a `do ... end` block, like any Ruby class body [planned](../../TODO.md#t-data-define). Works today: methods inside an `enum` [deprecated] (replace with `Data` variants):

```ruby
enum Priority
  Low
  Medium
  High

  def label -> String
    case self
    in Priority.Low then "low"
    in Priority.Medium then "medium"
    in Priority.High then "high"
    end
  end
end

puts Priority.Low.label
puts Priority.Medium.label
puts Priority.High.label
```

## `T?` — a value or nothing

Many languages have `null` to say "no value here." In Ruxen there is one way to say "maybe missing": the type `T?` and the value `nil`. A method that might not have a result returns `T?` [implemented]:

```ruby
def find(n: Int) -> Int?
  if n > 0
    n
  else
    nil
  end
end

def label(o: Int?) -> String
  case o
  in nil then "nothing"
  in _ then "something"
  end
end

puts label(find(5))     # something
puts label(find(0))     # nothing
puts find(0).nil?       # true
```

The bare value (`n` on its own) is the present case: Ruxen wraps it for you in a `-> Int?` method, for a typed variable and for `return`. Safe navigation `user&.name` calls a method only when the value is present [implemented].

A binding arm after `in nil` (`in v then ...`) binds the whole `T?` today, not the value inside, a [known issue](../../TODO.md#t-case-in-nil). To get at the value, the Ruby forms are narrowing in a condition, `if (v = find(5))` [planned](../../TODO.md#t-narrowing), and a default, `find(5) || 0` [planned](../../TODO.md#t-truthiness):

```ruby
if (v = find(5))
  puts "got #{v}"
end

a = find(5) || 0
```

Works today, all [deprecated]: `if let Some(v)`, `unwrap_or`, and the typed `let` locals inside a `def` (replace them with `if (v = ...)`, `||`, and a typed local `some_v: Int? = 5` [planned](../../TODO.md#t-typed-local)):

```ruby
def find(n: Int) -> Int?
  n > 0 ? n : nil
end

def show
  if let Some(v) = find(5)
    puts "got #{v}"
  end
  let some_v: Int? = 5
  let none_v: Int? = nil
  a = some_v.map { |n| n * 10 }.unwrap_or(0)
  b = none_v.map { |n| n * 10 }.unwrap_or(0)
  puts a     # 50
  puts b     # 0
end

show()
```

`Option[T]`, `Some(x)`, `unwrap!`, `expect!` and the `??` operator are the Rust spellings of the same idea, all [deprecated]: write `T?`, the bare value, `x || default`, `x or raise ...` and `case/in` instead. `Option` stays only as the compiler's internal representation of `T?`.

## Success or error

The Ruby way to report failure is to raise an exception and rescue it where you can handle it [planned](../../TODO.md#t-exceptions):

```ruby
def divide(a: Int, b: Int) -> Int
  raise ZeroDivisionError, "divide by zero" if b == 0
  a / b
end

begin
  puts "ok #{divide(10, 2)}"
  puts "ok #{divide(10, 0)}"
rescue ZeroDivisionError => e
  puts "err #{e.message}"
end
```

Works today: return a `Result` and match `Ok` and `Err` [deprecated] (replace with `raise` and `rescue`):

```ruby
def divide(a: Int, b: Int) -> Result[Int, String]
  if b == 0
    Err("divide by zero")
  else
    Ok(a / b)
  end
end

case divide(10, 2)
in Ok(v) then puts "ok #{v}"
in Err(e) then puts "err #{e}"
end
case divide(10, 0)
in Ok(v) then puts "ok #{v}"
in Err(e) then puts "err #{e}"
end
```

Output:

```text
ok 5
err divide by zero
```

There's a dedicated chapter on error handling ([Chapter 11](11-error-handling.md)); this much is enough to start.

## Generic tagged types

The `[T]` syntax makes an `enum` generic, just like methods and classes. Works today [deprecated] (replace with `T?`, or generic `Data` variants):

```ruby
enum MyOpt[T]
  Has(T)
  Empty
end

def describe(o: MyOpt[Int]) -> String
  case o
  in MyOpt.Has(n) then "some #{n}"
  in MyOpt.Empty then "none"
  end
end

a = MyOpt.Has(42)
puts describe(a)
puts describe(MyOpt.Empty)
```

`T?` is exactly this pattern, built into the language.

## Pattern matching extras

### Guards

A guard is an extra `if` filter on an arm [implemented]:

```ruby
def grade(score: Int) -> String
  case score
  in n if n >= 90 then "A"
  in n if n >= 80 then "B"
  in _ then "F"
  end
end

puts grade(92)
```

### Alternatives

Combine alternatives with `|` [implemented]:

```ruby
def kind(day: &String) -> String
  case day
  in "Saturday" | "Sunday" then "weekend"
  in _ then "weekday"
  end
end

puts kind("Monday")
```

### Wildcard

`_` matches anything and binds nothing [implemented]:

```ruby
def describe(value: Int) -> String
  case value
  in 0 then "zero"
  in _ then "something"
  end
end

puts describe(3)
```

Do not pass a bare value straight to a `T?` parameter today: `describe(3)` for a `value: Int?` parameter compiles but crashes at run time (the value is not wrapped at the call), a [known issue](../../TODO.md#t-optional-arg-wrap). Pass a value that is already optional, such as the result of a `-> Int?` method like `find` above.

More Ruby patterns are [planned](../../TODO.md#t-patterns): ranges (`in 3..5`), arrays (`in [first, *rest]`), hashes (`in {name: String => name}`), class checks (`in Integer => n`) and pins (`in ^expected`). The `Some(x)`, `Ok(v)` and `Err(e)` patterns are [deprecated]: use `in nil` / `in x` for absence and `rescue` for errors.

### Borrowing matches

When you match on a borrow (`&T`), the bindings inside patterns are themselves borrows — no need to write `ref` explicitly. When you match on an owned value and want to borrow rather than move out, use `ref` [implemented]:

```ruby
s = "hi"
case s
in ref t then puts t     # borrow, don't move
end
puts s                   # still usable
```

For everyday code, matching on a borrow (`case &thing`) is the idiomatic move.

## Custom error types

In Ruby, your application's error cases are exception classes [planned](../../TODO.md#t-exceptions):

```ruby
class NotFound < StandardError
end

def lookup(id: Int) -> Int
  raise NotFound, "not found: #{id}" unless id == 1
  100
end

begin
  puts "ok #{lookup(1)}"
  puts "ok #{lookup(2)}"
rescue NotFound => e
  puts "err #{e.message}"
end
```

Works today: an `enum` that includes the `Error` mixin, returned in a `Result` [deprecated] (replace with an exception class):

```ruby
enum MyErr
  NotFound(id: Int)

  include Error

  def message -> String
    case self
    in MyErr.NotFound(id) then "not found: #{id}"
    end
  end
end

def lookup(id: Int) -> Result[Int, MyErr]
  if id == 1
    Ok(100)
  else
    Err(MyErr.NotFound(id: id))
  end
end

case lookup(1)
in Ok(v) then puts "ok #{v}"
in Err(e) then puts "err #{e.message}"
end
case lookup(2)
in Ok(v) then puts "ok #{v}"
in Err(e) then puts "err #{e.message}"
end
```

Chapter 8 explains what mixins are; for now, `include Error` says "this type is a participant in the error story." Exception classes replace the `Error` mixin.

## Common mistakes

**Leaving out a case.** The compiler does not check today that `case/in` covers every case; the check is [planned](../../TODO.md#t-case-exhaustive). If you only care about one, add `_` as a fallback [deprecated] (`enum`, as above):

```ruby
enum Shape
  Circle(radius: Int)
  Rectangle(width: Int, height: Int)
end

def round_area(s: Shape) -> Int
  case s
  in Shape.Circle(r) then r * r * 3
  in _ then 0
  end
end

puts round_area(Shape.Circle(radius: 2))
```

**Forgetting `Color.` when matching a variant.** Variant names are namespaced under the type. Rejected today [deprecated] (`enum`, as above):

```ruby
enum Color
  Red
  Green
end

def short(c: Color) -> String
  case c
  in Red then "r"       # ERROR: undefined enum variant `.Red`; write Color.Red
  in _ then "other"
  end
end

puts short(Color.Red)
```

**Treating an `Int?` as an `Int`.** When you have `Int?`, you don't yet have an `Int`. Match it, give a default, or narrow it as shown above.

**Using `nil` for a regular reference.** `nil` is the empty case of `T?`, not a generic "no value" you can stick into any variable. References (`&T`) are never `nil`. Use `T?` to express "may or may not have a value."

## Try it

Add a `Yellow` variant to `Color` and a matching arm to `describe`. Then remove the arm again and add `in _ then "other"` — since the compiler does not check coverage yet, the catch-all is what keeps every input handled.

Then change `Shape` to add a `Triangle(base: Int, height: Int)` variant and extend `area` to handle it.

## Recap

- Plain tags are symbols and cases with data are `Data` types (both planned). The `enum` keyword works today and is deprecated.
- `case/in` matches them all, with guards, alternatives and `_`. Add a catch-all: coverage is not checked yet.
- `T?` and `nil` say "a value or nothing". `Option`, `Some` and `unwrap_or` are deprecated.
- Errors become exceptions (planned). `Result`, `Ok` and `Err` work today and are deprecated.
- Custom errors become exception classes; today they are enums that include the `Error` mixin.

**Next:** [Mixins](08-mixins.md) — how to share behaviour between unrelated types.
