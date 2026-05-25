# Getting Started

## Installation

Ruxen ships as a prebuilt toolchain for Linux and macOS. The installer drops
everything under `~/.ruxen` and adds `~/.ruxen/bin` to your `PATH` via your
shell rc file.

### One-line install

```bash
curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh | bash
```

Pick up the new `PATH` in the current shell:

```bash
source "$HOME/.ruxen/env"
```

Or open a new terminal. Confirm that it worked:

```bash
ruxen --version
ruxenc --version
```

### What gets installed

```
~/.ruxen/
  bin/
    ruxen          # package manager & build tool
    ruxenc         # standalone compiler (and formatter)
    ruxen-lsp      # LSP server for editors
    ruxen-repl     # interactive REPL
  lib/
    runtime.c      # C runtime source (used at link time)
  env              # shell snippet that adds bin/ to PATH
  version          # installed release tag
```

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

### Uninstalling

```bash
curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/uninstall.sh | bash
```

Or manually: remove `~/.ruxen` and delete the `. "$HOME/.ruxen/env"` line from
your shell rc file.

### Upgrading

Re-run the installer. It overwrites the binaries in `~/.ruxen/bin` and bumps
`~/.ruxen/version`.

## Your First Program

Create a file called `hello.rx`:

```ruxen
puts "Hello, Ruxen!"
```

Compile and run:

```bash
ruxenc hello.rx
./hello
```

You should see:

```
Hello, Ruxen!
```

## Creating a Project

For anything beyond a single file, use the package manager:

```bash
ruxen new my_app
cd my_app
```

This creates:

```
my_app/
  Ruxen.toml        # project manifest
  src/
    main.rx        # entry point
```

Build and run with:

```bash
ruxen build
ruxen run
```

## Project Commands

| Command | What it does |
|---------|--------------|
| `ruxen new <name>` | Create a new project |
| `ruxen init` | Initialize a project in the current directory |
| `ruxen build` | Compile the project (incremental) |
| `ruxen run` | Build and run |
| `ruxen check` | Type-check without producing a binary |
| `ruxen clean` | Remove build artifacts |
| `ruxen clean --global` | Clear the global cache at `~/.cache/ruxen/` |
| `ruxen add <dep>` | Add a dependency |
| `ruxen remove <dep>` | Remove a dependency |
| `ruxen update` | Refresh the lockfile |
| `ruxen tree` | Show the dependency graph |

## The REPL

Fire up an interactive session:

```bash
ruxen-repl
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

## Compiler Flags

```bash
ruxenc hello.rx              # compile with Cranelift (fast)
ruxenc --release hello.rx    # compile with LLVM (optimized, requires LLVM 18)
ruxenc -o mybin hello.rx     # custom output name
ruxenc --emit=ast hello.rx   # inspect AST (also: tokens, hir, mir)
ruxenc --force hello.rx      # ignore incremental cache, rebuild from scratch
ruxenc --verbose hello.rx    # log cache hits/misses
ruxenc fmt hello.rx          # format in place
ruxenc fmt --check .          # check formatting without changes
ruxenc fmt --diff file.rx    # show a unified diff
```

## Editor Support

Install the VSCode extension from `editors/vscode/` in the Ruxen repo for
syntax highlighting, hover info, go-to-definition, and error diagnostics. The
extension launches `ruxen-lsp` from your `PATH`, so no further configuration
is needed after installation.

## Troubleshooting

**`ruxen: command not found` after installing.** Your current shell hasn't
picked up the new `PATH`. Either run `source "$HOME/.ruxen/env"` or open a new
terminal.

**Installer can't resolve the latest release.** GitHub is rate-limiting
unauthenticated requests. Pin a version: `... | bash -s -- --version v0.1.0`.

**Install script downloaded but won't run on macOS.** macOS Gatekeeper may
quarantine the binaries. Run:
`xattr -dr com.apple.quarantine "$HOME/.ruxen/bin"`.

**Need to reset everything.** Remove `~/.ruxen`, remove `~/.cache/ruxen`, and
delete the `. "$HOME/.ruxen/env"` line from your shell rc file.
