# Standard Library Tour

When you sit down to write a real program, you'll need to read files, parse arguments, print to stderr, time things, and maybe make a network connection. Ruxen ships a small focused standard library that covers these everyday needs without trying to be everything. This chapter is a quick "what's in the box" tour — one section per module, each with the minimum example you need to be productive. Skim it now; come back when you need something specific.

The stdlib is moving to Ruby's class and method names (`File.read`, `ARGV`, `ENV`, `Time.now`, `system`) and to Ruby exceptions instead of returned `Result` values. Both are [planned](../../TODO.md#t-stdlib-shapes) and [planned](../../TODO.md#t-exceptions). So each section below shows the Ruby form first, then the form that works today. The works-today forms return `Result` and take it apart with `case/in` and `Ok(...)` / `Err(...)` patterns, which are [deprecated] and will be replaced by `begin/rescue`.

Every stdlib class used here (`FS`, `Env`, `Path`, `Instant`, `Time`, `Command`, `TcpStream`) is available without a `require`. The old `use std.fs.{...}` imports are [deprecated], replace them with the qualified names (`FS.read_to_string`).

---

## 1. A complete first example

Here's a tiny program that reads stdin and echoes each line back to stdout. Save as `echo.rx`. The Ruby form, with `STDIN.each_line` [planned](../../TODO.md#t-stdlib-shapes):

```ruby
STDIN.each_line do |text|
  puts "> #{text}"
end
```

Works today [deprecated] (`Result` and the `Ok` / `Err` patterns, replaced by the form above):

```ruby
for line in stdin().lines
  case line
  in Ok(text) then puts "> #{text}"
  in Err(_) then nil
  end
end
```

Run it:

```sh
echo -e "hello\nworld" | ruxen run echo.rx
```

Output:

```text
> hello
> world
```

That tiny program touches two pieces of the standard library: `stdin()` for reading and `puts` for writing, plus a `Result` per line for read errors. The rest of the chapter walks through each module in the same shape — short example, then the few methods you'll reach for.

---

## 2. `std.io` — stdin, stdout, stderr

Example [implemented]:

```ruby
print "hello "
puts "world"
warn "an oops on stderr"
```

| Call | What it does | Status |
|---|---|---|
| `print s` | Writes `s` to stdout, no newline | [implemented] |
| `puts s` | Writes `s` plus `\n` | [implemented] |
| `warn s` | Writes `s` plus `\n` to stderr | [implemented] |
| `stdin().lines` | Returns every line, fully buffered | [implemented]; Ruby's `STDIN.each_line` and `gets` [planned](../../TODO.md#t-stdlib-shapes) |
| `stdout().println(s)`, `stderr().eprintln(s)`, `eputs s` | Handle and Rust-named writers | [deprecated], replace with `puts` and `warn` |

`stdin().lines` gives you back an `Array[Result[String, IoError]]` — fully buffered, not streaming. For long-running input use a buffered reader (covered in [Chapter 31](31-io-and-cli.md)). Calling a zero-argument function such as `stdin` needs the parentheses today; the bare `stdin.lines` is [planned](../../TODO.md#t-f45).

`IoError` has cases like `NotFound`, `PermissionDenied`, `Interrupted`, and `Other(message)`. Call `.message` on one to get the user-facing string [implemented].

---

## 3. `std.fs` — files and directories

The Ruby form [planned](../../TODO.md#t-stdlib-shapes):

```ruby
File.write("hello.txt", "hi from Ruxen!")
puts "got: #{File.read("hello.txt")}"
File.delete("hello.txt") if File.exist?("hello.txt")
```

Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by the form above):

```ruby
case FS.write("hello.txt", "hi from Ruxen!")
in Ok(_) then puts "wrote"
in Err(_) then puts "write failed"
end

case FS.read_to_string("hello.txt")
in Ok(contents) then puts "got: #{contents}"
in Err(_) then puts "read failed"
end

if FS.exists("hello.txt")
  FS.remove_file("hello.txt")
end
```

Everything that touches the filesystem returns `Result[T, IoError]` today — that's how the library says "this might fail". The two predicates `FS.exists` and `FS.is_file` return plain `Bool` (a missing path is `false`, not an error).

Other common entries: `FS.is_file`, `FS.is_dir`, `FS.read_dir`, `FS.create_dir`, `FS.remove_dir_all` [implemented].

---

## 4. `std.env` — process environment

The Ruby form [planned](../../TODO.md#t-stdlib-shapes):

```ruby
puts "program = #{$PROGRAM_NAME}"
puts "home = #{ENV["HOME"]}"
puts "cwd = #{Dir.pwd}"
```

Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by the form above):

```ruby
av = Env.args
puts "program = #{av[0]}"

case Env.get("HOME")
in Ok(h) then puts "home = #{h}"
in Err(_) then puts "HOME unset"
end

case Env.current_dir
in Ok(cwd) then puts "cwd = #{cwd}"
in Err(_) then puts "cwd unreadable"
end
```

| Call | Returns | Status |
|---|---|---|
| `Env.args` | `Array[String]`; element 0 is the program name | [implemented]; Ruby's `ARGV` [planned](../../TODO.md#t-stdlib-shapes) |
| `Env.get(name)` | `Result[String, IoError]`, `Err` when unset | [deprecated] shape; Ruby's `ENV[name]` [planned](../../TODO.md#t-stdlib-shapes) |
| `Env.vars` | `Hash[String, String]`, a snapshot at call time | [implemented]; Ruby's `ENV.to_h` [planned](../../TODO.md#t-stdlib-shapes) |
| `Env.current_dir` | `Result[String, IoError]` | [deprecated] shape; Ruby's `Dir.pwd` [planned](../../TODO.md#t-stdlib-shapes) |

Read-only — there is no way to set a variable.

---

## 5. `std.process` — exit and child processes

The Ruby form [planned](../../TODO.md#t-stdlib-shapes):

```ruby
exit(1) unless system("/bin/echo", "hello")
```

Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by `system`):

```ruby
case Command.new("/bin/echo").arg("hello").status
in Ok(s) then puts "code #{s.code}"
in Err(_) then exit(1)
end
```

- `exit(code)` [implemented] — never returns. The OS sees `code mod 256`. Write the parentheses: the parenless `exit 1` is [planned](../../TODO.md#t-f45).
- `Command.new(cmd)` — builder for a child process. Chain `.arg`, `.args`, `.env`, `.current_dir` [implemented].
- `.status` — runs the child and returns `Result[ExitStatus, IoError]`. stdout/stderr are inherited from the parent.
- `.output` — same, but captures the child's stdout / stderr.

`exit` is declared `-> Never`. `Never` is the **never type**: this function does not return, ever [implemented].

---

## 6. `std.net` — basic TCP

The Ruby form, `TCPSocket` [planned](../../TODO.md#t-stdlib-shapes):

```ruby
socket = TCPSocket.new("127.0.0.1", 8080)
socket.write("GET / HTTP/1.0\r\n\r\n")
socket.close
```

Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by the form above):

```ruby
case TcpStream.connect("127.0.0.1:8080")
in Ok(stream) then
  req = "GET / HTTP/1.0\r\n\r\n".bytes
  stream.write(req)
  stream.close
in Err(e) then warn "connect failed: #{e.message}"
end
```

`TcpListener` and `TcpStream` are the only types — blocking I/O only, no TLS, no UDP. The handles own their underlying file descriptor and close it on drop, so even if you forget the explicit `.close` you won't leak. For async networking see [Chapter 24](24-async.md).

---

## 7. `std.path` — POSIX path manipulation

The Ruby form, on `File` [planned](../../TODO.md#t-stdlib-shapes):

```ruby
path = File.join("/usr/local", "bin/ruxen.rx")
puts File.dirname(path)
puts File.basename(path)
puts File.extname(path)
```

Works today [implemented]:

```ruby
path = Path.join("/usr/local", "bin/ruxen.rx")
puts path                              # /usr/local/bin/ruxen.rx
puts Path.parent(path)                 # /usr/local/bin
puts Path.file_name(path)              # ruxen.rx
puts Path.extname(path)                # rx
puts "abs=#{Path.is_absolute(path)}"   # abs=true
```

- Forward-slash paths only — no Windows backslash support.
- An absolute second argument to `Path.join` overrides the first.
- A path with no extension (or a dotfile like `.hidden`) returns an empty string from `Path.extname`, not an error.
- The free functions `path_join`, `path_parent`, `path_file_name`, `path_extension` and `path_is_absolute` are [deprecated], replace them with the `Path` methods above.

---

## 8. `std.time` — clocks, instants, durations

The Ruby form [planned](../../TODO.md#t-stdlib-shapes):

```ruby
start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
do_work
puts "took #{Process.clock_gettime(Process::CLOCK_MONOTONIC) - start} s"
puts "now = #{Time.now}"
```

Works today [deprecated] (`Instant` and `Duration`, replaced by the form above):

```ruby
def do_work
  sum = 0
  for i in 0..1000
    sum = sum + i
  end
end

start = Instant.now
do_work()
elapsed = start.elapsed
puts "took #{elapsed.as_millis} ms"

puts "epoch ns = #{Time.unix_ns}"
```

Two surfaces:

- **`Instant` + `Duration`** — for measuring how long something took. `Instant.now` is a point on the monotonic clock; subtract two instants (or call `.elapsed`) to get a `Duration`.
- **`Time.unix_ns`** [implemented] — wall-clock nanoseconds since 1970-01-01. Use this for timestamps.

Don't mix the two — `Instant` has no fixed origin, and `Time.unix_ns` can move backwards if the system clock is adjusted.

---

## 9. `std.fmt` — formatting and interpolation

This is where the format-spec machinery lives. Ruby's `to_s` and `inspect` [implemented] for built-in types replace the `Display` and `Debug` mixins, which are [deprecated]. The full walkthrough is in [Chapter 17](17-string-formatting-and-interpolation.md).

---

## 10. Collections

`Array`, `Hash`, and `Set` are documented in [Chapter 13](13-collections.md). The patterns you'll use most, works today [deprecated] (a typed `let` inside a `def`, replaced by a typed local `counts: Hash[String, Int] = {}` once typed locals [planned](../../TODO.md#t-typed-local) and `{}` [planned](../../TODO.md#t-empty-hash) land):

```ruby
def count_words(text: String) -> Hash[String, Int]
  let counts: Hash[String, Int] = Hash.new
  for word in text.split(" ")
    if counts.key?(word)
      counts[word] = counts[word] + 1
    else
      counts[word] = 1
    end
  end
  counts
end

def show_uniques
  let unique: Set[Int] = Set.new
  unique.insert(1)
  unique.insert(1)
  unique.insert(2)
  puts unique.size               # 2
end

puts count_words("a b a")["a"]   # 2
show_uniques()
doubled = [1, 2, 3].map { |x| x * 2 }
```

Ruby's `Set#add` and `Hash.new(0)` with a default value are [planned](../../TODO.md#t-stdlib-shapes).

---

## Common mistakes

- **Ignoring the `Result` on filesystem calls.** Almost every `FS` and `std.io` call returns `Result[T, IoError]` today. If you unconditionally `.expect!` ([deprecated]), your program aborts on the first missing file. Match with `case/in` instead.
- **Reading `stdin().lines` for a streaming program.** That returns the whole input at once. For a server-style "read forever" loop, use `BufReader` ([Chapter 31](31-io-and-cli.md)).
- **Treating `Instant` like a timestamp.** It isn't — two different runs of your program get different `Instant` origins. For "when did this happen" semantics, use `Time.unix_ns`.

> **Try it:** rewrite the echo example from section 1 using `FS.read_to_string` instead of stdin, and pass a filename on the command line via `Env.args`.

---

## Recap

- `std.io` — `puts`, `print` and `warn` [implemented], plus the `stdin()` handle.
- `std.fs` — files and directories through `FS`. Everything returns `Result` today; Ruby's `File.read` and exceptions are [planned](../../TODO.md#t-stdlib-shapes).
- `std.env` — `Env.args`, environment variables, current working directory.
- `std.process` — `exit(n)` and `Command` for running children.
- `std.net` — basic blocking TCP.
- `std.time` — `Instant` for elapsed, `Time.unix_ns` for timestamps.

**Next:** [Chapter 19 — Writing and Running Tests](19-writing-and-running-tests.md).
