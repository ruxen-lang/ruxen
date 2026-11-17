# Async

You want to do I/O without blocking your whole program. Maybe you're writing a network server that needs to handle many connections at once, or a tool that reads a few files in parallel. The naive approach — spawn a thread per connection — works but burns memory and tunes badly past a few thousand connections. **Async/await** is a lighter-weight alternative: a single thread (or small pool) can handle thousands of in-flight I/O operations by suspending each one whenever it would block and resuming it when the data is ready.

`async def` and `.await` are [implemented] and are the working model today. Ruby's Fiber scheduler (`Async do ... end`, `task.wait`) is [planned](../../TODO.md#t-fibers); when it lands, `async def`, `.await`, `Future`, `Poll` and `block_on` become [deprecated].

The new ideas:

- **Future** — a value that *will eventually* produce a result. Not a result yet; more like a recipe for getting one.
- **`async def`** — declare a method whose body can suspend. Calling it gives you back a future, not the final value.
- **`.await`** — inside another `async def`, suspend until a future resolves and grab its value.
- **`block_on(future)`** — from ordinary (synchronous) code, drive a future to completion and return its value.

This chapter starts with the simplest possible example and builds up.

---

## 1. Your first async method

Save as `hello_async.rx`. Example [implemented]:

```ruby
async def make_int -> Int
  42
end

result = block_on(make_int())
puts "#{result}"
```

Run:

```sh
ruxen run hello_async.rx
```

Output:

```text
42
```

What happened:

- `async def make_int` doesn't return an `Int` directly — it returns a *future* that, when driven, eventually produces `42`.
- `block_on(...)` takes that future, runs it to completion on the current thread, and hands back the inner value.

`block_on` is your bridge from sync code to async code. It's what lets the top-level statements (which are ordinary, synchronous code) get the result of `make_int`.

## 2. Chaining with `.await`

Inside an `async def`, call any future-returning method with `.await` to suspend until it resolves. The Ruby form, binding the awaited value with a plain assignment, is [planned](../../TODO.md#t-await-gaps):

```ruby
async def chain -> Int
  a = make_int().await
  b = make_other().await
  a + b
end
```

Works today [deprecated]: the binding needs `let` (replaced by the plain assignment above):

```ruby
async def make_int -> Int
  42
end

async def make_other -> Int
  35
end

async def chain -> Int
  let a = make_int().await
  let b = make_other().await
  a + b
end

puts block_on(chain())
```

Output:

```text
77
```

Each `.await` is a **suspension point**: at that line, `chain` is allowed to pause itself, let other work happen, and resume later when the awaited future is ready. You never see the suspension machinery directly — the `async def` keyword is what turns the body into a future-shaped thing.

> **Try it:** add a third `async def make_third -> Int` that returns `8`, await it inside `chain`, and watch the printed sum change.

## 3. Sleeping without blocking the thread

`Thread.sleep` blocks the OS thread — bad inside async code because it freezes any other in-flight task too. The async equivalent is `Async.sleep`, which registers a timer with the runtime and lets the thread go do other things until the timer fires. Example [deprecated] (`Duration.from_millis`, replaced by numeric seconds [planned](../../TODO.md#t-stdlib-shapes)):

```ruby
block_on(Async.sleep(Duration.from_millis(50)))
puts "awake"
```

From inside an `async def`, use the `.await` form. Works today [deprecated] (the `let` binding and `Duration.from_millis`, replaced by a plain assignment [planned](../../TODO.md#t-await-gaps) and numeric seconds):

```ruby
async def with_delay -> Int
  let done = Async.sleep(Duration.from_millis(50)).await
  42
end
```

**Don't** reach for `Thread.sleep` inside an `async def` — you'll block the runtime thread along with the task.

## 4. Async file I/O

`AsyncFile` mirrors the sync `File` surface, but every operation returns a future. The most common shape from a synchronous caller is `block_on(...)` per step. Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by exceptions [planned](../../TODO.md#t-exceptions)):

```ruby
def finish_write(f: AsyncFile, contents: &String) -> Int
  fv = f
  case block_on(fv.write_all(contents))
  in Ok(_) then 1
  in Err(_) then 0
  end
end

def write_payload(path: &String, contents: &String) -> Int
  case block_on(AsyncFile.create(path))
  in Ok(f) then finish_write(f, contents)
  in Err(_) then 0
  end
end

path = "/tmp/demo.txt"
text = "hello async file\n"
if write_payload(path, text) == 1
  puts "ok"
end
```

Most-used `AsyncFile` methods, all [implemented]:

- `AsyncFile.open(path)` — open for reading.
- `AsyncFile.create(path)` — open for writing (truncates if it exists).
- `.read_to_string` — reads the whole file.
- `.write_all(contents)` — writes the whole buffer.

All return a future of `Result[T, IoError]`.

## 5. Async TCP

`AsyncTcpListener` and `AsyncTcpStream` are the network equivalents. Same pattern — bind, accept, connect, read, write all return futures. Works today [deprecated] (`Result` with `Ok` / `Err` patterns, the tuple type and the tuple field `.0`, replaced by exceptions [planned](../../TODO.md#t-exceptions) and an array pair [planned](../../TODO.md#t-tuple-type)):

```ruby
def finish_echo(s: &var AsyncTcpStream, payload: String) -> Int
  case block_on(s.write(payload))
  in Ok(_) then 1
  in Err(_) then 0
  end
end

def echo(s: &var AsyncTcpStream) -> Int
  case block_on(s.read(64))
  in Ok(payload) then finish_echo(s, payload)
  in Err(_) then 0
  end
end

def handle_pair(pair: (AsyncTcpStream, String)) -> Int
  p = pair
  echo(&var p.0)
end

def accept_one(listener: AsyncTcpListener) -> Int
  l = listener
  case block_on(l.accept)
  in Ok(pair) then handle_pair(pair)
  in Err(_) then 0
  end
end

def server_run -> Int
  case block_on(AsyncTcpListener.bind("127.0.0.1:31729"))
  in Ok(listener) then accept_one(listener)
  in Err(_) then 0
  end
end
```

`l.accept` resolves to a `Result` holding a pair: the stream, and the peer's address as a string. The stream is reached through `&var p.0` so that `read` and `write`, which change the stream, get a mutable borrow (pinned by `tests/release-e2e/cases/727_async_tcp_echo.rx`).

## 6. Async stdin

`AsyncStdin.read_line` resolves to `Result[String, IoError]`. On EOF it returns `Ok("")` rather than an error — useful for "loop until empty input". Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by exceptions [planned](../../TODO.md#t-exceptions)):

```ruby
line = block_on(AsyncStdin.read_line)
case line
in Ok(s) then puts "got: #{s}"
in Err(_) then puts "read error"
end
```

`read_line` takes no arguments, so no parens are needed.

## 7. Writing your own future

For most work, the built-in futures (file I/O, TCP, sleep) plus `async def` are enough. But sometimes you need to write a future by hand — e.g. a custom event source. Include the `Future` mixin and implement `poll`. Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class CountdownFuture
  remaining: Int
  include Future
  type Output = Int

  def init(@remaining: Int)
  end

  def var poll(cx: &var Context) -> Poll[Int]
    if @remaining == 0
      Poll.Ready(0)
    else
      @remaining = @remaining - 1
      Poll.Pending
    end
  end
end
```

Three required pieces:

- `include Future` — opts into the runtime.
- `type Output = T` — what the future eventually produces.
- `def var poll(cx: &var Context) -> Poll[T]` — one step of the state machine. Return `Poll.Ready(v)` to resolve; `Poll.Pending` to ask to be polled again.

Once written, any caller can pass it to `block_on(...)`. Awaiting a hand-written future that was passed into an `async def` as a parameter does not link yet [planned](../../TODO.md#t-await-param-future).

## 8. Common mistakes

- **Calling an `async def` without `.await` or `block_on`.** You get back the future, not the value. Nothing runs. If you see "unused future" warnings or a confusing type mismatch, that's the cause.
- **Using `Thread.sleep` inside async code.** Blocks the runtime thread. Use `Async.sleep` instead.
- **Awaiting from inside a plain `def`.** `.await` is only legal inside `async def`. From a sync method, use `block_on(...)`.
- **Holding a `&var` borrow across an `.await`.** When the method suspends, the borrow is still live but the thread has moved on — a recipe for trouble. Take the value out first, await, then re-acquire. The compiler does not reject it yet [planned](../../TODO.md#t-await-borrow).

> **Try it:** chain three `Async.sleep` calls in one `async def` (50 ms each). Time the whole thing using `Instant.now`. Why is it ~150 ms (sequential) instead of ~50 ms (parallel)? (Hint: futures run sequentially unless explicitly composed in parallel.)

---

## Recap

- A **future** represents a value that will arrive eventually.
- `async def` declares a method whose body returns a future [implemented].
- `.await` (inside `async def`) suspends until a future resolves [implemented].
- `block_on(future)` (from sync code) drives a future to completion [implemented].
- `Async.sleep`, `AsyncFile`, `AsyncTcpListener`, `AsyncStdin` are the built-in awaitable types [implemented].
- For custom futures, `include Future`, declare `type Output`, and implement `poll` [implemented].
- All of this is replaced by the Fiber scheduler once it lands [planned](../../TODO.md#t-fibers).

**Next:** [Chapter 25 — Function Overloads and Optional Arguments](25-overloads-and-defaults.md).
