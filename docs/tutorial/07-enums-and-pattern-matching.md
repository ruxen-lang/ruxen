# Enums and Pattern Matching

## Defining Enums

Enums are algebraic data types (tagged unions). Each variant can optionally carry data.

```riven
enum Direction
  North
  South
  East
  West
end
```

### Variants with Data

```riven
enum Shape
  Circle(radius: Float)
  Rectangle(width: Float, height: Float)
  Triangle(a: Float, b: Float, c: Float)
end

let s = Shape.Circle(5.0)
```

### Generic Enums

```riven
enum Option[T]
  Some(T)
  nil
end

enum Result[T, E]
  Ok(T)
  Err(E)
end
```

## Pattern Matching on Enums

`match` is exhaustive — every variant must be handled:

```riven
def area(shape: &Shape) -> Float
  match shape
    Shape.Circle(r)           -> 3.14159 * r * r
    Shape.Rectangle(w, h)     -> w * h
    Shape.Triangle(a, b, c)   -> do
      let s = (a + b + c) / 2.0
      (s * (s - a) * (s - b) * (s - c)).sqrt
    end
  end
end
```

## Option[T]

Riven has no nullable references for ordinary values. (`nil` exists only as a raw-pointer literal in `unsafe`/FFI contexts — see [Chapter 14](14-ffi.md) and [Chapter 15](15-unsafe.md).) Optional values use `Option[T]`:

```riven
def find_user(id: Int) -> Option[User]
  if id == 42
    Some(User.new("Alice", 30))
  else
    nil
  end
end
```

### Working with Option

```riven
let user = find_user(42)

# Pattern match
match user
  Some(u) -> puts u.name
  nil    -> puts "not found"
end

# If-let
if let Some(u) = find_user(42)
  puts u.name
end

# Safe navigation
let name = find_user(42)?.name       # Option[String]

# Default value
let name = find_user(42).unwrap_or(default_user)

# Panic on nil (use sparingly!)
let name = find_user(42).unwrap!
let name = find_user(42).expect!("user 42 must exist")
```

## Result[T, E]

Fallible operations return `Result`:

```riven
def parse_port(input: &str) -> Result[Int, ParseError]
  match input.trim.parse_int
    Ok(n) if n > 0 && n < 65536 -> Ok(n)
    Ok(n)                        -> Err(ParseError.new("port out of range: #{n}"))
    Err(e)                       -> Err(e)
  end
end
```

### The `?` Operator

`?` propagates errors — returns early on `Err` or `nil`:

```riven
def load_config(path: &str) -> Result[Config, AppError]
  let text = File.read_string(path)?     # returns Err if file fails
  let json = Json.parse(&text)?          # returns Err if parse fails
  Config.from_json(&json)                # returns final Result
end
```

### Custom Error Types

```riven
enum AppError
  NotFound(resource: String)
  Validation(message: String)
  Io(IoError)

  include Error

  def message -> String
    match self
      AppError.NotFound(r)   -> "Not found: #{r}"
      AppError.Validation(m) -> "Validation: #{m}"
      AppError.Io(e)         -> e.message
    end
  end
end
```

## Match Ergonomics

### Ref Bindings

When matching on an owned value, bindings move by default. Use `ref` to borrow instead:

```riven
match some_string
  ref s -> puts s    # borrow, don't move
end
```

When matching on a reference (`&T`), bindings are automatically `ref` — no annotation needed.

### Wildcard and Rest

```riven
match value
  _  -> "matches anything"
end

match record
  User(name, ..) -> name    # ignore remaining fields
end
```
