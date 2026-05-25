# Error Handling

Ruxen has no exceptions. Errors are values — your code receives them, inspects them, and decides what to do. The two key types are `Result[T, E]` (success or error) and `Option[T]` (something or nothing). You met them in Chapter 7; this chapter goes deeper, with the `?` operator that makes chained fallible code readable.

## A first runnable example

```ruxen
def divide(a: Int, b: Int) -> Result[Int, String]
  if b == 0
    Err(String.from("divide by zero"))
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

```bash
ruxen compile divide.rx
./divide
```

Output:

```
ok 5
err divide by zero
```

`Result` always carries one of two cases. `match` makes you handle both.

## `Result[T, E]`

```ruxen
enum Result[T, E]
  Ok(T)
  Err(E)
end
```

`T` is the success type, `E` is the error type. A function that can fail returns `Result[T, E]` and the caller is forced to deal with both branches.

## The `?` operator

Without `?`, chaining fallible calls is painful — every step needs a `match`:

```ruxen
def process(path: &str) -> Result[Data, AppError]
  let text = match read_file(path)
    Ok(t)  -> t
    Err(e) -> return Err(e)
  end
  let parsed = match parse_data(&text)
    Ok(p)  -> p
    Err(e) -> return Err(e)
  end
  validate(parsed)
end
```

`?` collapses each of those `match`/`return` pairs to a single character: "unwrap on `Ok`, return early on `Err`":

```ruxen
def process(path: &str) -> Result[Data, AppError]
  let text = read_file(path)?       # early return on Err
  let parsed = parse_data(&text)?   # early return on Err
  validate(parsed)
end
```

Read it as "this could fail, and if it does, propagate the failure up." `?` is the same shape any time you want to bail out — the caller can chain its own `?` calls to do the same.

## Matching on `Result`

```ruxen
match do_work()
  Ok(value) -> puts "success: #{value}"
  Err(e)    -> puts "error: #{e}"
end
```

## Useful `Result` methods

```ruxen
let result = divide(10, 0)

result.unwrap_or(0)                 # value on Ok, fallback on Err
result.unwrap!                      # value on Ok, panic on Err
result.expect!("must succeed")      # value on Ok, panic with this message on Err
result.map { |n| n * 2 }            # transform the Ok value
result.map_err { |e| wrap(e) }      # transform the Err value
```

`unwrap!` and `expect!` end with `!` — a Ruxen convention that warns "this might crash at runtime." Use them when failure here would mean a logic bug, not when handling user input.

## `Option[T]`

`Option` is the "may or may not have a value" cousin:

```ruxen
def find(id: Int) -> Option[User]
  # ...
end
```

Same methods, same shape:

```ruxen
let maybe_user = find(42)

maybe_user.unwrap_or(default_user)
maybe_user.unwrap!                  # panics on nil
maybe_user.map { |u| u.name }       # transform if Some
```

### Safe navigation (`?.`)

Chain through optionals without nested matches:

```ruxen
let name = find_user(42)?.profile?.display_name
```

If any step is `nil`, the whole chain short-circuits to `nil`.

## Custom error types

For real applications, define an enum that lists the things that can go wrong. Include the `Error` mixin to gain a uniform `.message`:

```ruxen
enum AppError
  NotFound(resource: String)
  InvalidInput(message: String)
  Io(IoError)

  include Error

  def message -> String
    match self
      AppError.NotFound(r)     -> "Not found: #{r}"
      AppError.InvalidInput(m) -> "Invalid: #{m}"
      AppError.Io(e)           -> e.message
    end
  end
end
```

Callers handle the enum just like any other `Result`:

```ruxen
match load_config(path)
  Ok(cfg)                       -> use(cfg)
  Err(AppError.NotFound(r))     -> puts "file missing: #{r}"
  Err(AppError.InvalidInput(m)) -> puts "config invalid: #{m}"
  Err(AppError.Io(e))           -> puts "io: #{e.message}"
end
```

### Automatic conversion through `?`

If you have many sources of errors (file IO, JSON parsing, network calls) you don't want to write a `match` to convert each one into your `AppError`. Include the `Into[AppError]` mixin on the source error type and `?` will convert for you:

```ruxen
extension IoError
  include Into[AppError]

  def consume into -> AppError
    AppError.Io(self)
  end
end

def load(path: &str) -> Result[String, AppError]
  fs.read_to_string(path)?         # IoError → AppError automatically via ?
end
```

## Panic — for bugs, not for expected errors

`panic!` aborts the program with a message. Use it for "this can't happen" cases — broken invariants, exhausted assumptions:

```ruxen
panic!("the queue should never be empty here")
```

Never use `panic!` for normal failure modes — invalid input, missing files, network problems. Those are values; return them as `Result` or `Option` and let the caller decide.

Same goes for `unwrap!` and `expect!` — fine for unit tests and small scripts; risky in long-running services.

## Common mistakes

**Reaching for `unwrap!` to silence the compiler.** Every `unwrap!` is a runtime panic waiting to happen. If you find yourself writing it in non-test code, ask whether `?`, `match`, or `unwrap_or` is a better fit.

**Hand-writing what `?` already does.** A long `match` chain that pulls `Ok` out and returns `Err` is exactly what `?` was built for. Use it.

**Ignoring the error in `Err(_)`.** The error usually carries useful detail. If you really don't care, write `Err(_)`, but consider whether logging it would help future debugging.

**Mixing `panic!` with normal control flow.** Panics are not exceptions — they don't carry data you can match on at a higher level. They mean "abort."

**Forgetting `include Error` on a custom enum.** Without it, your custom enum doesn't fit functions that expect "anything that includes `Error`." It still works as a payload — but you lose the uniform `.message` surface.

## Try it

Take `divide` from the first example and chain two divisions:

```ruxen
def double_divide(a: Int, b: Int, c: Int) -> Result[Int, String]
  let q1 = divide(a, b)?
  let q2 = divide(q1, c)?
  Ok(q2)
end
```

Call it with `(20, 2, 2)` (should print `5`) and with `(20, 0, 2)` (should propagate the `divide by zero` error). Notice you didn't write any `match`.

## Recap

- Errors are values. No exceptions.
- `Result[T, E]` carries `Ok(T)` or `Err(E)`; `Option[T]` carries `Some(T)` or `nil`.
- `?` is the workhorse: unwrap on success, return early on failure.
- `unwrap!`, `expect!`, and `panic!` are loud — use them only when the failure is genuinely a bug.
- Custom error enums + `include Error` give you a uniform `.message`.
- `Into[OtherErr]` on a source error lets `?` convert it automatically.

**Next:** [Generics](12-generics.md) — writing code that works across types without copy-paste.
