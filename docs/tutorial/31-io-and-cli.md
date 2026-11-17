# I/O and CLI Tools

Most useful programs talk to the world — read files, parse command-line arguments, print output, return an exit code. This chapter is the working guide to writing CLI tools in Ruxen: stdout / stderr, command-line arguments, file read / write, buffered I/O for line-oriented work, stdin, error reporting, and the `ruxen explain` tool for debugging compiler messages. By the end you'll have a complete file-to-uppercase CLI as a reference template.

The file and argument APIs are moving to Ruby's names (`File.read`, `File.write`, `File.foreach`, `ARGV`) [planned](../../TODO.md#t-stdlib-shapes), and their errors to Ruby exceptions [planned](../../TODO.md#t-exceptions). Today they return a `Result`, taken apart with `case/in` and `Ok(...)` / `Err(...)` patterns, which are [deprecated]. Every class used here (`Env`, `FS`, `File`, `OpenOptions`, `SeekFrom`, `BufReader`, `BufWriter`) is available without a `require`; the old `use std.io.{...}` imports are [deprecated], replace them with the bare names.

---

## 1. A complete first CLI

Save as `shout.rx`. Example [implemented]:

```ruby
puts "hello on stdout"
```

Run it:

```sh
ruxen run shout.rx
```

Output:

```text
hello on stdout
```

`puts` writes a string plus a newline to stdout. That's the smallest possible CLI — we'll grow it through the rest of this chapter.

## 2. `puts`, `warn`, and `exit`

Three methods you'll use in nearly every CLI. Example [implemented]:

```ruby
puts "hello on stdout"
warn "hello on stderr"
exit(0)
```

- `puts msg` [implemented] — stdout, trailing newline.
- `warn msg` [implemented] — stderr, trailing newline. `eputs` is the [deprecated] spelling.
- `exit(code)` [implemented] — never returns (its type is `Never`). The OS sees `code mod 256`. The parenless `exit 1` is [planned](../../TODO.md#t-f45).
- `abort(msg)` [implemented] — prints `msg` to stderr and exits with status 1.

You usually don't need `exit(0)` — falling off the end of the program exits with status 0. Use `exit(n)` for non-zero codes from inside helpers where `return` won't propagate that far.

## 3. Reading command-line arguments

The Ruby form, `ARGV` [planned](../../TODO.md#t-stdlib-shapes):

```ruby
abort("usage: tool <input>") if ARGV.empty?
puts "processing #{ARGV[0]}"
```

Works today [implemented]:

```ruby
argv = Env.args
if argv.size < 2
  warn "usage: tool <input>"
  exit(1)
end
path = argv[1]
puts "processing #{path}"
```

`Env.args` returns an `Array[String]` that always has at least element 0 (the program name); Ruby's `ARGV` will leave the program name out. Real flag parsing — `--verbose`, subcommands, `KEY=val` env-style overrides — is your responsibility today; there's no built-in option parser.

## 4. Reading files

The Ruby form, one-shot [planned](../../TODO.md#t-stdlib-shapes):

```ruby
puts File.read("/etc/hostname")
```

Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by the form above):

```ruby
case FS.read_to_string("/etc/hostname")
in Ok(text) then puts text
in Err(_) then puts "read failed"
end
```

If you need more control — incremental reads, seeks, metadata — use the `File` API. Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by exceptions):

```ruby
def show(f: File)
  fv = f
  case fv.read_to_string
  in Ok(s) then puts s
  in Err(_) then puts "read failed"
  end
end

case File.open("/etc/hostname")
in Ok(f) then show(f)
in Err(_) then puts "open failed"
end
```

Notice the pattern: each `case` arm calls a single helper, never inlines a multi-line body. This keeps the arms short and easy to read. (`fv = f` gives the helper its own changeable binding: reading moves the file's position.)

### `OpenOptions` — read / write / append / create flags

Ruby's mode strings (`File.open(path, "a+")`) are [planned](../../TODO.md#t-stdlib-shapes). Works today [deprecated] (`Result` with `Ok` / `Err` patterns):

```ruby
def write_first_line(f: File)
  fv = f
  fv.write_str("hello\n")
end

opts = OpenOptions.new.read(true).write(true).create(true)
case File.open_options("/tmp/log.txt", opts)
in Ok(f) then write_first_line(f)
in Err(_) then puts "open failed"
end
```

### Seeking

Works today [deprecated] (`Result` with `Ok` / `Err` patterns; Ruby's `f.seek(3)` is [planned](../../TODO.md#t-stdlib-shapes)):

```ruby
def tail(f: &var File)
  case f.read_to_string
  in Ok(rest) then puts "rest=#{rest}"
  in Err(_) then puts "read failed"
  end
end

def after_seek(f: File)
  fv = f
  case fv.seek(SeekFrom.Start(offset: 3))
  in Ok(_) then tail(&var fv)
  in Err(_) then puts "seek failed"
  end
end
```

`SeekFrom` has cases `Start(offset)`, `Current(offset)`, and `End(offset)`. `seek` returns the resulting absolute position.

### Metadata

Works today [deprecated] (`Result` with `Ok` / `Err` patterns; Ruby's `File.size(path)` and `File.file?(path)` are [planned](../../TODO.md#t-stdlib-shapes)):

```ruby
def show_meta(f: &File)
  case f.metadata
  in Ok(m) then puts "size=#{m.size} file=#{m.is_file}"
  in Err(_) then puts "meta failed"
  end
end
```

## 5. Writing files

The Ruby form, one-shot [planned](../../TODO.md#t-stdlib-shapes):

```ruby
File.write("/tmp/out.txt", "hello\n")
```

Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by the form above):

```ruby
case FS.write("/tmp/out.txt", "hello\n")
in Ok(_) then nil
in Err(e) then warn "write failed: #{e.message}"
end
```

Incremental writes via `File.create` + `write_str`. Works today [deprecated] (`Result` with `Ok` / `Err` patterns):

```ruby
def body(f: File)
  fv = f
  fv.write_str("hello\n")
  fv.write_str("world\n")
end

case File.create("/tmp/out.txt")
in Ok(f) then body(f)
in Err(_) then puts "create failed"
end
```

### Socket timeouts

The same `Result` shape covers sockets (see [Chapter 18](18-stdlib-tour.md)). To bound a blocking read or write on a `TcpStream`, use `set_read_timeout_ns(ns)` and `set_write_timeout_ns(ns)` [implemented], which take nanoseconds. A `set_read_timeout(duration)` taking a `&Duration` is [planned](../../TODO.md#t-d24); the `set_read_timeout` method that exists today does not match its C side, so do not call it.

## 6. Buffered I/O

The Ruby form, `File.foreach(path) { |line| ... }` [planned](../../TODO.md#t-stdlib-shapes):

```ruby
File.foreach("/tmp/in.txt") do |line|
  puts "line=#{line}"
end
```

For line-by-line reading or batched writes today, wrap a `File` in `BufReader` / `BufWriter`. Works today [deprecated] (`Result` with `Ok` / `Err` patterns and the `Some` pattern, replaced by the form above):

```ruby
def show(line: String) -> Bool
  puts "line=#{line}"
  false
end

def handle(opt: String?) -> Bool
  case opt
  in Some(line) then show(line)
  in nil then true            # EOF
  end
end

def drain(br: BufReader.File)
  b = br
  done = false
  while !done
    case b.read_line
    in Ok(opt) then done = handle(opt)
    in Err(_) then done = true
    end
  end
end

case File.open("/tmp/in.txt")
in Ok(f) then drain(BufReader.File.new(f))
in Err(_) then puts "open failed"
end
```

`BufReader.File#read_line` returns a `Result` holding a `String?` — `Ok(nil)` means clean EOF. The arm `in Some(line)` is used because a binding arm after `in nil` still binds the optional value today [known issue](../../TODO.md#t-case-in-nil).

`BufWriter` is the writing equivalent. Example [implemented]:

```ruby
def write_buffered(f: &File, bytes: &Array[Int])
  bw = BufWriter.File.new(f)
  bw.write_all(bytes)
  bw.flush
end
```

**Always `flush` before dropping a `BufWriter`** — drop alone does not guarantee the buffered tail is written.

## 7. Reading stdin

The Ruby form, `STDIN.each_line` [planned](../../TODO.md#t-stdlib-shapes):

```ruby
STDIN.each_line { |text| puts "> #{text}" }
```

Works today [deprecated] (`Result` with `Ok` / `Err` patterns). `Stdin` is a handle type with no constructor — get one from the method `stdin()` (likewise `stdout()` / `stderr()`):

```ruby
for line in stdin().lines
  case line
  in Ok(text) then puts "> #{text}"
  in Err(_) then puts "read error"
  end
end
```

`stdin().lines` returns `Array[Result[String, IoError]]` — fully buffered, not streaming. For a "read forever" streaming loop, wrap raw stdin in `BufReader`.

## 8. Aborts vs. graceful errors

`abort(msg)` ends the process with the message on stderr and exit status 1. Works today [deprecated] (`parse_int`'s `Result` with `Ok` / `Err` patterns; Ruby's `Integer(s)` with `rescue` is [planned](../../TODO.md#t-exceptions)):

```ruby
def parse_or_die(s: &String) -> Int
  case s.parse_int
  in Ok(n) then n
  in Err(_) then abort("bad number: #{s}")
  end
end
```

Use `abort` for *invariants and never-supposed-to-happen* conditions — bugs in your code, not bad input from the user. `panic!(msg)` is the [deprecated] spelling of the same thing, replaced by `abort` or by `raise` [planned](../../TODO.md#t-exceptions).

For expected user-facing errors, the conventional shape is "print to stderr, exit non-zero." Pull the error path into a helper so each `case` arm stays a single expression. Works today [deprecated] (`Result` with `Ok` / `Err` patterns):

```ruby
def die(msg: String) -> Never
  warn "error: #{msg}"
  exit(1)
end

case "12".parse_int
in Ok(v) then puts v
in Err(e) then die("not a number")
end
```

## 9. `ruxen explain` — looking up compiler errors

Every compiler diagnostic carries a code (e.g. `E0702`). Look one up with [implemented]:

```sh
ruxen explain E0702
ruxen explain E1117
```

The output expands on the inline message with examples and the recommended fix. Worth adding to your debugging workflow — particularly for the const-generic (`E07xx`) and mixin (`E11xx`) families where the one-line error is intentionally brief.

## 10. A full skeleton: file-to-uppercase CLI

The Ruby form, with `ARGV` and `File.read` [planned](../../TODO.md#t-stdlib-shapes) and `begin/rescue` [planned](../../TODO.md#t-exceptions):

```ruby
abort("usage: shout <file>") if ARGV.empty?

begin
  puts File.read(ARGV[0]).upcase
rescue IOError => e
  abort("shout: #{e.message}")
end
```

Works today [deprecated] (`Result` with `Ok` / `Err` patterns, replaced by `rescue` [planned](../../TODO.md#t-exceptions)):

```ruby
def emit(path: &String) -> Bool
  case FS.read_to_string(path)
  in Ok(text) then
    puts text.upcase
    true
  in Err(e) then
    warn "shout: #{e.message}"
    false
  end
end

argv = Env.args
if argv.size < 2
  warn "usage: shout <file>"
  exit(1)
end
if !emit(argv[1])
  exit(1)
end
```

Build and run:

```sh
ruxen build
./target/debug/shout README.md
```

This is the canonical CLI shape:

- Short top level — argv parsing, one call into a helper, one error funnel.
- All real work lives in helpers that report success or failure.
- The error path prints to stderr and exits non-zero. (A top-level statement modifier such as `exit(1) unless emit(path)` is [planned](../../TODO.md#t-toplevel-stmts); it works inside a method today.) The postfix `?` that earlier drafts used to pass errors up is [deprecated]; exceptions will propagate on their own.

## 11. Common mistakes

- **Calling `exit(0)` at the end of the program.** Don't. Falling off the end exits cleanly; `exit` short-circuits drops and can mask resource leaks.
- **Forgetting to `flush` a `BufWriter`.** The last batch of writes may never reach disk. Always `flush` before dropping.
- **Using `abort` for user errors.** Reserve `abort` for bugs. For bad input or missing files, print to stderr with `warn` and `exit(1)`.
- **Inlining multi-statement `case` arms.** Pull each arm body into a helper. Single-expression arms read far better.
- **Treating `stdin().lines` as streaming.** It buffers the whole input. For a "process line, then read another" loop, wrap stdin in `BufReader`.

> **Try it:** extend the `shout` example to take multiple file arguments (every element of `argv` after the first) and concatenate them all to uppercase output.

---

## Recap

- `puts` / `warn` / `exit(n)` plus `Env.args` cover most CLI plumbing [implemented].
- `FS.read_to_string` / `FS.write` for one-shot file I/O; `File` + `OpenOptions` for fine control. Ruby's `File.read` / `File.write` are [planned](../../TODO.md#t-stdlib-shapes).
- `BufReader` / `BufWriter` for line-oriented or batched I/O — remember to `flush`.
- `abort` for bugs; "stderr + `exit(1)`" for user-facing errors.
- `ruxen explain CODE` looks up the full text of any compiler error.

**Next:** [Chapter 32 — Idiomatic Patterns](32-idiomatic-patterns.md).
