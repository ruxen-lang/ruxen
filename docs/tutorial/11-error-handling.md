# Error Handling

Errors in Ruxen are Ruby exceptions, checked statically: `raise` reports a failure, `begin ... rescue ... end` handles it, and the compiler knows which exceptions every method can raise. Under the hood the compiler lowers exceptions to ordinary return values, so there is no stack unwinding and no hidden runtime cost. You never write `Result`, `Ok`, `Err` or `?`.

Exceptions are [planned](../../TODO.md#t-exceptions). Until they land, fallible code returns a `Result` and propagates it with the postfix `?` operator; that form works today and is [deprecated] (replace with `raise` and `rescue`). This chapter shows the Ruby form first, then the form that works today. "A value or nothing" is a different job, done by `T?` and `nil` (Chapter 7).

## A first runnable example

The Ruby form [planned](../../TODO.md#t-exceptions):

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

Works today: a `Result` matched with `case/in` [deprecated] (replace with `raise` and `rescue`):

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

```bash
ruxen compile divide.rx
./divide
```

Output:

```text
ok 5
err divide by zero
```

## Raising and rescuing

All of these are [planned](../../TODO.md#t-exceptions):

- `raise "boom"` raises a `RuntimeError`; `raise NotFound, "no user #{id}"` and `raise NotFound.new(id)` raise your own class.
- `begin ... rescue NotFound => e ... rescue => e ... else ... ensure ... end` handles them. `rescue => e` with no class catches `StandardError`, as in Ruby.
- A bare `raise` inside `rescue` re-raises; `retry` restarts the `begin` body.
- A method body can carry its own handlers: `def load ... rescue IOError => e ... ensure ... end`.
- `ensure` runs on both the normal and the raising exit, on the same cleanup path as `drop`.

The Ruby form [planned](../../TODO.md#t-exceptions):

```ruby
class NotFound < StandardError
end

def find_user(id: Int) -> User
  users.find { |u| u.id == id } or raise NotFound, "no user #{id}"
end

attempts = 0
begin
  user = find_user(42)
  puts user.name
rescue NotFound => e
  attempts += 1
  retry if attempts < 3
  warn e.message
ensure
  log.close
end
```

The static rules:

- Each method has a **raise set**: the exception classes it can raise and does not rescue, including the raise sets of the methods it calls. The compiler infers it; it is part of the method's type. (Whether a method may also declare it explicitly is an open question.)
- A `rescue` clause narrows the raise set of its `begin` body.
- An exception that escapes the top level prints its message and class to STDERR and exits with status 1, as Ruby does.

## Propagating errors today: `?`

Exceptions propagate on their own: a method that calls a raising method and does not rescue simply raises too. Until exceptions land, propagation is spelled with the postfix `?` operator on a `Result`: "unwrap on `Ok`, return early on `Err`". Works today [deprecated] (replace with exceptions, which need no operator):

```ruby
def divide(a: Int, b: Int) -> Result[Int, String]
  if b == 0
    Err("divide by zero")
  else
    Ok(a / b)
  end
end

def double_divide(a: Int, b: Int, c: Int) -> Result[Int, String]
  q1 = divide(a, b)?        # early return on Err
  q2 = divide(q1, c)?       # early return on Err
  Ok(q2)
end

case double_divide(20, 2, 2)
in Ok(v) then puts v
in Err(e) then puts "err #{e}"
end
case double_divide(20, 0, 2)
in Ok(v) then puts v
in Err(e) then puts "err #{e}"
end
```

Output:

```text
5
err divide by zero
```

## `Result` helper methods

These work today and are [deprecated] with `Result` (replace them with `rescue`, `x || default` and `x or raise ...`):

```ruby
def divide(a: Int, b: Int) -> Result[Int, String]
  if b == 0
    Err("divide by zero")
  else
    Ok(a / b)
  end
end

def show
  a = divide(10, 0).unwrap_or(0)                # value on Ok, fallback on Err
  b = divide(10, 2).unwrap!                     # value on Ok, panic on Err
  c = divide(10, 2).expect!("must succeed")     # value on Ok, panic with this message on Err
  d = divide(10, 2).map { |n| n * 2 }           # transform the Ok value
  e = divide(10, 0).map_err { |e| "wrapped: #{e}" }  # transform the Err value
  puts a
  puts b
  puts c
end

show()
```

## Values that may be missing

"No value here" is not an error: it is `T?` and `nil` (Chapter 7) [implemented]:

```ruby
def first_word(s: &String) -> String?
  s.empty? ? nil : s
end
```

Safe navigation chains through optionals without nested matches: `find_user(42)&.profile&.name` short-circuits to `nil` if any step is `nil` [implemented]. Ruby's `name = user&.name || "guest"` default needs truthiness of `T?` [planned](../../TODO.md#t-truthiness) (today `||` requires `Bool` operands, E0731), and "must be present" is `find(id) or raise NotFound` [planned](../../TODO.md#t-exceptions).

## Custom error types

For real applications, define one exception class per thing that can go wrong [planned](../../TODO.md#t-exceptions):

```ruby
class NotFound < StandardError
end

class InvalidInput < StandardError
end

def load_config(path: &String) -> Int
  raise NotFound, "Not found: #{path}" unless path == "a"
  1
end

begin
  puts load_config("b")
rescue NotFound => e
  puts "file missing: #{e.message}"
rescue InvalidInput => e
  puts "config invalid: #{e.message}"
end
```

Works today: an `enum` that includes the `Error` mixin, in a `Result` [deprecated] (replace with exception classes):

```ruby
enum AppError
  NotFound(resource: String)
  InvalidInput(message: String)

  include Error

  def message -> String
    case self
    in AppError.NotFound(r) then "Not found: #{r}"
    in AppError.InvalidInput(m) then "Invalid: #{m}"
    end
  end
end

def load_config(path: &String) -> Result[Int, AppError]
  if path == "a"
    Ok(1)
  else
    Err(AppError.NotFound(resource: path.clone))
  end
end

case load_config("b")
in Ok(cfg) then puts cfg
in Err(AppError.NotFound(r)) then puts "file missing: #{r}"
in Err(AppError.InvalidInput(m)) then puts "config invalid: #{m}"
end
```

### Converting one error into another

With exceptions there is nothing to convert: a `rescue` names the classes it handles, and the rest propagate. Today, `?` converts a source error into the caller's error type when the source includes `Into[AppError]`. Works today [deprecated] (`Into` goes away with `?`):

```ruby
enum AppError
  Io(code: Int)
end

enum IoFail
  Closed(code: Int)

  include Into[AppError]

  def consume into -> AppError
    case self
    in IoFail.Closed(c) then AppError.Io(c)
    end
  end
end

def read_it(ok: Bool) -> Result[String, IoFail]
  if ok
    Ok("data")
  else
    Err(IoFail.Closed(code: 5))
  end
end

def load(ok: Bool) -> Result[String, AppError]
  text = read_it(ok)?         # IoFail → AppError automatically via ?
  Ok(text)
end

case load(false)
in Ok(t) then puts t
in Err(AppError.Io(c)) then puts "io #{c}"
end
```

## Aborting — for bugs, not for expected errors

`abort "msg"` prints the message to STDERR and ends the program with status 1; `exit(1)` and `exit!(1)` end it with a status of your choice [implemented]:

```ruby
abort("the queue should never be empty here")
```

Use it for "this can't happen" cases — broken invariants, exhausted assumptions. The parenless `exit 1` is [planned](../../TODO.md#t-f45); write `exit(1)` today. `panic!("msg")` is [deprecated]: replace it with `raise "msg"` when the failure is recoverable, or `abort "msg"` when it is not.

Never abort for normal failure modes — invalid input, missing files, network problems. Raise an exception (today: return a `Result`) and let the caller decide. The same goes for `unwrap!` and `expect!` — fine for unit tests and small scripts; risky in long-running services.

## Common mistakes

**Reaching for `unwrap!` to silence the compiler.** Every `unwrap!` is a runtime panic waiting to happen. In non-test code, propagate the error instead (`?` today, nothing at all once exceptions land).

**Hand-writing propagation.** A long `case` chain that pulls `Ok` out and returns `Err` is exactly what `?` was built for today, and what exceptions make unnecessary.

**Ignoring the error.** The error usually carries useful detail. If you really don't care, `in Err(_)` (today) or a bare `rescue` (planned) drops it, but consider whether logging it would help future debugging.

**Mixing aborts with normal control flow.** `abort` and `panic!` are not exceptions — they don't carry data you can rescue at a higher level. They mean "stop the program."

## Try it

Take `divide` from the first example and chain two divisions. With exceptions you will write it as [planned](../../TODO.md#t-exceptions):

```ruby
def double_divide(a: Int, b: Int, c: Int) -> Int
  divide(divide(a, b), c)
end
```

Today, build the `Result` version from "Propagating errors today" above. Call it with `(20, 2, 2)` (should print `5`) and with `(20, 0, 2)` (should propagate the `divide by zero` error). Notice you didn't write a `case` inside `double_divide`.

## Recap

- Errors are Ruby exceptions: `raise`, `begin/rescue/else/ensure`, `retry`, exception classes (planned).
- The compiler infers each method's raise set and checks it statically.
- Today, fallible methods return `Result[T, E]` and propagate with `?`; both are deprecated.
- `T?` and `nil` are for missing values, not errors.
- `abort` and `exit` stop the program; `panic!` is deprecated.

**Next:** [Generics](12-generics.md) — writing code that works across types without copy-paste.
