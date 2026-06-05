# Standard Library Tour

When you sit down to write a real program, you'll need to read files, parse arguments, print to stderr, time things, and maybe make a network connection. Ruxen ships a small focused standard library that covers these everyday needs without trying to be everything. This chapter is a quick "what's in the box" tour — one section per module, each with the minimum example you need to be productive. Skim it now; come back when you need something specific.

---

## 1. A complete first example

Here's a tiny program that reads stdin and echoes each line back to stdout. Save as `echo.rx`:

```ruxen
use std.io.{Stdin, Stdout}

def main
  let stdin = Stdin.new
  for line in stdin.lines()
    match line
      Ok(text) -> Stdout.new.println("> #{text}")
      Err(_)   -> nil
    end
  end
end
```

Run it:

```bash
echo -e "hello\nworld" | ruxen run echo.rx
```

Output:

```
> hello
> world
```

That tiny program touches three pieces of the standard library: `std.io.Stdin` for reading, `std.io.Stdout` for writing, and `Result` for handling read errors. The rest of the chapter walks through each module in the same shape — short example, then the few methods you'll reach for.

---

## 2. `std.io` — stdin, stdout, stderr

```ruxen
use std.io.{Stdin, Stdout, Stderr}

def main
  let _ = Stdout.new.write_str("hello ")
  Stdout.new.println("world")
  Stderr.new.eprintln("an oops on stderr")
end
```

The handle constructors (`Stdout.new`, `Stderr.new`, `Stdin.new`) are essentially free — feel free to call them at the point of use instead of caching in a variable.

| Call                       | What it does                                |
|----------------------------|---------------------------------------------|
| `Stdout.new.print(s)`      | Writes `s` to stdout, no newline            |
| `Stdout.new.println(s)`    | Writes `s` plus `\n`                        |
| `Stderr.new.eprintln(s)`   | Writes `s` plus `\n` to stderr              |
| `Stdin.new.lines()`        | Returns every line, fully buffered          |

`Stdin.lines()` gives you back an `Array[Result[String, IoError]]` — fully buffered, not streaming. For long-running input use a buffered reader (covered in [Chapter 31](31-io-and-cli.md)).

`IoError` is an enum with variants like `NotFound`, `PermissionDenied`, `Interrupted`, and `Other(String)`. Call `.message()` on one to get the user-facing string.

---

## 3. `std.fs` — files and directories

```ruxen
use std.fs.{read_to_string, write, exists, remove_file}

def main
  match write("hello.txt", "hi from Ruxen!")
    Ok(_)  -> puts "wrote"
    Err(_) -> puts "write failed"
  end

  match read_to_string("hello.txt")
    Ok(contents) -> puts "got: #{contents}"
    Err(_)       -> puts "read failed"
  end

  if exists("hello.txt")
    let _ = remove_file("hello.txt")
  end
end
```

Everything that touches the filesystem returns `Result[T, IoError]` — that's how the library says "this might fail". The two predicates `exists` and `is_file` return plain `Bool` (a missing path is `false`, not an error).

Other common entries: `is_file`, `is_dir`, `read_dir`, `create_dir`, `remove_dir`.

---

## 4. `std.env` — process environment

```ruxen
use std.env.{args, var, current_dir}

def main
  let av = args()
  puts "program = #{av[0]}"

  match var("HOME")
    Ok(h)  -> puts "home = #{h}"
    Err(_) -> puts "HOME unset"
  end

  match current_dir()
    Ok(cwd) -> puts "cwd = #{cwd}"
    Err(_)  -> puts "cwd unreadable"
  end
end
```

| Call            | Returns                       | Notes                       |
|-----------------|-------------------------------|-----------------------------|
| `args()`        | `Array[String]`               | Element 0 is the program name |
| `var(name)`     | `Result[String, VarError]`    | `Err(NotPresent)` when unset  |
| `vars()`        | `Hash[String, String]`        | Snapshot at call time         |
| `current_dir()` | `Result[String, IoError]`     |                             |

Read-only — there is no `set_var`.

---

## 5. `std.process` — exit and child processes

```ruxen
use std.process.{exit, Command}

def main
  match Command.new("/bin/echo").arg("hello").status
    Ok(s)  -> if s.code != 0 then exit(s.code) end
    Err(_) -> exit(1)
  end
end
```

- `exit(code: Int) -> !` — never returns. The OS sees `code mod 256`.
- `Command.new(cmd)` — builder for a child process. Chain `.arg`, `.args`, `.env`, `.current_dir`.
- `.status` — runs the child and returns `Result[ExitStatus, IoError]`. stdout/stderr are inherited from the parent.
- `.output` — same, but captures the child's stdout / stderr into byte arrays.

The `!` in `exit(code: Int) -> !` is the **never type**: this function does not return, ever.

---

## 6. `std.net` — basic TCP

```ruxen
use std.net.TcpStream

def main
  match TcpStream.connect(&"127.0.0.1:8080")
    Ok(stream) -> do
      let req = "GET / HTTP/1.0\r\n\r\n".bytes()
      let _ = stream.write(&req)
      stream.close()
    end
    Err(e) -> eputs "connect failed: #{e.message()}"
  end
end
```

`TcpListener` and `TcpStream` are the only types — blocking I/O only, no TLS, no UDP. The handles own their underlying file descriptor and close it on drop, so even if you forget the explicit `.close()` you won't leak. For async networking see [Chapter 24](24-async.md).

---

## 7. `std.path` — POSIX path manipulation

```ruxen
use std.path.{path_join, path_parent, path_file_name,
              path_extension, path_is_absolute}

def main
  let p = path_join(&"/usr/local", &"bin/ruxen.rx")
  puts p                               # /usr/local/bin/ruxen.rx
  puts path_parent(&p)                 # /usr/local/bin
  puts path_file_name(&p)              # ruxen.rx
  puts path_extension(&p)              # rx
  puts "abs=#{path_is_absolute(&p)}"   # abs=true
end
```

- Forward-slash paths only — no Windows backslash support.
- An absolute second argument to `path_join` overrides the first.
- A path with no extension (or a dotfile like `.hidden`) returns an empty string from `path_extension`, not an error.

---

## 8. `std.time` — clocks, instants, durations

```ruxen
use std.time.{Instant, Duration, unix_ns}

def main
  let start = Instant.now
  do_work()
  let elapsed = start.elapsed
  puts "took #{elapsed.as_millis} ms"

  puts "epoch ns = #{unix_ns()}"
end

def do_work
  var sum = 0
  for i in 0..1000
    sum = sum + i
  end
end
```

Two surfaces:

- **`Instant` + `Duration`** — for measuring how long something took. `Instant.now` is a point on the monotonic clock; subtract two instants (or call `.elapsed`) to get a `Duration`.
- **`unix_ns()`** — wall-clock nanoseconds since 1970-01-01. Use this for timestamps.

Don't mix the two — `Instant` has no fixed origin, and `unix_ns()` can move backwards if the system clock is adjusted.

---

## 9. `std.fmt` — formatting and interpolation

This is where `Display`, `Debug`, and the format-spec machinery live. The full walkthrough is in [Chapter 17](17-string-formatting-and-interpolation.md).

---

## 10. Collections

`Array`, `Hash`, and `Set` are documented in [Chapter 13](13-collections.md). The patterns you'll use most:

```ruxen
var counts: Hash[String, Int] = Hash.new
for word in text.split(" ")
  counts.entry(word).or_insert(0)
end

var unique: Set[Int] = Set.new
unique.insert(1)
unique.insert(1)
unique.insert(2)
# unique now has 2 elements

let v: Array[Int] = Array.new
let doubled: Array[Int] = v.map { |x| x * 2 }
```

---

## Common mistakes

- **Forgetting `Result` on filesystem calls.** Almost every `std.fs` and `std.io` call returns `Result[T, IoError]`. If you unconditionally `.expect!()`, your program will panic on the first missing file. Match instead.
- **Reading `Stdin.lines()` for a streaming program.** That returns the whole input at once. For a server-style "read forever" loop, use `BufReader` ([Chapter 31](31-io-and-cli.md)).
- **Treating `Instant` like a timestamp.** It isn't — two different runs of your program get different `Instant` origins. For "when did this happen" semantics, use `unix_ns()`.

> **Try it:** rewrite the echo example from section 1 using `std.fs.read_to_string` instead of stdin, and pass a filename on the command line via `args()`.

---

## Recap

- `std.io` — stdout, stderr, stdin handles plus `puts` / `eputs`.
- `std.fs` — files and directories. Everything returns `Result`.
- `std.env` — `args()`, environment variables, current working directory.
- `std.process` — `exit(n)` and `Command` for running children.
- `std.net` — basic blocking TCP.
- `std.time` — `Instant` for elapsed, `unix_ns()` for timestamps.

**Next:** [Chapter 19 — Writing and Running Tests](19-writing-and-running-tests.md).
