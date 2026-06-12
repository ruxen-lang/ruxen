# Enums and Pattern Matching

An **enum** ("enumeration") is a type that can be one of a fixed set of named cases — each called a **variant**. A `TrafficLight` is `Red`, `Yellow`, or `Green`; an HTTP response is `Success`, `Redirect`, `ClientError`, or `ServerError`. Enums make "this OR that" structural — the compiler can check you've handled every case.

The same enum mechanism gives you Ruxen's `Option` (a value or nothing) and `Result` (a value or an error). That's most of error handling, covered here at a tour level and again in Chapter 11.

## A first runnable example

```ruxen
enum Color
  Red
  Green
  Blue
end

def describe(c: Color) -> String
  match c
    Color.Red   -> "red"
    Color.Green -> "green"
    Color.Blue  -> "blue"
  end
end

def main
  puts describe(Color.Red)
  puts describe(Color.Green)
  puts describe(Color.Blue)
end
```

```bash
ruxen compile colors.rx
./colors
```

Output:

```
red
green
blue
```

The `match` is **exhaustive** — if you delete one of the arms, the program won't compile.

## Variants that carry data

A variant can carry payload — different variants can carry different things:

```ruxen
enum Shape
  Circle(radius: Int)
  Rectangle(width: Int, height: Int)
end

def area(s: Shape) -> Int
  match s
    Shape.Circle(r)        -> r * r * 3
    Shape.Rectangle(w, h)  -> w * h
  end
end

def main
  let c = Shape.Circle(radius: 5)
  let r = Shape.Rectangle(width: 4, height: 6)
  puts "#{area(c)}"
  puts "#{area(r)}"
end
```

You can construct a variant with positional arguments (`Shape.Circle(5)`) or named arguments (`Shape.Circle(radius: 5)`). The pattern in `match` binds the payload to a name you choose.

## Methods on enums

Enums can have methods, just like classes:

```ruxen
enum Priority
  Low
  Medium
  High

  def label -> String
    match self
      Priority.Low    -> "low"
      Priority.Medium -> "medium"
      Priority.High   -> "high"
    end
  end
end

def main
  puts Priority.Low.label
  puts Priority.Medium.label
  puts Priority.High.label
end
```

## `Option[T]` — a value or nothing

Many languages have `null` to say "no value here." Ruxen doesn't — for ordinary code, "maybe" is expressed with the `Option[T]` enum:

```ruxen
enum Option[T]
  Some(T)
  nil
end
```

A function that might not have a result returns `Option[T]`:

```ruxen
def find(n: Int) -> Int?
  if n > 0
    n
  else
    nil
  end
end

def main
  match find(5)
    Some(v) -> puts "got #{v}"
    nil     -> puts "nothing"
  end
end
```

`Int?` is a shorthand for `Option[Int]`. The bare value (`n` on its own, no `Some(n)`) is implicitly wrapped in `Some` at a return position — that's why the `if n > 0 ... n` branch works.

### Working with `Option`

```ruxen
def main
  let some_v: Int? = 5
  let none_v: Int? = nil

  let a = some_v.map { |n| n * 10 }.unwrap_or(0)
  let b = none_v.map { |n| n * 10 }.unwrap_or(0)
  puts "#{a}"     # 50
  puts "#{b}"     # 0
end
```

A quick tour of common methods:

- `.unwrap_or(default)` — give me the value, or this default.
- `.unwrap!` — give me the value, or crash. (The `!` is a Ruxen convention: this can panic.)
- `.map { |v| ... }` — transform the value if present.
- `if let Some(v) = opt` — peek without writing a full `match`.

## `Result[T, E]` — success or error

Fallible operations return `Result[T, E]` — either an `Ok` carrying the success value, or an `Err` carrying an error:

```ruxen
def divide(a: Int, b: Int) -> Result[Int, String]
  if b == 0
    Err("divide by zero")
  else
    Ok(a / b)
  end
end

def main
  match divide(10, 2)
    Ok(v)  -> puts "ok #{v}"
    Err(e) -> puts "err #{e}"
  end
  match divide(10, 0)
    Ok(v)  -> puts "ok #{v}"
    Err(e) -> puts "err #{e}"
  end
end
```

Output:

```
ok 5
err divide by zero
```

There's a dedicated chapter on error handling ([Chapter 11](11-error-handling.md)) covering the `?` operator that chains fallible calls together; this much is enough to start.

## Generic enums

The `[T]` syntax makes enums generic, just like functions and classes:

```ruxen
enum MyOpt[T]
  Has(T)
  Empty
end

def describe(o: MyOpt[Int]) -> String
  match o
    MyOpt.Has(n) -> "some #{n}"
    MyOpt.Empty  -> "none"
  end
end

def main
  let a = MyOpt.Has(42)
  let b: MyOpt[Int] = MyOpt.Empty
  puts "#{describe(a)}"
  puts "#{describe(b)}"
end
```

`Option[T]` and `Result[T, E]` are exactly this pattern — generic enums built into the standard library.

## Pattern matching extras

### Guards

A guard is an extra `if` filter on a match arm:

```ruxen
match score
  n if n >= 90 -> "A"
  n if n >= 80 -> "B"
  _            -> "F"
end
```

### Or-patterns

Combine alternatives with `|`:

```ruxen
match day
  "Saturday" | "Sunday" -> "weekend"
  _                     -> "weekday"
end
```

### Wildcard

`_` matches anything and binds nothing:

```ruxen
match value
  Some(_) -> "something"
  nil     -> "nothing"
end
```

### Borrowing matches

When you match on a borrow (`&T`), the bindings inside patterns are themselves borrows — no need to write `ref` explicitly. When you match on an owned value and want to borrow rather than move out, use `ref`:

```ruxen
match owned_string
  ref s -> puts s     # borrow, don't move
end
```

For everyday code, matching on a borrow (`match &thing`) is the idiomatic move.

## Custom error types

You'll define enums for your application's error cases — and include the `Error` mixin to give them a consistent `.message` surface:

```ruxen
enum MyErr
  NotFound(id: Int)

  include Error

  def message -> String
    match self
      MyErr.NotFound(id) -> "not found: #{id}"
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

def main
  match lookup(1)
    Ok(v)  -> puts "ok #{v}"
    Err(e) -> puts "err #{e.message}"
  end
  match lookup(2)
    Ok(v)  -> puts "ok #{v}"
    Err(e) -> puts "err #{e.message}"
  end
end
```

Chapter 8 explains what mixins are; for now, `include Error` says "this enum is a participant in the error story."

## Common mistakes

**Non-exhaustive `match`.** The compiler insists you handle every variant. If you only care about one, use `_` as a fallback:

```ruxen
match shape
  Shape.Circle(r) -> r * r * 3
  _               -> 0
end
```

**Forgetting `Color.` when matching a variant.** Variant names are namespaced under the enum:

```ruxen
match c
  Red -> ...        # ERROR: write Color.Red
end
```

**Treating `Some(n)` as `n`.** When you have `Option[Int]`, you don't yet have an `Int` — you have an `Option`. Pattern-match it, call `.unwrap_or(default)`, or use the `?` operator (Chapter 11) to get at the inner value.

**Using `nil` for a regular reference.** `nil` is the empty case of `Option`, not a generic "no value" you can stick into any binding. Use `Option[T]` to express "may or may not have a value."

## Try it

Add a `Yellow` variant to `Color`. Recompile — the `describe` function fails because the match is no longer exhaustive. Add the missing arm and rerun.

Then change `Shape` to add a `Triangle(base: Int, height: Int)` variant and extend `area` to handle it.

## Recap

- Enums are types whose values are one of a fixed set of named **variants**.
- Variants can carry data — different variants can carry different shapes of data.
- `match` is exhaustive — the compiler enforces handling every case.
- `Option[T]` (`Some(v)` or `nil`) replaces nullable references.
- `Result[T, E]` (`Ok(v)` or `Err(e)`) is how fallible operations report success or failure.
- Custom error enums include the `Error` mixin to expose a uniform `.message`.

**Next:** [Mixins](08-mixins.md) — how to share behaviour between unrelated types.
