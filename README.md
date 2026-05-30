# Ruxen

A compiled, statically-typed programming language that fuses Ruby's expressiveness with Rust's ownership-based memory safety. No garbage collector. Native binaries. Compile-time safety guarantees.

```ruxen
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

## Why Ruxen?

Ruxen targets developers coming from Ruby, Python, and JavaScript who want predictable performance and compile-time safety without sacrificing the joy of writing code.

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

The first in-tree example is [`examples/01-cli-utility/`](examples/01-cli-utility/README.md), a small CLI that exercises `std.env.args`, `std.fs.read_to_string`, the `?` operator, and `std.process.exit`.

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

```ruxen
let name = "Ruxen"               # immutable
var counter = 0                  # mutable
counter += 1

let a = String.from("hello")
let b = a                        # move — `a` is now invalid
# puts a                         # COMPILE ERROR: use after move
```

### Functions

```ruxen
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

```ruxen
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

```ruxen
match status
  Status.Pending            -> handle_pending()
  Status.InProgress(who)    -> puts "Assigned: #{who}"
  Status.Completed(date)    -> puts "Done: #{date}"
  Status.Cancelled(reason)  -> puts "Cancelled: #{reason}"
end
```

### Error Handling

```ruxen
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

```ruxen
let nums = [1, 2, 3, 4, 5]
let evens = nums.filter { |n| n % 2 == 0 }

nums.each do |n|
  puts n
end

let add = { |a: Int, b: Int| a + b }
let result = add.(3, 4)
```

## Toolchain

Everything ships as one binary, `ruxen`. Subcommands:

| Command | Purpose |
|---------|---------|
| `ruxen new` / `init` / `build` / `run` / `check` / `clean` | Project lifecycle |
| `ruxen add` / `remove` / `update` / `tree` / `verify` | Dependency management |
| `ruxen compile` | Low-level single-file driver (like `rustc`) — `--emit=tokens/ast/hir/mir`, `--release`, `--backend=...` |
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

```
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
