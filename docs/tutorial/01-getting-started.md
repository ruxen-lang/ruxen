# Getting Started

Welcome. This chapter gets Ruxen installed on your machine and a working program in your hands in under a minute. Ruxen is a compiled language, so "running" a program means producing a small native executable and then running that executable — no virtual machine, no interpreter at runtime.

## Your first program in 30 seconds

Create a file called `hello.rx`:

```ruxen
def main
  puts "Hello, Ruxen!"
end
```

Compile and run:

```bash
ruxen compile hello.rx
./hello
```

You should see:

```
Hello, Ruxen!
```

That's it — you wrote, compiled, and ran a Ruxen program. The rest of this chapter is reference material you can skim now and come back to later.

## Installation

Ruxen ships as a prebuilt toolchain for Linux and macOS. The installer drops everything under `~/.ruxen` and adds `~/.ruxen/bin` to your `PATH` via your shell rc file.

### One-line install

```bash
curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh | bash
```

Pick up the new `PATH` in the current shell (or open a new terminal):

```bash
source "$HOME/.ruxen/env"
```

Confirm that it worked:

```bash
ruxen --version
```

### What gets installed

```
~/.ruxen/
  bin/
    ruxen          # the toolchain — every feature is a subcommand
  lib/
    runtime.c      # C runtime source (used at link time)
  env              # shell snippet that adds bin/ to PATH
  version          # installed release tag
```

There is one binary: `ruxen`. Everything you need — compile, run, format, REPL, language server — is a subcommand of that one binary.

### Install options

```bash
# Pin a specific release
curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh \
  | bash -s -- --version v0.1.0

# Don't touch shell rc files
curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh \
  | bash -s -- --no-modify-path

# Install somewhere other than ~/.ruxen
RUXEN_HOME=/opt/ruxen curl -fsSL \
  https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh | bash
```

### Upgrading the toolchain

Use **`ruxen upgrade`** to upgrade Ruxen itself:

```bash
ruxen upgrade
```

That fetches and installs the latest release into `~/.ruxen/bin/ruxen`.

If you have a local source checkout of the Ruxen repo, build and reinstall from it with `--from-source`:

```bash
ruxen upgrade --from-source .              # rebuild from the current directory
ruxen upgrade --from-source /path/to/ruxen
```

> **`upgrade` vs. `update`** — `ruxen upgrade` is for the **toolchain** (Ruxen itself). `ruxen update` is for your **project dependencies** (the entries in `Ruxen.toml`). They never overlap.

### Uninstalling

```bash
curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/uninstall.sh | bash
```

Or manually: remove `~/.ruxen` and delete the `. "$HOME/.ruxen/env"` line from your shell rc file.

## Creating a project

A single `.rx` file is great for experimenting. For anything bigger, use the project layout:

```bash
ruxen new my_app
cd my_app
```

This creates:

```
my_app/
  Ruxen.toml       # project manifest (name, version, dependencies)
  src/
    main.rx        # entry point
```

Build and run:

```bash
ruxen build
ruxen run
```

`ruxen run` builds first if needed, then launches the binary. Arguments after `--` are forwarded to your program:

```bash
ruxen run -- --name Alice
```

## Project commands

| Command | What it does |
|---------|--------------|
| `ruxen new <name>` | Create a new project (add `--lib` for a library) |
| `ruxen init` | Initialize a project in the current directory |
| `ruxen build` | Compile the project (incremental; add `--release` for optimized) |
| `ruxen run` | Build and run (arguments after `--` are passed to the program) |
| `ruxen check` | Type-check without producing a binary |
| `ruxen test` | Build and run all tests in the project |
| `ruxen clean` | Remove the `target/` directory |
| `ruxen add <dep>` | Add a dependency (`--version`, `--git`, `--path`, `--dev`) |
| `ruxen remove <dep>` | Remove a dependency |
| `ruxen update [package]` | Refresh project dependencies (all if no name, or just `<package>`) |
| `ruxen upgrade` | Upgrade Ruxen itself (add `--from-source <PATH>` for a local checkout) |
| `ruxen tree` | Show the dependency graph |
| `ruxen verify` | Verify lock file checksums |
| `ruxen explain <code>` | Explain a compiler error code (e.g. `ruxen explain E0001`) |
| `ruxen fmt` | Format all `.rx` files (`--check`, `--diff`, `--stdin`) |
| `ruxen bench <file>` | Run microbenchmarks in a file |
| `ruxen compile <file>` | Compile a single `.rx` file directly |
| `ruxen repl` | Start the interactive REPL |
| `ruxen lsp` | Run the Language Server (for editor integrations) |

## The REPL

A **REPL** is an interactive shell that reads an expression, evaluates it, and prints the result. Use it to try things out without writing a file:

```bash
ruxen repl
```

```
Ruxen 0.1.0 REPL — Type :help for commands
> 1 + 2
=> 3 : Int
> let x = "world"
> "hello #{x}"
=> "hello world" : String
> :type 1.0 + 2.0
Float
> :quit
```

REPL commands: `:help`, `:type <expr>`, `:reset`, `:quit`.

## Useful compile flags

```bash
ruxen compile hello.rx              # fast compile (good for development)
ruxen compile --release hello.rx    # optimized compile (slower, smaller, faster binary)
ruxen compile -o mybin hello.rx     # choose the output filename
ruxen compile --force hello.rx      # ignore incremental cache, rebuild from scratch
ruxen fmt hello.rx                  # format in place
ruxen fmt --check .                 # check formatting without changes
```

## Editor support

Install the VSCode extension from `editors/vscode/` in the Ruxen repo for syntax highlighting, hover info, go-to-definition, and error diagnostics. The extension runs `ruxen lsp` from your `PATH`, so no further configuration is needed after installation.

## Troubleshooting

**`ruxen: command not found` after installing.** Your current shell hasn't picked up the new `PATH`. Either run `source "$HOME/.ruxen/env"` or open a new terminal.

**Installer can't resolve the latest release.** GitHub is rate-limiting unauthenticated requests. Pin a version: `... | bash -s -- --version v0.1.0`.

**Install script downloaded but won't run on macOS.** macOS Gatekeeper may quarantine the binaries. Run: `xattr -dr com.apple.quarantine "$HOME/.ruxen/bin"`.

**Need to reset everything.** Remove `~/.ruxen`, remove `~/.cache/ruxen`, and delete the `. "$HOME/.ruxen/env"` line from your shell rc file.

## Try it

Change `"Hello, Ruxen!"` to `"Hello, " + your name`. Recompile and rerun — you should see your name printed back. Then try:

```ruxen
def main
  let name = "world"
  puts "Hello, #{name}!"
end
```

The `#{...}` syntax interpolates a value into a string — we'll see plenty more of it in the next chapter.

## Recap

- Ruxen is compiled — `ruxen compile foo.rx` produces a native binary named `foo`.
- One binary (`ruxen`) covers every workflow: compile, run, fmt, test, REPL, language server.
- For single files, use `ruxen compile`. For projects, use `ruxen new` + `ruxen run`.
- The REPL is one command away when you want to try something out.

**Next:** [Variables and Types](02-variables-and-types.md) — how to name values and what kinds of values Ruxen has.
