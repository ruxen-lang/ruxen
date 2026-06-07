# I/O and CLI Tools

Most useful programs talk to the world — read files, parse command-line arguments, print output, return an exit code. This chapter is the working guide to writing CLI tools in Ruxen: stdout / stderr, `args()`, file read / write, buffered I/O for line-oriented work, stdin, error reporting, and the `ruxen explain` tool for debugging compiler messages. By the end you'll have a complete file-to-uppercase CLI as a reference template.

---

## 1. A complete first CLI

Save as `shout.rx`:

```ruxen
use std.io.puts

def main
  puts "hello on stdout"
end
```

Run it:

```bash
ruxen run shout.rx
```

Output:

```
hello on stdout
```

`puts` writes a string plus a newline to stdout. That's the smallest possible CLI — we'll grow it through the rest of this chapter.

## 2. `puts`, `eputs`, and `exit`

Three functions you'll use in nearly every CLI:

```ruxen
use std.io.{puts, eputs}
use std.process.exit

def main
  puts "hello on stdout"
  eputs "hello on stderr"
  exit(0)
end
```

- `puts(msg: &str)` — stdout, trailing newline.
- `eputs(msg: &str)` — stderr, trailing newline.
- `exit(code: Int) -> !` — never returns. The OS sees `code mod 256`.

You usually don't need `exit(0)` — falling off the end of `main` exits with status 0. Use `exit(n)` for non-zero codes from inside helpers where `return` won't propagate that far.

## 3. Reading command-line arguments

```ruxen
use std.env.args
use std.process.exit
use std.io.{puts, eputs}

def main
  let argv = args()
  if argv.size < 2
    eputs "usage: tool <input>"
    exit(1)
  end
  let path = argv.get(1).expect!("path")
  puts "processing #{path}"
end
```

`args() -> Array[String]` always has at least element 0 (the program name). Real flag parsing — `--verbose`, subcommands, `KEY=val` env-style overrides — is your responsibility today; v1 doesn't ship a built-in clap-style parser.

## 4. Reading files

The one-shot read:

```ruxen
use std.fs.read_to_string

def main
  match read_to_string("/etc/hostname")
    Ok(text) -> puts text
    Err(_)   -> puts "read failed"
  end
end
```

If you need more control — incremental reads, seeks, metadata — use the `File` API:

```ruxen
use std.io.File

def show(f: File)
  match f.read_to_string()
    Ok(s)  -> puts s
    Err(_) -> puts "read failed"
  end
end

def main
  match File.open("/etc/hostname")
    Ok(f)  -> show(f)
    Err(_) -> puts "open failed"
  end
end
```

Notice the pattern: each `match` arm calls a single helper, never inlines a multi-line body. This keeps the arms short and easy to read.

### `OpenOptions` — read / write / append / create flags

```ruxen
use std.io.{File, OpenOptions}

def write_first_line(f: File)
  let _ = f.write_str("hello\n")
end

def main
  let opts = OpenOptions.new().read(true).write(true).create(true)
  match File.open_options("/tmp/log.txt", opts)
    Ok(f)  -> write_first_line(f)
    Err(_) -> puts "open failed"
  end
end
```

### Seeking

```ruxen
use std.io.{File, SeekFrom}

def tail(f: File)
  match f.read_to_string()
    Ok(rest) -> puts "rest=#{rest}"
    Err(_)   -> puts "read failed"
  end
end

def after_seek(f: File)
  match f.seek(SeekFrom.Start(offset: 3))
    Ok(_)  -> tail(f)
    Err(_) -> puts "seek failed"
  end
end
```

`SeekFrom` has variants `Start(offset)`, `Current(offset)`, and `End(offset)`. `seek` returns the resulting absolute position.

### Metadata

```ruxen
def show_meta(f: File)
  match f.metadata()
    Ok(m)  -> puts "size=#{m.size} file=#{m.is_file}"
    Err(_) -> puts "meta failed"
  end
end
```

## 5. Writing files

One-shot write:

```ruxen
use std.fs.write

def main
  write("/tmp/out.txt", "hello\n").expect!("write")
end
```

Incremental writes via `File.create` + `write_str`:

```ruxen
use std.io.File

def body(f: File)
  let _ = f.write_str("hello\n")
  let _ = f.write_str("world\n")
end

def main
  match File.create("/tmp/out.txt")
    Ok(f)  -> body(f)
    Err(_) -> puts "create failed"
  end
end
```

## 6. Buffered I/O

For line-by-line reading or batched writes, wrap a `File` in `BufReader` / `BufWriter`:

```ruxen
use std.io.File
use std.bufio.{BufReader, BufWriter}

def show(line: String) -> Bool
  puts "line=#{line}"
  false
end

def handle(opt: String?) -> Bool
  match opt
    Some(line) -> show(line)
    nil       -> true            # EOF
  end
end

def drain(br: BufReader.File)
  var done = false
  while !done
    match br.read_line()
      Ok(opt) -> done = handle(opt)
      Err(_)  -> done = true
    end
  end
end

def main
  match File.open("/tmp/in.txt")
    Ok(f)  -> drain(BufReader.File.new(f))
    Err(_) -> puts "open failed"
  end
end
```

`BufReader.read_line` returns `Result[Option[String], IoError]` — `Ok(nil)` means clean EOF.

`BufWriter` is the writing equivalent:

```ruxen
let bw = BufWriter.File.new(f)
let _ = bw.write_all(&bytes)
let _ = bw.flush()
```

**Always `flush()` before dropping a `BufWriter`** — drop alone does not guarantee the buffered tail is written.

## 7. Reading stdin

```ruxen
use std.io.Stdin

def show(text: String)
  puts "> #{text}"
end

def report
  puts "read error"
end

def handle(line: Result[String, IoError])
  match line
    Ok(text) -> show(text)
    Err(_)   -> report
  end
end

def main
  # `Stdin` is a handle type with no constructor — get one from the free
  # function `stdin()` (likewise `stdout()` / `stderr()`).
  let stdin = stdin()
  for line in stdin.lines()
    handle(line)
  end
end
```

`stdin.lines()` returns `Array[Result[String, IoError]]` — fully buffered, not streaming. For a "read forever" streaming loop, wrap raw stdin in `BufReader`.

## 8. Panics vs. graceful errors

A `panic!(msg)` aborts the process with the message on stderr and a non-zero exit code:

```ruxen
def parse_or_die(s: &str) -> Int
  match s.parse_int
    Ok(n)  -> n
    Err(_) -> panic!("bad number: #{s}")
  end
end
```

Use `panic!` for *invariants and never-supposed-to-happen* conditions — bugs in your code, not bad input from the user.

For expected user-facing errors, the conventional shape is "print to stderr, exit non-zero." Pull the error path into a helper so each match arm stays a single expression:

```ruxen
def die(msg: String)
  eputs "error: #{msg}"
  exit(1)
end

def main
  match parse(s)
    Ok(v)  -> use_it(v)
    Err(e) -> die(e.message)
  end
end
```

## 9. `ruxen explain` — looking up compiler errors

Every compiler diagnostic carries a code (e.g. `E0702`). Look one up with:

```bash
ruxen explain E0702
ruxen explain E1117
```

The output expands on the inline message with examples and the recommended fix. Worth adding to your debugging workflow — particularly for the const-generic (`E07xx`) and mixin (`E11xx`) families where the one-line error is intentionally brief.

## 10. A full skeleton: file-to-uppercase CLI

```ruxen
use std.env.args
use std.fs.read_to_string
use std.io.{puts, eputs}
use std.process.exit

def emit(path: &String) -> Result[nil, IoError]
  let text = read_to_string(path)?
  puts text.to_upper
  Ok(nil)
end

def die(msg: String)
  eputs "shout: #{msg}"
  exit(1)
end

def main
  let argv = args()
  if argv.size < 2
    eputs "usage: shout <file>"
    exit(1)
  end
  let path = argv.get(1).expect!("path")
  match emit(path)
    Ok(_)  -> nil
    Err(e) -> die(e.message)
  end
end
```

Build and run:

```bash
ruxen build
./target/debug/shout README.md
```

This is the canonical CLI shape:

- Short `main` — argv parsing, one call into a helper, one error funnel.
- All real work lives in `Result`-returning helpers.
- One `die` helper isolates the "print and exit" pattern from the match arms.

## 11. Common mistakes

- **Calling `exit(0)` at the end of `main`.** Don't. Falling off the end exits cleanly; `exit` short-circuits drops and can mask resource leaks.
- **Forgetting to `flush()` a `BufWriter`.** The last batch of writes may never reach disk. Always `flush` before dropping.
- **Using `panic!` for user errors.** Reserve `panic!` for bugs. For bad input or missing files, print to stderr and `exit(1)`.
- **Inlining multi-statement match arms.** Pull each arm body into a helper. Single-expression arms read far better.
- **Treating `Stdin.lines()` as streaming.** It buffers the whole input. For a "process line, then read another" loop, wrap stdin in `BufReader`.

> **Try it:** extend the `shout` example to take multiple file arguments (`argv[1..]`) and concatenate them all to uppercase output.

---

## Recap

- `puts` / `eputs` / `exit(n)` plus `args()` cover most CLI plumbing.
- `read_to_string` / `write` for one-shot file I/O; `File` + `OpenOptions` for fine control.
- `BufReader` / `BufWriter` for line-oriented or batched I/O — remember to `flush`.
- `panic!` for bugs; "stderr + `exit(1)`" for user-facing errors.
- `ruxen explain CODE` looks up the full text of any compiler error.

**Next:** [Chapter 32 — Idiomatic Patterns](32-idiomatic-patterns.md).
