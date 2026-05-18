# Standard Library Tour

> **See also:** every section below cross-links to its formal spec
> under [`docs/specs/stdlib/`](../specs/).  The tutorial gives you
> the 10-minute orientation; the spec is the source of truth.

Riven ships a small focused standard library.  This chapter is a
"what's in the box" tour — one section per module, each with the
minimum to be productive.  For full method surfaces, follow the
spec link at the top of each section.

---

## `std.io` — stdin / stdout / stderr

[Spec](../specs/stdlib/io.spec.md)

```riven
use std.io.{Stdin, Stdout, Stderr}

def main
  let _ = Stdout.new().write_str("hello ")
  Stdout.new().println("world")

  Stderr.new().eprintln("oops, something wrong")

  let stdin = Stdin.new()
  for line in stdin.lines()
    match line
      Ok(text) -> Stdout.new().println("> #{text}")
      Err(_)   -> Stderr.new().eprintln("read error")
    end
  end
end
```

Cheat sheet:

- `Stdout.new()` / `Stderr.new()` are zero-cost handle constructors.
- `println(s)` adds `\n`; `print(s)` doesn't.
- `Stderr.eprintln(s)` is the stderr version (returns `Result[(), IoError]`).
- `Stdin.lines()` returns `Array[Result[String, IoError]]` — fully
  buffered (v1 simplification; not a streaming `BufRead`).

`IoError` is a tagged enum with `NotFound`, `PermissionDenied`,
`Interrupted`, `UnexpectedEof`, and `Other(String)`.  Call
`.message()` for the user-facing string.

---

## `std.fs` — files & directories

[Spec](../specs/stdlib/fs.spec.md)

```riven
use std.fs.{read_to_string, write, exists, is_file, is_dir, read_dir,
            create_dir, remove_file}

def main
  match write("hello.txt", "hi from Riven!")
    Ok(_)  -> puts "wrote"
    Err(_) -> puts "write failed"
  end

  match read_to_string("hello.txt")
    Ok(contents) -> puts "got: #{contents}"
    Err(_)       -> puts "read failed"
  end

  if exists("hello.txt") && is_file("hello.txt")
    puts "still there"
  end

  let _ = remove_file("hello.txt")
end
```

Everything returns `Result[T, IoError]`.  Predicates (`exists`,
`is_file`, `is_dir`) return `Bool` (a missing path returns `false`,
not an error).

`read_dir(path)` returns `Result[Array[String], IoError]` — order is
unspecified; sort before comparing.

---

## `std.env` — process environment

[Spec](../specs/stdlib/env.spec.md)

```riven
use std.env.{args, var, vars, current_dir}

def main
  let av = args()
  puts "argv[0] = #{av[0]}"

  match var("HOME")
    Ok(h) -> puts "home = #{h}"
    Err(_) -> puts "$HOME unset"
  end

  match current_dir()
    Ok(cwd) -> puts "cwd = #{cwd}"
    Err(_)  -> puts "cwd unreadable"
  end
end
```

- `args() -> Array[String]` — element 0 is the program name; never empty.
- `var(name)` returns `Result[String, VarError]` (`Err(NotPresent)`
  when unset).
- `vars()` returns a `Map[String, String]` snapshot.
- `current_dir()` returns `Result[String, IoError]`.

Read-only in v1: no `set_var` / `remove_var`.

---

## `std.process` — exit & spawn

[Spec](../specs/stdlib/process.spec.md)

```riven
use std.process.{exit, Command}

def main
  match Command.new("/bin/echo").arg("hello").status
    Ok(s)  -> if s.code != 0 then exit(s.code) end
    Err(e) -> eputs "spawn failed: #{e.message}"; exit(1)
  end
end
```

- `exit(code: Int) -> !` — never returns; OS exit code is `code` mod 256.
- `Command.new(cmd: &str) -> Command` — builder; chain `.arg`,
  `.args`, `.env`, `.current_dir` to configure the child.
- `.status -> Result[ExitStatus, IoError]` — fork+execvp, inherit
  stdio, wait, return the exit status (`128+signal` on signal
  termination).  Missing-binary returns `Err(IoError.NotFound(_))`.
- `.output -> Result[Output, IoError]` — same, but stdout / stderr
  are captured into `Array[UInt8]` instead of inherited.

The flat `process_run(cmd, args) -> Int` shortcut shipped in earlier
previews was removed once `Command.{status, output}` covered every
use case — see `docs/specs/stdlib/process.spec.md`.

---

## `std.net` — minimal TCP

[Spec](../specs/stdlib/net.spec.md)

```riven
use std.net.{TcpStream}

def main
  match TcpStream.connect(&"127.0.0.1:8080")
    Ok(stream) ->
      let req = "GET / HTTP/1.0\r\n\r\n".bytes()
      let _ = stream.write(&req)
      stream.close()
    Err(e) -> eputs "connect failed: #{e.message()}"
  end
end
```

The class wrappers (`TcpListener`, `TcpStream`) own the underlying
fd and `close` it automatically on drop. Both return
`Result[_, IoError]` from every operation. Blocking I/O only — no
TLS, UDP, or async (those are post-v1). The raw fd-based free
functions (`tcp_connect`, `tcp_listen`, …) that existed in earlier
previews have been removed from the user-facing surface.

---

## `std.path` — POSIX path manipulation

[Spec](../specs/stdlib/path.spec.md)

```riven
use std.path.{path_join, path_parent, path_file_name,
              path_extension, path_is_absolute}

def main
  let p = path_join(&"/usr/local", &"bin/riven.rvn")
  puts p                               # /usr/local/bin/riven.rvn
  puts path_parent(&p)                 # /usr/local/bin
  puts path_file_name(&p)              # riven.rvn
  puts path_extension(&p)              # rvn
  puts "abs=#{path_is_absolute(&p)}"   # abs=true
end
```

- POSIX forward-slash only; no Windows backslashes.
- An absolute second argument to `path_join` overrides the first
  (matches Rust).
- A path without an extension (or a dotfile like `.hidden`) yields
  an empty string from `path_extension` — not a `Result.Err`.

---

## `std.time` — clocks, `Instant`, `Duration`

[Spec](../specs/stdlib/time.spec.md)

```riven
use std.time.{Instant, Duration, unix_ns}

def main
  let start = Instant.now
  do_work()
  let elapsed = start.elapsed       # Duration
  puts "took #{elapsed.as_millis} ms"

  let wall = unix_ns()
  puts "epoch+#{wall}"
end
```

Two surfaces:

- **`Instant` + `Duration`** — typed monotonic-clock wrappers.
  `Instant.now` returns an opaque point on the monotonic clock;
  `instant.elapsed` and `later.duration_since(earlier)` return a
  `Duration`.  Use for measuring elapsed time.
- **`unix_ns() -> Int`** — wall-clock nanoseconds since
  1970-01-01T00:00:00Z.  Use for timestamps.

Don't confuse the two: `Instant` is not a wall-clock value (its origin
is unspecified), and `unix_ns()` can jump backwards under NTP
correction.

The flat `now_ns() -> Int` shortcut shipped in earlier previews was
removed once `Instant.now` covered every use case — see
`docs/specs/stdlib/time.spec.md`.

---

## `std.fmt` — formatting & interpolation

[Spec](../specs/stdlib/fmt.spec.md) ·
[Tutorial chapter 17](17-string-formatting-and-interpolation.md)

`std.fmt` is the only stdlib module with its own dedicated tutorial
chapter — see chapter 17 for the full walk-through of `Display`,
`Debug`, format specs (`:>10`, `:.2`, `:*<5`), and writing
classes that `include Display`.

---

## Collections

[Map spec](../specs/stdlib/map.spec.md) ·
[Set spec](../specs/stdlib/set.spec.md) ·
[Array spec](../specs/stdlib/array.spec.md) ·
[Iterator spec](../specs/stdlib/iterator.spec.md)

Collections are introduced in [Chapter 13 — Collections](13-collections.md).
The specs above cover the v1 method surface; the most common
patterns are:

```riven
var counts: Map[String, Int] = Map.new
for word in text.split(" ")
  counts.entry(word).or_insert(0)
end

var unique: Set[Int] = Set.new
unique.insert(1); unique.insert(1); unique.insert(2)
# unique now has 2 elements

let v: Array[Int] = Array.new
let doubled: Array[Int] = v.iter.map(|x| x * 2).collect[Array[Int]]()
```

---

## Where it lives in the source tree

| Module       | Riven-side resolution           | C runtime fn prefix         |
|--------------|----------------------------------|------------------------------|
| `std.io`    | `resolve/mod.rs` builtin reg     | `riven_stdin_*` / `riven_stdout_*` / `riven_stderr_*` |
| `std.fs`    | same                             | `riven_fs_*`                 |
| `std.env`   | same                             | `riven_env_*`                |
| `std.process` | same                           | `riven_process_*`            |
| `std.net`   | same                             | `riven_tcp_*` / `riven_net_*`|
| `std.path`  | same                             | `riven_path_*`               |
| `std.time`  | same                             | `riven_time_*`               |
| `std.fmt`   | same + `Formatter` class         | `riven_fmt_formatter_*`      |

Codegen wiring lives in
[`codegen/runtime.rs`](../../crates/riven-core/src/codegen/runtime.rs)
(the symbol allow-list + the MIR-callee → C-symbol map).

---

**Next:** [Chapter 19 — Writing and Running Tests](19-writing-and-running-tests.md)
to learn how to pin your own programs against your own specs.
