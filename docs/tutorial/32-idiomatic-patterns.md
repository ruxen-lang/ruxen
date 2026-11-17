# Idiomatic Patterns

A handful of small patterns show up across well-written Ruxen code. Each one is a shape — not a library — that plays to Ruxen's strengths: owned values, `case/in` branching, and short pure methods. This chapter walks through nine of them. None is novel on its own, but together they form the vocabulary you'll see in the standard library and in any non-trivial Ruxen program.

Several patterns lean on features that are still being built. Each section shows the Ruby form with its status, and when that form is not built yet, a version that works today.

---

## 1. The builder

When a value has a few required fields and a lot of optional configuration, give it a small constructor for the essentials and a chain of fluent `with_*` methods for the rest. Each setter changes the receiver and returns `self`, so calls compose. Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class Request
  url: String
  method: String
  timeout_ms: Int

  def init(@url: String)
    @method = "GET"
    @timeout_ms = 30_000
  end

  def var with_method(m: String) -> Request
    @method = m
    self
  end

  def var with_timeout(ms: Int) -> Request
    @timeout_ms = ms
    self
  end
end

r = Request.new("https://example.com")
  .with_method("POST")
  .with_timeout(5_000)
puts "#{r.method} #{r.url} timeout=#{r.timeout_ms}"
```

Notice three things:

- The setters are `def var` [implemented]: they may change `self`. Returning `self` is what makes them chainable.
- The constructor sets sensible defaults for every non-essential field.
- There's no separate `Builder` class. The value *is* its own builder — the way `OpenOptions` and `Command` work in the standard library.

## 2. Type-state — encoding "what state am I in" in the type

Use a generic parameter to represent the current state, and add methods only for the state where they make sense. In Ruby form you reopen the class for one type argument, which is [planned](../../TODO.md#t-oq) (OQ-6, reopening with a bound or a type argument):

```ruby
module Phases
  class Draft
  end
  class Submitted
  end
  class Approved
  end
end

class Workflow[P]
  data: String

  def initialize(@data: String)
  end

  def submit -> Workflow[Phases::Submitted]
    Workflow.new(@data.clone)
  end
end

class Workflow[Phases::Submitted]
  def approve -> Workflow[Phases::Approved]
    Workflow.new(@data.clone)
  end
end
```

The goal: once you call `submit` on a draft you hold a `Workflow[Phases::Submitted]`, and only that type has `approve`, so a misordered transition is a compile error instead of a runtime bug.

Works today [deprecated] (`extension` and `def init`, replaced by reopening the class and `def initialize`):

```ruby
module Phases
  class Draft
  end
  class Submitted
  end
  class Approved
  end
end

class Workflow[P]
  data: String

  def init(@data: String)
  end

  def submit -> Workflow[Phases.Submitted]
    Workflow.new(@data.clone)
  end
end

extension Workflow[Phases.Submitted]
  def approve -> Workflow[Phases.Approved]
    Workflow.new(@data.clone)
  end
end

w = Workflow[Phases.Draft].new("doc")
s = w.submit
a = s.approve
```

This compiles, but it does not enforce the state yet: the type checker also accepts `approve` on the draft `w` [known issue](../../TODO.md#t-extension-type-arg). Treat the type parameter as documentation until the planned form lands.

## 3. Value wrappers (newtypes)

A **newtype** is a one-field value around an existing type that makes two otherwise-interchangeable values distinct. Use one for identifiers (`UserId`, `OrderId`), units (`Meters`, `Seconds`), and any time an `Int` or `String` is too easy to confuse with another. The Ruby form is a one-field `Data` [planned](../../TODO.md#t-data-define):

```ruby
UserId = Data.define(value: Int)
OrderId = Data.define(value: Int)

def cancel_order(id: OrderId)
  puts "cancel order #{id.value}"
end

user = UserId.new(value: 42)
order = OrderId.new(value: 99)
cancel_order(order)      # OK
# cancel_order(user)     # meant to be a compile error: UserId is not OrderId
```

The old `newtype UserId(Int)` with `id.0` access is [deprecated], replaced by the `Data` form above. Works today [deprecated] (`def init`, replaced by `Data.define`): a small class per wrapper.

```ruby
class UserId
  value: Int

  def init(@value: Int)
  end
end

class OrderId
  value: Int

  def init(@value: Int)
  end
end

def cancel_order(id: &OrderId)
  puts "cancel order #{id.value}"
end

user = UserId.new(42)
order = OrderId.new(99)
cancel_order(order)
```

The protection relies on call arguments being type-checked, and today they are not: `cancel_order(user)` is accepted [planned](../../TODO.md#t-d14). Keep the wrappers anyway; they document intent and start catching mistakes once that check lands.

## 4. Conversions

Ruby converts with `to_*` methods on the source value (`"42".to_i`, `3.to_f`, `obj.to_s`) [implemented], and builds a value from another with a class method such as `Url.parse(text)` [implemented]. A conversion that can fail returns `nil` instead of a value; the return type ends in `?`. Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)) and the `Some` pattern [deprecated] (replace with a plain binding after `in nil` once the [known issue](../../TODO.md#t-case-in-nil) is fixed):

```ruby
class Port
  value: Int

  def init(@value: Int)
  end

  def self.parse(s: &String) -> Port?
    n = s.to_i
    if n > 0 && n < 65_536
      Port.new(n)
    else
      nil
    end
  end
end

case Port.parse("8080")
in Some(port) then puts "port #{port.value}"
in nil then puts "bad port"
end
puts Port.parse("99999").nil?
```

Output:

```text
port 8080
true
```

A conversion that should explain *why* it failed raises an exception in Ruby (`Integer("x")` raises `ArgumentError`), which is [planned](../../TODO.md#t-exceptions). Today such a method returns a `Result` and the caller matches `Ok(...)` / `Err(...)` with `case/in` [deprecated]; section 6 shows one. The old spellings `def self.from(input)` and `def self.from_str(input) -> Result[Self, E]` are [deprecated], replaced by `parse` and `to_*` methods.

## 5. Make "missing" and "failed" part of the type

Never return a sentinel value (`-1`, an empty string) or a boolean "did it work". A lookup that can come back empty returns `T?` [implemented]; an operation that can fail raises [planned](../../TODO.md#t-exceptions):

```ruby
# YES
def find_user(id: Int) -> User?               # nil when there is no such user
def parse_port(s: &String) -> Port            # raises ArgumentError on bad input

# NO
def find_user(id: Int) -> User                # what does "not found" return?
def parse_port(s: &String) -> Int             # what does -1 mean? 0?
```

At the call site, test the optional value before you use it (`user.nil?`, or `case/in` as in section 4) [implemented], so the "not found" path is impossible to forget. Until exceptions land, a method that can fail returns `Result[T, E]` [deprecated], as in section 6.

## 6. Short top level, fat helpers

Keep the top of a script to the shape "read the arguments, call one helper, report the result". All real work lives in named methods. Ruby form, [planned](../../TODO.md#t-exceptions) (exceptions with `begin` / `rescue`):

```ruby
def line_count(path: &String) -> Int
  File.read(path).lines.size
end

argv = Env.args
if argv.size < 2
  warn "usage: tool <file>"
  exit(1)
end
begin
  puts "#{line_count(argv[1])} lines"
rescue => e
  warn "tool: #{e.message}"
  exit(1)
end
```

Works today [deprecated] (`Result` with `Ok` / `Err` and the `as` cast, replaced by exceptions and `size` returning `Int` [planned](../../TODO.md#t-size-int)):

```ruby
def line_count(path: &String) -> Result[Int, String]
  case FS.read_to_string(path)
  in Ok(text) then Ok(text.lines.size as Int)
  in Err(_) then Err("cannot read #{path}")
  end
end

argv = Env.args
if argv.size < 2
  warn "usage: tool <file>"
  exit(1)
end
case line_count(argv[1])
in Ok(n) then puts "#{n} lines"
in Err(msg) then
  warn "tool: #{msg}"
  exit(1)
end
```

The top level doesn't dig into details — it funnels every error through one place. The older shape with `def main`, `args()`, `eputs` and the `?` operator is [deprecated], replaced by top-level statements, `Env.args`, `warn` and exceptions.

## 7. Cleanup via `def drop`

A class with a `def drop` method runs it automatically when the value goes out of scope [implemented]. Use it for resource cleanup — file handles, sockets, locks, C handles. Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class Timer
  label: String
  start_ns: Int

  def init(@label: String)
    @start_ns = Time.unix_ns
  end

  def drop -> nil
    elapsed = Time.unix_ns - @start_ns
    puts "#{@label}: #{elapsed} ns"
    nil
  end
end

def work
  t = Timer.new("hot loop")
  puts "working"
end

work
```

Output (the number varies):

```text
working
hot loop: 1250 ns
```

The pattern: own the resource as a field and release it in `def drop`, so callers don't have to remember to close anything. Ruby's `ensure` and block-scoped resources (`File.open(path) do |f| ... end`) built on the same drop are [planned](../../TODO.md#t-exceptions).

## 8. `case/in` instead of `if / elsif`

Whenever you're branching on a discrete value, prefer `case/in` [implemented]. In Ruby form the variants are `Data` classes grouped in a module, and the compiler checks that every variant has an arm, which is [planned](../../TODO.md#t-oq) (OQ-1, how a module declares its closed set of variants):

```ruby
module Status
  Pending = Data.define
  Running = Data.define
  Failed = Data.define(msg: String)
end

def label(s: Status) -> String
  case s
  in Status::Pending then "pending"
  in Status::Running then "running"
  in Status::Failed(msg:) then "failed: #{msg}"
  end
end
```

Works today [deprecated] (`enum` and `Status.Running`, replaced by the `Data` variants above):

```ruby
enum Status
  Pending
  Running
  Failed(msg: String)
end

def label(s: &Status) -> String
  case s
  in Status.Pending then "pending"
  in Status.Running then "running"
  in Status.Failed(msg) then "failed: #{msg}"
  end
end

puts label(Status.Running)
puts label(Status.Failed("disk full"))
```

Output:

```text
running
failed: disk full
```

Today the type checker does not report a missing arm ([planned](../../TODO.md#t-case-exhaustive)), so don't rely on it to catch a forgotten variant until the planned check lands. The `match v ... pat -> expr ... end` expression is [deprecated], replaced by `case/in`.

## 9. Tiny pure methods, then compose

A method that does one thing and returns a value is trivial to test and never has to be re-read. Prefer many small methods over one medium one — especially when branching, where each arm naturally maps to one helper. Example [implemented]:

```ruby
def handle_get(path: &String) -> String = "GET #{path}"
def handle_post(path: &String, body: &String) -> String = "POST #{path} with #{body}"
def handle_delete(path: &String) -> String = "DELETE #{path}"

def reply(verb: &String, path: &String, body: &String) -> String
  case verb
  in "GET" then handle_get(path)
  in "POST" then handle_post(path, body)
  in "DELETE" then handle_delete(path)
  in _ then "405 #{verb}"
  end
end

puts reply("GET", "/users", "")
puts reply("POST", "/users", "name=ada")
puts reply("PATCH", "/users", "")
```

Output:

```text
GET /users
POST /users with name=ada
405 PATCH
```

The one-line methods use the endless `def name(...) = expr` form [implemented]. Reading, testing, and editing all get easier the moment you stop inlining bodies into `case` arms.

---

## 10. Common mistakes

- **Builder setters that don't return `self`.** A `def var with_foo(...)` that doesn't end with `self` won't chain.
- **Reaching into a wrapper with `.0`.** The tuple-style `id.0` belongs to the [deprecated] `newtype`; give the wrapper a named field (`id.value`).
- **Building deep `if / elsif` chains over a fixed set of cases.** Use `case/in` instead.
- **Returning sentinels (`-1`, `""`, `false`) instead of `nil` or an error.** Every caller has to remember the special value; eventually one forgets. `T?` makes the check part of the type.
- **Doing real work at the top level.** It quickly grows unreadable. Push everything into helpers and keep the top to argument handling plus one funnel.

> **Try it:** rewrite a method in your code that ends with `if x ... elsif y ... else ... end` as a `case/in`. Does it read more clearly?

---

## Recap

- **Builder** — a constructor for essentials, fluent `with_*` setters for the rest [implemented].
- **Type-state** — encode the current state in a generic parameter; enforced transitions are [planned](../../TODO.md#t-oq).
- **Value wrappers** — a one-field `Data` [planned](../../TODO.md#t-data-define) (a small class today) keeps two `Int`s from being mixed up.
- **Conversions** — `to_*` methods and `parse` class methods returning `T?` [implemented]; raising conversions [planned](../../TODO.md#t-exceptions).
- **No sentinels** — `T?` for "could be missing" [implemented], exceptions for "could fail" [planned](../../TODO.md#t-exceptions).
- **Short top level, fat helpers** — arguments, then one helper, then one error funnel.
- **`def drop`** — clean up resources without callers remembering [implemented].
- **`case/in` over `if / elsif`** [implemented]; exhaustive checks over `Data` variants [planned](../../TODO.md#t-oq).
- **Tiny methods** — one job each; pull `case` arm bodies into named helpers.

These patterns repeat throughout `library/std`. Reading one of the stdlib packages end to end — `library/std/io/src/open_options.rx` for the builder, `library/std/sync/src/mutex.rx` for cleanup in `def drop` — is the fastest way to internalise them.

**Next:** [Chapter 33 — Editor Setup and Cheatsheet](33-editor-and-cheatsheet.md).
