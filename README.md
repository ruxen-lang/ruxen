# Riven

A compiled, statically-typed programming language that fuses Ruby's expressiveness with Rust's ownership-based memory safety. No garbage collector. Native binaries. Compile-time safety guarantees.

```riven
class Greeter
  name: String

  def init(@name: String)
  end

  def greet -> String
    "Hello, #{self.name}!"
  end
end

let greeter = Greeter.new("World")
puts greeter.greet
```

## Why Riven?

Riven targets developers coming from Ruby, Python, and JavaScript who want predictable performance and compile-time safety without sacrificing the joy of writing code.

- **Reads like Ruby** — classes, blocks, `do...end`, string interpolation, implicit returns
- **Compiles like Rust** — ownership, borrowing, no GC, deterministic destruction
- **Types disappear** — aggressive bidirectional inference makes code look dynamically typed while every value has a known type at compile time
- **Safety by default** — references are always valid, no exceptions, no data races. `Option[T]` and `Result[T, E]` with `?` propagation

## Design Principles

| # | Principle | Meaning |
|---|-----------|---------|
| P1 | Implicit Safety, Explicit Danger | Safety is the default. `unsafe`, `unwrap!`, raw pointers require loud syntax |
| P2 | The Compiler Works For You | Aggressive inference, lifetime elision, sensible defaults |
| P3 | One Obvious Path | One closure type, one error handling mechanism, one range syntax |
| P4 | Own What You Use | Every value has one owner. No hidden allocations or reference counting |
| P5 | Clarity At The Boundaries | Terse inside functions, explicit types at public API boundaries |

## Installation

### Install (Linux / macOS)

```bash
curl -fsSL https://raw.githubusercontent.com/sherazp995/riven/master/install.sh | bash
```

The installer downloads the latest prebuilt release, installs the toolchain
into `~/.riven`, and adds `~/.riven/bin` to your `PATH` via your shell rc
file. To pick up the new `PATH` in the current shell:

```bash
source "$HOME/.riven/env"
```

Verify it worked:

```bash
riven --version
rivenc --version
```

Other install options:

```bash
# Pin a specific version
curl -fsSL https://raw.githubusercontent.com/sherazp995/riven/master/install.sh | bash -s -- --version v0.1.0

# Install without modifying shell rc files
curl -fsSL https://raw.githubusercontent.com/sherazp995/riven/master/install.sh | bash -s -- --no-modify-path

# Custom install root
RIVEN_HOME=/opt/riven curl -fsSL https://raw.githubusercontent.com/sherazp995/riven/master/install.sh | bash
```

Uninstall:

```bash
curl -fsSL https://raw.githubusercontent.com/sherazp995/riven/master/uninstall.sh | bash
```

### Build from source

If you want to build from source instead:

```bash
git clone https://github.com/sherazp995/riven.git
cd riven
cargo build --release
# Binaries land in target/release/ (riven, rivenc, riven-lsp, riven-repl)
```

## Quick Start

### Create a Project

```bash
riven new my_app
cd my_app
riven build
riven run
```

### Examples

For complete runnable projects, see [examples/](examples/README.md).

The first in-tree example is [`examples/01-cli-utility/`](examples/01-cli-utility/README.md), a small CLI that exercises `std.env.args`, `std.fs.read_to_string`, the `?` operator, and `std.process.exit`.

### Compile a Single File

```bash
echo 'puts "Hello, Riven!"' > hello.rvn
rivenc hello.rvn
./hello

# Inspect compiler stages
rivenc --emit=tokens hello.rvn
rivenc --emit=ast hello.rvn
rivenc --emit=hir hello.rvn
rivenc --emit=mir hello.rvn

# Release build (LLVM backend, requires LLVM 18)
rivenc --release hello.rvn
```

### REPL

```bash
riven-repl
```

### Format Code

```bash
rivenc fmt .                # format all .rvn files
rivenc fmt --check .        # check without modifying
rivenc fmt --diff file.rvn  # show unified diff
```

## Language at a Glance

### Variables and Ownership

```riven
let name = "Riven"               # immutable
var counter = 0                  # mutable
counter += 1

let a = String.from("hello")
let b = a                        # move — `a` is now invalid
# puts a                         # COMPILE ERROR: use after move
```

### Functions

```riven
# Types inferred where possible
def double(x)
  x * 2
end

# Types required at public API boundaries (P5)
def add(a: Int, b: Int) -> Int
  a + b
end
```

### Classes and Mixins

```riven
class Animal
  name: String
  def init(@name: String) end
  def speak -> String; "..."; end
end

class Dog < Animal
  def speak -> String; "Woof! I'm #{self.name}"; end
end

mixin Display
  def fmt(f: &var Formatter) -> Result[(), FmtError]
end
```

### Pattern Matching

```riven
match status
  Status.Pending            -> handle_pending()
  Status.InProgress(who)    -> puts "Assigned: #{who}"
  Status.Completed(date)    -> puts "Done: #{date}"
  Status.Cancelled(reason)  -> puts "Cancelled: #{reason}"
end
```

### Error Handling

```riven
# No exceptions. Result[T, E] and Option[T] only.
def load_config(path: &str) -> Result[Config, AppError]
  let text = fs.read_to_string(path)?  # ? propagates errors
  let json = Json.parse(&text)?
  Config.from_json(&json)
end

let user = find_user(42)?.name          # ?. safe navigation
let user = find_user(42).unwrap!        # panics on nil
```

### Closures

```riven
let nums = [1, 2, 3, 4, 5]
let evens = nums.filter { |n| n % 2 == 0 }

nums.each do |n|
  puts n
end

let add = { |a: Int, b: Int| a + b }
let result = add.(3, 4)
```

## Toolchain

| Binary | Purpose |
|--------|---------|
| `riven` | Package manager and build tool (`new`, `build`, `run`, `check`, `add`, `remove`, `clean`) |
| `rivenc` | Standalone compiler and formatter |
| `riven-lsp` | Language Server Protocol server for editor integration |
| `riven-repl` | Interactive REPL (Cranelift JIT) |

After installation all four binaries live in `~/.riven/bin/`.

### Editor Support

A **VSCode extension** is included at `editors/vscode/` providing syntax highlighting, hover information, go-to-definition, semantic tokens, and diagnostics via the LSP server.

## Architecture

The compiler follows a six-phase pipeline:

```
Source (.rvn)
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
| `riven-core` | Compiler core — lexer, parser, type system, borrow checker, MIR, codegen, formatter |
| `rivenc` | Standalone compiler binary |
| `riven-cli` | Package manager and build tool |
| `riven-ide` | Error-resilient semantic analysis for editors |
| `riven-lsp` | LSP server (tower-lsp) |

## Implementation Status

| Phase | Status | Notes |
|-------|--------|-------|
| Lexer | Complete | All tokens, string interpolation, raw strings, numeric suffixes |
| Parser | Complete | Full language syntax, error recovery, REPL support |
| Name Resolution | Complete | Two-pass, full scope management, built-in types/mixins |
| Type Inference | Complete | Bidirectional inference, mixin resolution, coercion |
| Borrow Checker | Mostly complete | Move/borrow tracking with NLL; lifetime checking infrastructure present, not fully wired |
| MIR Lowering | Mostly complete | Break/continue and capturing closures have gaps |
| Cranelift Codegen | Mostly complete | Primary backend; drop is wired (user `def var drop` runs, heap-owning locals freed at scope exit per type) |
| LLVM Codegen | Experimental | Feature-gated; less complete than Cranelift; no DWARF debug info yet — gdb/lldb show no source-line mapping for `--backend=llvm` builds |
| C Runtime | Mostly complete | String, Array, I/O, Option/Result operations; Map/Set stubs |
| Formatter | Complete | AST-based, zero-config, comment preservation, `fmt: off` support |
| Package Manager | Complete | Project scaffolding, dependency resolution, lock files |
| LSP / IDE | Phase 1 MVP | Hover, goto-def, diagnostics, semantic tokens (single-file) |
| VSCode Extension | Functional | Syntax highlighting + LSP client |

## Documentation

- [Tutorial](docs/tutorial/) — learn Riven step by step
  - [Getting Started](docs/tutorial/01-getting-started.md)
  - [Variables and Types](docs/tutorial/02-variables-and-types.md)
  - [Functions](docs/tutorial/03-functions.md)
  - [Ownership and Borrowing](docs/tutorial/04-ownership-and-borrowing.md)
  - [Classes and Structs](docs/tutorial/06-classes-and-structs.md)
  - [Pattern Matching](docs/tutorial/07-enums-and-pattern-matching.md)
  - [Error Handling](docs/tutorial/11-error-handling.md)
  - [FFI](docs/tutorial/14-ffi.md)

## License

Riven is dual-licensed under either of:

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT License](LICENSE-MIT)

at your option. This is the same licensing scheme used by the Rust project.

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in Riven by you, as defined in the Apache-2.0 license, shall be
dual-licensed as above, without any additional terms or conditions.
