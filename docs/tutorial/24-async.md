# Async

You want to do I/O without blocking your whole program. Maybe you're writing a network server that needs to handle many connections at once, or a tool that reads a few files in parallel. The naive approach — spawn a thread per connection — works but burns memory and tunes badly past a few thousand connections. **Async/await** is a lighter-weight alternative: a single thread (or small pool) can handle thousands of in-flight I/O operations by suspending each one whenever it would block and resuming it when the data is ready.

Three new ideas:

- **Future** — a value that *will eventually* produce a result. Not a result yet; more like a recipe for getting one.
- **`async def`** — declare a function whose body can suspend. Calling it gives you back a future, not the final value.
- **`.await`** — inside another `async def`, suspend until a future resolves and grab its value.
- **`block_on(future)`** — from ordinary (synchronous) code, drive a future to completion and return its value.

This chapter starts with the simplest possible example and builds up.

---

## 1. Your first async function

Save as `hello_async.rx`:

```ruxen
async def make_int() -> Int
  42
end

def main
  let result = block_on(make_int())
  puts "#{result}"
end
```

Run:

```bash
ruxen run hello_async.rx
```

Output:

```
42
```

What happened:

- `async def make_int()` doesn't return an `Int` directly — it returns a *future* that, when driven, eventually produces `42`.
- `block_on(...)` takes that future, runs it to completion on the current thread, and hands back the inner value.

`block_on` is your bridge from sync code to async code. It's what lets `main` (which is a plain `def`) get the result of `make_int`.

## 2. Chaining with `.await`

Inside an `async def`, call any future-returning function with `.await` to suspend until it resolves:

```ruxen
async def make_int() -> Int
  42
end

async def make_other() -> Int
  35
end

async def chain() -> Int
  let a = make_int().await
  let b = make_other().await
  a + b
end

def main
  puts "#{block_on(chain())}"
end
```

Output:

```
77
```

Each `.await` is a **suspension point**: at that line, `chain` is allowed to pause itself, let other work happen, and resume later when the awaited future is ready. You never see the suspension machinery directly — the `async def` keyword is what turns the body into a future-shaped thing.

> **Try it:** add a third `async def make_third() -> Int { 8 }`, await it inside `chain`, and watch the printed sum change.

## 3. Sleeping without blocking the thread

`Thread.sleep` blocks the OS thread — bad inside async code because it freezes any other in-flight task too. The async equivalent is `Async.sleep`, which registers a timer with the runtime and lets the thread go do other things until the timer fires:

```ruxen
use std.time.Duration

def main
  block_on(Async.sleep(Duration.from_millis(50)))
  puts "awake"
end
```

From inside an async fn, use the `.await` form:

```ruxen
use std.time.Duration

async def with_delay() -> Int
  let _done = Async.sleep(Duration.from_millis(50)).await
  42
end
```

**Don't** reach for `Thread.sleep` inside an `async def` — you'll block the runtime thread along with the task.

## 4. Async file I/O

`std.async_fs.AsyncFile` mirrors the sync `std.io.File` surface, but every operation returns a future. The most common shape from a synchronous caller is `block_on(...)` per step:

```ruxen
use std.async_fs.AsyncFile

def write_payload(path: &String, contents: &String) -> Int
  let opened = block_on(AsyncFile.create(path))
  match opened
    Ok(f)  -> finish_write(f, contents)
    Err(_) -> 0
  end
end

def finish_write(f: AsyncFile, contents: &String) -> Int
  var fv = f
  match block_on(fv.write_all(contents))
    Ok(_)  -> 1
    Err(_) -> 0
  end
end

def main
  let p = String.from("/tmp/demo.txt")
  let c = String.from("hello async file\n")
  if write_payload(&p, &c) == 1
    puts "ok"
  end
end
```

Most-used `AsyncFile` methods:

- `AsyncFile.open(path: &String)` — open for reading.
- `AsyncFile.create(path: &String)` — open for writing (truncates if it exists).
- `.read_to_string` — reads the whole file.
- `.write_all(contents: &String)` — writes the whole buffer.

All return `Result[T, IoError]`.

## 5. Async TCP

`std.async_net.AsyncTcpListener` and `AsyncTcpStream` are the network equivalents. Same pattern — bind, accept, connect, read, write all return futures.

```ruxen
use std.async_net.{AsyncTcpListener, AsyncTcpStream}

def server_run -> Int
  let addr  = String.from("127.0.0.1:31729")
  let bound = block_on(AsyncTcpListener.bind(addr))
  match bound
    Ok(listener) -> accept_one(listener)
    Err(_)       -> 0
  end
end

def accept_one(listener: AsyncTcpListener) -> Int
  var l = listener
  match block_on(l.accept)
    Ok(pair) -> echo(pair.0)
    Err(_)   -> 0
  end
end

def echo(stream: AsyncTcpStream) -> Int
  var s = stream
  match block_on(s.read(64))
    Ok(payload) -> finish_echo(s, payload)
    Err(_)      -> 0
  end
end

def finish_echo(stream: AsyncTcpStream, payload: String) -> Int
  var s = stream
  match block_on(s.write(payload))
    Ok(_)  -> 1
    Err(_) -> 0
  end
end
```

`l.accept` returns `Result[(AsyncTcpStream, String), IoError]` — the second tuple field is the peer's address as a string.

## 6. Async stdin

`AsyncStdin.read_line` resolves to `Result[String, IoError]`. On EOF it returns `Ok("")` rather than an error — useful for "loop until empty input":

```ruxen
use std.async_io.AsyncStdin

def main
  let line = block_on(AsyncStdin.read_line)
  match line
    Ok(s)  -> puts "got: #{s}"
    Err(_) -> puts "read error"
  end
end
```

`read_line` is a reading method — no parens needed.

## 7. Writing your own future

For most work, the built-in futures (file I/O, TCP, sleep) plus `async def` are enough. But sometimes you need to write a future by hand — e.g. a custom event source. Include the `Future` mixin and implement `poll`:

```ruxen
class CountdownFuture
  remaining: Int
  include Future
  type Output = Int

  def init(n: Int)
    self.remaining = n
  end

  def var poll(cx: &var Context) -> Poll[Int]
    if self.remaining == 0
      Poll.Ready(0)
    else
      self.remaining = self.remaining - 1
      Poll.Pending
    end
  end
end
```

Three required pieces:

- `include Future` — opts into the runtime.
- `type Output = T` — what the future eventually produces.
- `def var poll(cx: &var Context) -> Poll[Self.Output]` — one step of the state machine. Return `Poll.Ready(v)` to resolve; `Poll.Pending` to ask to be polled again.

Once written, any caller can `.await` your future from inside `async def`, or pass it to `block_on(...)`.

## 8. Common mistakes

- **Calling an `async def` without `.await` or `block_on`.** You get back the future, not the value. Nothing runs. If you see "unused future" warnings or a confusing type mismatch, that's the cause.
- **Using `Thread.sleep` inside async code.** Blocks the runtime thread. Use `Async.sleep` instead.
- **Awaiting from inside a plain `def`.** `.await` is only legal inside `async def`. From a sync function, use `block_on(...)`.
- **Holding a `&var` borrow across an `.await`.** When the function suspends, the borrow is still live but the thread has moved on — a recipe for trouble. Take the value out first, await, then re-acquire.

> **Try it:** chain three `Async.sleep` calls in one `async def` (50 ms each). Time the whole thing using `Instant.now` from `std.time`. Why is it ~150 ms (sequential) instead of ~50 ms (parallel)? (Hint: futures run sequentially unless explicitly composed in parallel.)

---

## Recap

- A **future** represents a value that will arrive eventually.
- `async def` declares a function whose body returns a future.
- `.await` (inside `async def`) suspends until a future resolves.
- `block_on(future)` (from sync code) drives a future to completion.
- `Async.sleep`, `AsyncFile`, `AsyncTcpListener`, `AsyncStdin` are the built-in awaitable types.
- For custom futures, `include Future`, declare `type Output`, and implement `poll`.

**Next:** [Chapter 25 — Function Overloads and Optional Arguments](25-overloads-and-defaults.md).
