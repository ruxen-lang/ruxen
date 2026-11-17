# Ruxen

A compiled, statically-typed programming language that fuses Ruby's expressiveness with Rust's ownership-based memory safety. No garbage collector. Native binaries. Compile-time safety guarantees.

Example [implemented]. Its constructor is still spelled `def init` [deprecated] until `initialize` lands [planned](TODO.md#t-initialize):

```ruby
class Greeter
  name: String

  def init(@name: String)
  end

  def greet -> String
    "Hello, #{@name}!"
  end
end

greeter = Greeter.new("World")
puts greeter.greet
```

## Why Ruxen?

Ruxen targets developers coming from Ruby, Python, and JavaScript who want predictable performance and compile-time safety without sacrificing the joy of writing code.

- **Reads like Ruby** [implemented]: classes, blocks, `do...end`, string interpolation, implicit returns
- **Owns its memory** [implemented]: ownership, borrowing, no GC, deterministic destruction
- **Types stay out of the way** [implemented]: you annotate parameters and fields; locals and most other types are inferred, and every value has a known type at compile time
- **Safety by default** [implemented]: references are always valid and there are no data races. A value that may be missing is typed `T?` and is either the value or `nil`
- **Ruby exceptions, checked statically** [planned](TODO.md#t-exceptions): `raise` (and a bare `raise` to re-raise inside `rescue`), `begin ... rescue ... else ... ensure ... end`, method-level `rescue`/`ensure`, `retry`, and exception classes (`class NotFound < StandardError`), with no stack unwinding

## Design Principles

| # | Principle | Meaning |
|---|-----------|---------|
| P1 | Implicit Safety, Explicit Danger | Safety is the default. `unsafe` blocks and raw pointers require loud syntax |
| P2 | The Compiler Works For You | Aggressive inference, lifetime elision, sensible defaults |
| P3 | One Obvious Path | One way to say "maybe missing" (`T?`), one error handling mechanism, one range syntax |
| P4 | Own What You Use | Every value has one owner. No hidden allocations or reference counting |
| P5 | Clarity At The Boundaries | Terse inside functions, explicit types at public API boundaries |

## Installation

### Install (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh | bash
```

The installer downloads the latest prebuilt release, installs the toolchain
into `~/.ruxen`, and adds `~/.ruxen/bin` to your `PATH` via your shell rc
file. To pick up the new `PATH` in the current shell:

```bash
source "$HOME/.ruxen/env"
```

Verify it worked:

```bash
ruxen --version
```

Other install options:

```bash
# Pin a specific version
curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh | bash -s -- --version v0.1.0

# Install without modifying shell rc files
curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh | bash -s -- --no-modify-path

# Custom install root
RUXEN_HOME=/opt/ruxen curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh | bash
```

Uninstall:

```bash
curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/uninstall.sh | bash
```

### Build from source

If you want to build from source instead:

```bash
git clone https://github.com/ruxen-lang/ruxen.git
cd ruxen
cargo build --release
# Binary lands in target/release/ruxen — every subcommand
# (build, run, compile, fmt, lsp, repl, …) routes through it.
```

## Quick Start

### Create a Project

```bash
ruxen new my_app
cd my_app
ruxen build
ruxen run
```

### Examples

For complete runnable projects, see [examples/](examples/README.md).

The first in-tree example is [`examples/01-cli-utility/`](examples/01-cli-utility/README.md), a small CLI that reads command-line arguments, prints a file and exits with a status. It is still written in the [deprecated] `use`, `Result` and `?` forms, which Ruby's `ARGV`, `File.read` [planned](TODO.md#t-stdlib-shapes) and exceptions [planned](TODO.md#t-exceptions) replace.

### Compile a Single File

```bash
echo 'puts "Hello, Ruxen!"' > hello.rx
ruxen compile hello.rx
./hello

# Inspect compiler stages
ruxen compile hello.rx --emit=tokens
ruxen compile hello.rx --emit=ast
ruxen compile hello.rx --emit=hir
ruxen compile hello.rx --emit=mir

# Release build (LLVM backend, requires LLVM 18)
ruxen compile hello.rx --release
```

### REPL

```bash
ruxen repl
```

### Format Code

```bash
ruxen fmt .                # format all .rx files
ruxen fmt --check .        # check without modifying
ruxen fmt --diff file.rx   # show unified diff
```

## Language at a Glance

### Variables and Ownership

Example [implemented]:

```ruby
name = "Ruxen"                   # a local is declared by its first assignment
counter = 0
counter += 1                     # locals are always reassignable

a = "hello"                      # a bare literal is an owned String
b = a                            # move: `a` is now invalid
# puts a                         # compile error E1001: use after move
```

### Functions

Example [implemented]:

```ruby
# Parameters are annotated; the return type of an endless method is inferred
def double(x: Int) = x * 2

# Spell out the return type at public API boundaries (P5)
def add(a: Int, b: Int) -> Int
  a + b
end

def greet(name: String)
  puts "Hello, #{name}!"
end

total = add(2, 3)
puts double(total)
greet("Ruxen")
```

### Classes and Mixins

Example [implemented]. The constructor is still spelled `def init` [deprecated] until `initialize` lands [planned](TODO.md#t-initialize):

```ruby
class Animal
  name: String

  def init(@name: String)
  end

  def speak -> String = "..."
end

class Dog < Animal
  def speak -> String = "Woof! I'm #{@name}"
end

puts Dog.new("Rex").speak
```

A mixin is a Ruby `module` that a class includes: a method with no body is one the class must provide, and a full `def` is a default. The Ruby form [planned](TODO.md#t-module-mixins); the bare `name` call inside the module is [planned](TODO.md#t-implicit-self) too:

```ruby
module Describable
  def name -> String

  def describe -> String = "This is #{name}"
end

class Dog
  include Describable

  def name -> String = "Rex"
end

puts Dog.new.describe
```

Works today, with the [deprecated] `mixin` keyword (replaced by `module`), `self.name` for the call and a `def init` constructor:

```ruby
mixin Describable
  def name -> String

  def describe -> String
    "This is #{self.name}"
  end
end

class Dog
  include Describable

  def init
  end

  def name -> String = "Rex"
end

puts Dog.new.describe
```

### Pattern Matching

Example [implemented]:

```ruby
def describe(code: Int) -> String
  case code
  in 200 then "ok"
  in 301 | 302 then "redirect"
  in n if n >= 500 then "server error #{n}"
  in _ then "other"
  end
end

puts describe(302)
puts describe(503)
```

Matching on symbols (`in :active`) is [planned](TODO.md#t-symbols), and so are typed record patterns on `Data` variants (`in Shape::Circle(radius:)`) [planned](TODO.md#t-patterns).

### Error Handling

Errors are Ruby exceptions, checked statically: the compiler infers what each method can raise, and an exception propagates on its own. The Ruby form [planned](TODO.md#t-exceptions); `File.read` is [planned](TODO.md#t-stdlib-shapes) too:

```ruby
def load_config(path: String) -> Config
  text = File.read(path)              # an IOError propagates on its own
  Config.parse(text)
rescue IOError => e
  warn "cannot read #{path}: #{e.message}"
  Config.default
end
```

Until exceptions land, a recoverable error is returned as a `Result`, which is [deprecated] (replaced by `raise` and `rescue`). Absence works today. Example [implemented]:

```ruby
def find_name(id: Int) -> String?
  return nil if id == 0
  "user#{id}"
end

puts find_name(42)&.upcase.nil?     # false: &. calls upcase only on a present value
if find_name(0).nil?
  abort("no user 0")                # prints to STDERR and exits with status 1
end
```

### Closures

Example [implemented]:

```ruby
nums = [1, 2, 3, 4, 5]
evens = nums.select { |n| n % 2 == 0 }
puts evens.size

nums.each do |n|
  puts n
end

add = ->(a: Int, b: Int) { a + b }
puts add.(3, 4)
```

## Toolchain

Everything ships as one binary, `ruxen`. Subcommands:

| Command | Purpose |
|---------|---------|
| `ruxen new` / `init` / `build` / `run` / `check` / `clean` | Project lifecycle |
| `ruxen add` / `remove` / `update` / `tree` / `verify` | Dependency management |
| `ruxen compile` | Low-level single-file driver — `--emit=tokens/ast/hir/mir`, `--release`, `--backend=...` |
| `ruxen fmt` | Format `.rx` files (`--check`, `--diff`) |
| `ruxen test` / `bench` | Test runner / microbenchmarks |
| `ruxen repl` | Interactive REPL (Cranelift JIT) |
| `ruxen lsp` | Language Server Protocol over stdio |
| `ruxen explain <code>` | Explain a compiler error code |
| `ruxen publish` / `upgrade` | Publish a release tag / upgrade the toolchain |

After installation the `ruxen` binary lives in `~/.ruxen/bin/`.

### Editor Support

A **VSCode extension** is included at `editors/vscode/` providing syntax highlighting, hover information, go-to-definition, semantic tokens, and diagnostics via the LSP server.

## Architecture

The compiler follows a six-phase pipeline:

```text
Source (.rx)
  → Lexer         → tokens
  → Parser        → AST (untyped)
  → Resolver      → symbol table + type registry
  → Type Checker  → HIR (typed, with inference resolved)
  → Borrow Check  → ownership/borrowing validation
  → MIR Lowering  → basic blocks + control flow graph
  → Codegen       → native executable
```

Two codegen backends:
- **Cranelift** (default) — fast compilation for development
- **LLVM** (opt-in, `--release`) — optimized output for production, requires LLVM 18

### Crate Structure

| Crate | Role |
|-------|------|
| `ruxen_core` | Compiler core — lexer, parser, type system, borrow checker, MIR, codegen, formatter |
| `ruxen_cli` | Unified `ruxen` binary — every subcommand routes through here |
| `ruxenc` | Compile/fmt/test/bench driver library that `ruxen_cli` invokes |
| `ruxen_repl` | Cranelift JIT REPL library + `libruxenrt` C runtime build |
| `ruxen_lsp` | LSP server (tower-lsp) |
| `ruxen_ide` | Error-resilient semantic analysis for editors |

## Implementation Status

| Phase | Status | Notes |
|-------|--------|-------|
| Lexer | [implemented] | All tokens, string interpolation, numeric suffixes |
| Parser | [implemented] | Full language syntax, error recovery, REPL support |
| Name Resolution | [implemented] | Two-pass, full scope management, built-in types/mixins |
| Type Inference | [implemented] | Bidirectional inference, mixin resolution, coercion |
| Borrow Checker | [implemented], with gaps | Move/borrow tracking with NLL; lifetime checking infrastructure present, not fully wired |
| MIR Lowering | [implemented], with gaps | `next` in `while`/`until`/`for` [planned](TODO.md#t-next-loops); capturing closures have gaps |
| Cranelift Codegen | [implemented], with gaps | Primary backend; drop is wired (user `def var drop` runs, heap-owning locals freed at scope exit per type) |
| LLVM Codegen | [implemented], experimental | Feature-gated; less complete than Cranelift; no DWARF debug info yet: gdb/lldb show no source-line mapping for `--backend=llvm` builds |
| C Runtime | [implemented], with gaps | String, Array, Hash, Set and I/O operations; an untyped `Hash.new` panics on its first insert [known issue](TODO.md#t-hash-new-key) |
| Formatter | [implemented] | AST-based, zero-config, `# fmt: off` support. It never loses a comment: for a comment inside a method chain or an argument list, or a partial `# fmt: off` range, it reports an error instead; supporting both is [planned](TODO.md#t-d21-impl) |
| Package Manager | [implemented] | Project scaffolding, dependency resolution, lock files |
| LSP / IDE | [implemented], phase 1 | Hover, goto-def, diagnostics, semantic tokens (single-file) |
| VSCode Extension | [implemented] | Syntax highlighting + LSP client |

## Documentation

- [Tutorial](docs/tutorial/) — learn Ruxen step by step
  - [Getting Started](docs/tutorial/01-getting-started.md)
  - [Variables and Types](docs/tutorial/02-variables-and-types.md)
  - [Functions](docs/tutorial/03-functions.md)
  - [Ownership and Borrowing](docs/tutorial/04-ownership-and-borrowing.md)
  - [Classes and Structs](docs/tutorial/06-classes-and-structs.md)
  - [Pattern Matching](docs/tutorial/07-enums-and-pattern-matching.md)
  - [Error Handling](docs/tutorial/11-error-handling.md)
  - [FFI](docs/tutorial/14-ffi.md)

## License

Ruxen is dual-licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option. This is the same licensing scheme used by the Rust project.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in Ruxen by you, as defined in the Apache-2.0 license, shall be
dual-licensed as above, without any additional terms or conditions.
