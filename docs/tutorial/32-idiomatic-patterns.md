# Idiomatic Patterns

A handful of small patterns show up across well-written Ruxen code. Each one is a shape — not a library — that plays to Ruxen's strengths: owned values, exhaustive matching, and short pure functions. This chapter walks through nine of them. None is novel on its own, but together they form the vocabulary you'll see in the standard library and in any non-trivial Ruxen program.

---

## 1. The builder

When a value has a few required fields and a lot of optional configuration, give it a small `init` for the essentials and a chain of fluent `with_*` methods for the rest. Each setter mutates in place and returns `self`, so calls compose:

```ruxen
class Request
  url: String
  method: String
  timeout_ms: Int

  def init(@url: String)
    self.method = String.from("GET")
    self.timeout_ms = 30_000
  end

  def var with_method(m: String) -> Request
    self.method = m
    self
  end

  def var with_timeout(ms: Int) -> Request
    self.timeout_ms = ms
    self
  end
end

def main
  let r = Request.new(String.from("https://example.com"))
    .with_method(String.from("POST"))
    .with_timeout(5_000)
  puts "#{r.method} #{r.url} timeout=#{r.timeout_ms}"
end
```

Notice three things:

- The setters are `def var` — they mutate in place. Returning `self` is what makes them chainable.
- `init` sets sensible defaults for every non-essential field.
- There's no separate `Builder` class. The value *is* its own builder — the way `OpenOptions` and `Command` work in the standard library.

## 2. Type-state — encoding "what state am I in" in the type

Use a generic parameter to represent the current state. The same class supports different operations depending on which state it's in.

```ruxen
class Phase end

module Phases
  class Draft end
  class Submitted end
  class Approved end
end

class Workflow[P]
  data: String

  def init(@data: String)
  end

  def submit -> Workflow[Phases.Submitted]
    Workflow.new(self.data)
  end
end

extension Workflow[Phases.Submitted]
  def approve -> Workflow[Phases.Approved]
    Workflow.new(self.data)
  end
end
```

Once you call `.submit()` on a `Draft`, you get a `Submitted` and the type system forgets it was ever a draft — there's no way to call `submit` twice on the same workflow. Misordered transitions become compile errors instead of runtime bugs.

## 3. Newtype wrappers

A **newtype** is a single-field struct around an existing type that makes two otherwise-interchangeable values distinct at the type level. The cost is zero (no extra bytes); the protection against mix-ups is real:

```ruxen
newtype UserId(Int)
newtype OrderId(Int)

def cancel_order(id: OrderId)
  puts "cancel order #{id.0}"
end

def main
  let user  = UserId(42)
  let order = OrderId(99)

  cancel_order(order)    # OK
  # cancel_order(user)   # compile error — UserId is not OrderId
end
```

Use newtypes for identifiers (`UserId`, `OrderId`), units (`Meters`, `Seconds`), and any time `Int` or `String` is too easy to confuse with another `Int` or `String`.

## 4. The `From` / conversion style

The standard library uses a consistent vocabulary for type conversions. For infallible cases, define `def self.from(input) -> Self`:

```ruxen
class Url
  raw: String

  def init(@raw: String)
  end

  def self.from(s: &str) -> Url
    Url.new(String.from(s))
  end
end

def main
  let u = Url.from("https://example.com")
  puts u.raw
end
```

For fallible cases, use `def self.from_<kind>(input) -> Result[Self, E]` and pull each error branch into its own helper:

```ruxen
class Port
  value: UInt16

  def init(@value: UInt16)
  end

  def self.out_of_range -> Result[Port, String]
    Err(String.from("port out of range"))
  end

  def self.not_a_number -> Result[Port, String]
    Err(String.from("not a number"))
  end

  def self.from_str(s: &str) -> Result[Port, String]
    match s.parse_int
      Ok(n)  -> Port.classify(n)
      Err(_) -> Port.not_a_number
    end
  end

  def self.classify(n: Int) -> Result[Port, String]
    if n > 0 && n < 65_536
      Ok(Port.new(n as UInt16))
    else
      Port.out_of_range
    end
  end
end
```

Each match arm stays one expression; the construction logic and error formatting live in their own named functions.

## 5. `Option` / `Result` first

The strict pattern that pervades the standard library: any operation that *can* fail returns `Result`; any operation that *can* be empty returns `Option`. Never a sentinel value (`-1`, empty string), never a boolean "did it work":

```ruxen
# YES
def find_user(id: Int) -> Option[User]
def parse_port(s: &str) -> Result[Port, String]

# NO
def find_user(id: Int) -> User              # what does "not found" return?
def parse_port(s: &str) -> Int              # what does -1 mean? 0?
```

At the call site, `match` exhaustively or compose with `?`:

```ruxen
match find_user(42)
  Some(user) -> greet(user)
  nil       -> puts "not found"
end
```

The error path is impossible to forget.

## 6. Short `main`, fat helpers

Keep `main` to the shape "parse args → call one helper → report the result". All real work lives in named functions that return `Result`:

```ruxen
def die(msg: String)
  eputs "tool: #{msg}"
  exit(1)
end

def run(path: &String) -> Result[nil, AppError]
  let text = read_to_string(path)?
  process(&text)?
  Ok(nil)
end

def main
  let argv = args()
  if argv.len < 2
    eputs "usage: tool <file>"
    exit(1)
  end
  match run(argv.get(1).expect!("path"))
    Ok(_)  -> nil
    Err(e) -> die(e.message)
  end
end
```

`main` doesn't `match` on deep details — it funnels errors through one helper. Helpers compose because they all speak the same `Result` dialect.

## 7. RAII via `def drop`

Any class with a `def drop` method runs that method automatically when the value goes out of scope. Use it for resource cleanup — file handles, sockets, locks, FFI handles.

```ruxen
class Timer
  label: String
  start_ns: Int

  def init(@label: String)
    self.start_ns = unix_ns()
  end

  def drop -> nil
    let elapsed = unix_ns() - self.start_ns
    puts "#{self.label}: #{elapsed} ns"
    nil
  end
end

def main
  let _t = Timer.new(String.from("hot loop"))
  # ... work ...
  # `drop` runs at end of main: prints "hot loop: ... ns"
end
```

The pattern: own the resource as a class field; release it in `def drop`. Callers don't have to remember to close anything.

## 8. `match` instead of `if / elsif`

Whenever you're branching on a discrete value, prefer `match`. The compiler forces you to handle every variant:

```ruxen
let label = match status
  Status.Pending     -> "pending"
  Status.Running     -> "running"
  Status.Done(_)     -> "done"
  Status.Failed(msg) -> "failed: #{msg}"
end
```

If you later add a `Status.Cancelled` variant, every `match` that forgot it becomes a compile error — you can't accidentally ship the "if/elsif fell through to else" branch.

## 9. Tiny pure functions, then compose

A function that does one thing and returns a value is trivial to test, trivial to inline mentally, and never has to be re-read. The Ruxen idiom is to prefer many small functions over one medium one — especially when walking enums, where each arm naturally maps to one helper:

```ruxen
def handle_get(path: &String) -> Response { ... }
def handle_post(path: &String, body: &String) -> Response { ... }
def handle_delete(path: &String) -> Response { ... }

def reply(req: Request) -> Response
  match req
    Request.Get(path)        -> handle_get(&path)
    Request.Post(path, body) -> handle_post(&path, &body)
    Request.Delete(path)     -> handle_delete(&path)
  end
end
```

Reading, testing, and editing all get easier the moment you stop inlining bodies into match arms.

---

## 10. Common mistakes

- **Mutable builders without `self` return.** A `def var with_foo(...)` that doesn't `return self` won't chain. The `with_*` setter must end with `self`.
- **Newtype with no inner access.** `newtype UserId(Int)` lets you read the inner via `id.0`. Don't add an explicit accessor — the `.0` field is the idiom.
- **Building deep `if / elsif` chains over enum variants.** Use `match` instead; the compiler will tell you when you forget a variant.
- **Returning sentinels (`-1`, `""`, `false`) instead of `Option` / `Result`.** Every caller has to remember the special value; eventually one forgets. The type-level form forces the check.
- **Doing real work in `main`.** It quickly grows unreadable. Push everything into helpers and keep `main` to argument parsing plus one funnel.

> **Try it:** rewrite a function in your code (any function!) that ends with `if x then ... elsif y then ... else ... end` as a `match`. Notice whether the compiler asks you to add any missing branches you'd forgotten.

---

## Recap

- **Builder** — `init` for essentials, fluent `with_*` setters for the rest.
- **Type-state** — encode the current state in a generic parameter; misordered transitions become compile errors.
- **Newtype** — single-field wrapper to keep two `Int`s (or two `String`s) from being mixed up.
- **`From`** — `def self.from(input)` for infallible, `def self.from_<kind>(input) -> Result` for fallible.
- **`Option` / `Result` first** — never sentinels; the type encodes "could be missing" or "could fail".
- **Short `main`, fat helpers** — argv → helper → one error funnel.
- **`def drop`** — RAII; clean up resources without callers remembering.
- **`match` over `if / elsif`** — exhaustive by construction.
- **Tiny functions** — one job each; pull match-arm bodies into named helpers.

These patterns repeat throughout `library/std`. Reading one of the stdlib packages end to end — `library/std/io/src/open_options.rx` for the builder, `library/std/sync/src/mutex.rx` for RAII, `library/std/option_result` for the conversion style — is the fastest way to internalise them.

**Next:** [Chapter 33 — Editor Setup and Cheatsheet](33-editor-and-cheatsheet.md).
