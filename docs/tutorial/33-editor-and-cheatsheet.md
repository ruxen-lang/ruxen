# Editor Setup and Cheatsheet

You've made it through the tutorial. This last chapter is a two-fer: first, the editor setup you'll want for day-to-day work — the language server runs as a subcommand of `ruxen` and plugs into any LSP-aware editor. Second, a one-screen cheatsheet covering every keyword, operator, and common form so you can look something up without hunting through earlier chapters.

---

## Part 1: Editor Setup

### 1. The language server

Ruxen ships its own LSP. It's a subcommand of the main toolchain:

```bash
ruxen lsp
```

That command reads LSP requests on stdin and writes responses on stdout — it's not interactive. Editors spawn it and speak the protocol over those pipes.

Features in v1:

- Diagnostics (errors and warnings, live as you type).
- Hover (type information at the cursor).
- Go-to-definition.
- Document symbols.
- Formatting (routed through `ruxen fmt`).

### 2. VSCode

A first-party extension lives at `editors/vscode/` in the Ruxen repo. To install from source:

```bash
cd editors/vscode
npm install
npm run package
code --install-extension ruxen-vscode-*.vsix
```

After install, opening any `.rx` file activates the extension. It runs `ruxen lsp` from your `PATH`, so as long as `ruxen --version` works in a terminal, the extension finds it.

Override the binary location in `settings.json` if your install is non-standard:

```json
{
  "ruxen.lsp.path": "/opt/ruxen/bin/ruxen"
}
```

### 3. Neovim (built-in LSP)

Neovim 0.8+ ships an LSP client. Add this to `~/.config/nvim/init.lua`:

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = "ruxen",
  callback = function()
    vim.lsp.start({
      name = "ruxen",
      cmd = { "ruxen", "lsp" },
      root_dir = vim.fs.dirname(vim.fs.find({ "Ruxen.toml" }, { upward = true })[1]),
    })
  end,
})

vim.filetype.add({ extension = { rx = "ruxen" } })
```

Save the file, reopen any `.rx` buffer, and `:LspInfo` should show the `ruxen` server attached.

For syntax highlighting, install a community TextMate grammar (port from `editors/vscode/syntaxes/`) or use the tree-sitter parser once one ships.

### 4. Other editors

Any editor with an LSP client works:

**Helix** — add to `languages.toml`:

```toml
[[language]]
name        = "ruxen"
file-types  = ["rx"]
language-servers = ["ruxen-lsp"]

[language-server.ruxen-lsp]
command = "ruxen"
args    = ["lsp"]
```

**Sublime Text** — install the LSP package, point it at `ruxen lsp`.

**Emacs** — `lsp-mode` or `eglot` both accept a command list of `("ruxen" "lsp")`.

### 5. Format-on-save

Every editor's LSP client can route formatting requests to the server. Set "format on save" and Ruxen will canonicalise your file every time you save it — same output as `ruxen fmt <file>` from the command line.

---

## Part 2: Cheatsheet

A one-screen reference. Each entry is one line of intent followed by the smallest example.

### Bindings

```ruxen
let x = 42                          # immutable
var y = 0                            # mutable
y = 1
let x: Int = 42                      # explicit type
```

### Primitive types

```
Int  Int8  Int16  Int32  Int64
UInt UInt8 UInt16 UInt32 UInt64
ISize USize
Float Float32 Float64
Bool Char nil
```

### Numeric literals

```ruxen
42      0xFF      0b1010      0o777      1_000_000
3.14    1.0e3     2.5e-2
42i32   255u8     100u64
```

### Strings

```ruxen
"plain &str literal"
String.from("owned string")
"hi #{name}"                          # interpolation -> String
'no \escape \here'                    # raw (single quotes, verbatim)
'can have "quotes" inside'            # raw can hold double quotes
"""
multi
line
"""
```

### Characters

```ruxen
?a            ?Z            ?5           # Char literals
?\n           ?\t           ?\\          # escapes
?\u{1F600}                              # unicode scalar
```

### Control flow

```ruxen
if x > 0 then a elsif x < 0 then b else c end

while cond
  body
end

for x in 0...10                      # exclusive range (0..9)
for x in 0..10                       # inclusive range (0..10)

loop
  break
  continue
end

match value
  Pattern1 -> result1
  Pattern2 if guard -> result2
  _        -> default
end

if let Some(x) = maybe
  use(x)
end
```

### Functions

```ruxen
def greet                            # zero-arg
  puts "hi"
end

def double(x: Int) -> Int            # typed
  x * 2
end

def square(x: Int) -> Int { x * x }  # single-expression

def with_default(name: String, suffix: String = "!") -> String

def overload(v: Int)    -> String
def overload(v: String) -> String

def identity[T](x: T) -> T           # generic
def longest[a](x: &a String, y: &a String) -> &a String   # lifetime
```

### Methods

```ruxen
def name          -> String          # reading
def var rename(n: String)             # writing
def consume into  -> String          # consuming
def self.create   -> Self            # class method
```

### Classes

```ruxen
class User
  name: String
  age: Int

  def init(@name: String, @age: Int)
  end

  def display -> String
    "#{self.name} (#{self.age})"
  end
end

class Dog < Animal                   # inheritance
```

### Structs

```ruxen
struct Point
  x: Float
  y: Float
end
```

### Newtype

```ruxen
newtype UserId(Int)
let id = UserId(42)
puts "#{id.0}"                        # access inner
```

### Enums

```ruxen
enum Status
  Pending
  Running(since: Int)
  Done(when: Int)
end

let s = Status.Running(since: 100)
```

### Mixins

```ruxen
mixin Renderable
  def to_display -> String
end

mixin Greetable
  def name -> String                  # required
  def greet -> String                  # default
    "Hello, #{self.name}!"
  end
end

class User
  include Renderable
  include Greetable
  # ...
end

def show[T: Renderable](x: &T)        # generic bound
def show(x: &some Renderable)         # existential, monomorphic
def show(x: &any Renderable)          # existential, vtable
```

### Modules

```ruxen
module Http
  class Request end
end

use Http.Request
use Http.{Request, Response}
use Http.Client as HC
use package.utils.format               # this package
```

### Borrowing

```ruxen
&x                                    # read-only borrow
&var x                                # writable borrow
x.clone                               # explicit deep copy
move { |x| ... }                      # move-capturing closure
```

### Operators

```
+  -  *  /  %  **                     # arithmetic, power
== != <  >  <= >=                     # comparison
&& || !                               # logical
&  |  ^  ~  << >>                     # bitwise
?                                     # try (propagates Err / nil)
..  ...                               # range inclusive / exclusive
?.                                    # safe navigation
=  += -= *= /= %=                     # assignment
->                                    # function return / match arm
=>                                    # map literal pair
```

### Patterns

```ruxen
match value
  0                 -> "zero"
  n                 -> "n=#{n}"        # bind
  n if n > 10       -> "big"           # guard
  Status.Done(t)    -> "done #{t}"     # destructure
  (x, y)            -> "pair"
  "yes" | "y"       -> "affirm"        # or
  _                 -> "other"         # wildcard
  User(name, ..)    -> name            # rest
end
```

### Error handling

```ruxen
Result[T, E]                          # explicit
Option[T]    or    T?                # equivalent
Some(v)   nil                          # Option payloads (nil = the empty case)
Ok(v)     Err(e)                     # Result payloads
expr?                                 # propagate Err / nil

panic!("msg")
opt.unwrap!
res.expect!("msg")
```

### Collections

```ruxen
[1, 2, 3]                             # Array literal
[1, 2, 3] : [Int; 3]                  # fixed-size
{ "a" => 1, "b" => 2 }                # Hash literal
[1, 2, 3].to_set                      # Set
Array.new    Array.with_capacity(64)
Hash.new     Set.new
```

### Closures

```ruxen
let f = { |x: Int| x * 2 }
f.(10)
move { |x| x + n }                    # ownership-moving capture
nums.each { |n| puts n }
nums.map  { |n| n * 2 }
```

### Async

```ruxen
async def fetch() -> Int
  let n = compute().await
  n + 1
end

block_on(fetch())                     # drive from sync
Async.sleep(Duration.from_millis(50)).await
```

### Concurrency

```ruxen
let h = Thread.spawn_raw({ || work() })
JoinHandle.join_raw(h)

let m = Mutex.new(0)
let g = m.lock_raw
g.set(g.get + 1)

let s = SharedSync.new(value)
let s2 = s.clone

let a = AtomicI64.new(0)
a.fetch_add(1)
```

### FFI / unsafe

```ruxen
lib "m"
  def sqrt(x: Float) -> Float
end

unsafe
  let y = sqrt(2.0)
end
```

### Comments and directives

```ruxen
# line comment
let x = 1   # inline

#= block
   #= nested =#
   comment =#

## doc comment
def foo end

# fmt: off
preserved
# fmt: on

inline def fast_path(x: Int) -> Int
struct Header
  layout c       # or `layout packed`, `layout transparent`
end
```

### Project commands

```bash
ruxen new my_app
ruxen init
ruxen build [--release] [--bin name]
ruxen run [--release] [--bin name] [-- args]
ruxen check
ruxen test [--filter name]
ruxen bench file.rx [--filter name] [--iter-hint N]
ruxen fmt [--check] [--diff] [--stdin]
ruxen add dep [--version v] [--git url] [--path p] [--dev]
ruxen remove dep
ruxen update [dep]
ruxen tree
ruxen verify
ruxen clean
ruxen explain CODE
ruxen compile file.rx [-o out] [--release]
ruxen repl
ruxen lsp
```

---

## Where next

Done! Pick wherever you want to go from here:

- [Chapter 01 — Getting Started](01-getting-started.md) for the very beginning.
- [Chapter 06 — Classes and Structs](06-classes-and-structs.md) for class design.
- [Chapter 11 — Error Handling](11-error-handling.md) for the `Result` / `Option` idiom.
- [Chapter 17 — String Formatting](17-string-formatting-and-interpolation.md) for `Display` and format specs.
- [Chapter 24 — Async](24-async.md) for futures and `.await`.
- [Chapter 27 — Manifest and Deps](27-manifest-and-deps.md) for everything package-shaped.
- [Chapter 32 — Idiomatic Patterns](32-idiomatic-patterns.md) for the shapes that recur across the standard library.

The standard library itself (`library/std/`) is the best long-form example of idiomatic Ruxen. When in doubt about how to shape something, find a similar problem there and read.
