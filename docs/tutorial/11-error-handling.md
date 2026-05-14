# Error Handling

Riven has **no exceptions**. All errors are values, handled through `Result[T, E]` and `Option[T]`.

## Result[T, E]

Functions that can fail return `Result`:

```riven
def parse_number(input: &str) -> Result[Int, String]
  match input.trim.parse_int
    Ok(n)  -> Ok(n)
    Err(_) -> Err(String.new("not a number: #{input}"))
  end
end
```

### The `?` Operator

`?` is the primary way to handle errors. It returns early on `Err`, unwraps on `Ok`:

```riven
def process(path: &str) -> Result[Data, AppError]
  let text = read_file(path)?          # early return if Err
  let parsed = parse_data(&text)?      # early return if Err
  validate(parsed)                      # returns Result
end
```

Without `?`, you'd have to write:

```riven
def process(path: &str) -> Result[Data, AppError]
  let text = match read_file(path)
    Ok(t)  -> t
    Err(e) -> return Err(e)
  end
  # ...
end
```

### Matching on Result

```riven
match do_work()
  Ok(value) -> puts "Success: #{value}"
  Err(e)    -> puts "Error: #{e.message}"
end
```

### Result Methods

```riven
let result = parse_number("42")

result.unwrap_or(0)                # 42 on Ok, 0 on Err
result.unwrap!                     # panics on Err
result.expect!("must be valid")    # panics with message on Err
result.map { |n| n * 2 }          # transform Ok value
result.map_err { |e| wrap(e) }    # transform Err value
```

## Option[T]

For values that may or may not exist:

```riven
def find(id: Int) -> Option[User]
  # ...
end
```

### Option Methods

```riven
let maybe_user = find(42)

maybe_user.unwrap_or(default)      # value or default
maybe_user.unwrap!                 # panics on nil
maybe_user.map { |u| u.name }     # transform if Some
```

### Safe Navigation (`?.`)

Chain through optional values:

```riven
let name = find_user(42)?.profile?.display_name

# Equivalent to:
let name = match find_user(42)
  Some(user) -> match user.profile
    Some(profile) -> profile.display_name
    nil -> nil
  end
  nil -> nil
end
```

## Custom Error Types

Define your own error types as enums. Include the `Error` mixin to participate in the standard error surface.

```riven
enum AppError
  NotFound(resource: String)
  InvalidInput(message: String)
  Io(IoError)

  include Error

  def message -> String
    match self
      AppError.NotFound(r)      -> "Not found: #{r}"
      AppError.InvalidInput(m)  -> "Invalid: #{m}"
      AppError.Io(e)            -> e.message
    end
  end
end
```

### Error Conversion

Include the `Into[AppError]` mixin on the source error to enable automatic conversion through `?`:

```riven
extension IoError
  include Into[AppError]

  def consume into -> AppError
    AppError.Io(self)
  end
end

# Now ? automatically converts IoError to AppError
def load(path: &str) -> Result[String, AppError]
  File.read_string(path)?    # IoError converted to AppError
end
```

## Panic (Unrecoverable)

For bugs and invariant violations — not for expected errors:

```riven
panic!("this should never happen")

let value = option.unwrap!                  # panics on nil
let value = result.expect!("must succeed")  # panics on Err
```

`unwrap!` and `expect!` use `!` suffix to signal danger (design principle P1).
