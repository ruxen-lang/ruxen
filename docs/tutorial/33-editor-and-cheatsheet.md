# Editor Setup and Cheatsheet

You've made it through the tutorial. This last chapter is a two-fer: first, the editor setup you'll want for day-to-day work — the language server runs as a subcommand of `ruxen` and plugs into any LSP-aware editor. Second, a cheatsheet covering the common forms so you can look something up without hunting through earlier chapters.

---

## Part 1: Editor Setup

### 1. The language server

Ruxen ships its own LSP [implemented]. It's a subcommand of the main toolchain:

```sh
ruxen lsp
```

That command reads LSP requests on stdin and writes responses on stdout — it's not interactive. Editors spawn it and speak the protocol over those pipes.

Features the server advertises, all [implemented]:

- Diagnostics (errors and warnings as you edit) [implemented].
- Hover (type information at the cursor) [implemented].
- Go to definition and go to type definition [implemented].
- Completion and signature help [implemented].
- Document and workspace symbols [implemented].
- Find references, document highlights and rename [implemented].
- Inlay hints, folding ranges and semantic tokens [implemented].
- Code actions [implemented].
- Formatting of a whole document or a range, with the same output as `ruxen fmt` [implemented].

### 2. VSCode

A first-party extension lives at `editors/vscode/` in the Ruxen repo. To install from source:

```sh
cd editors/vscode
npm install
npm run package
code --install-extension ruxen-*.vsix
```

After install, opening any `.rx` file activates the extension. In each open workspace folder it looks for `target/release/ruxen`, then `target/debug/ruxen`, and otherwise uses `ruxen` on your `PATH`; it runs that binary's `lsp` subcommand. As long as `ruxen --version` works in a terminal, the extension finds it.

Override the binary location in `settings.json` if your install is non-standard (`ruxen.server.args` sets the arguments, `["lsp"]` by default):

```json
{
  "ruxen.server.path": "/opt/ruxen/bin/ruxen"
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

For syntax highlighting, port the TextMate grammar from `editors/vscode/syntaxes/`; the server's semantic tokens also colour the buffer in clients that support them.

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

### 6. The REPL

`ruxen repl` [implemented] starts an interactive session. Type an expression or a statement and press Enter; `:type expr` (or `:t`) shows a type, `:reset` clears the session, `:help` lists the commands and `:quit` leaves. Session variables live in 8-byte slots between inputs. A redesign of those slots, so they can hold any value and the REPL no longer needs its C replay guards, is [planned](../../TODO.md#t-d19).

---

## Part 2: Cheatsheet

A reference card. Each entry is one line of intent followed by the smallest example, tagged with its status. When the Ruby form is not built yet, the form that works today follows it, captioned [deprecated].

### Variables

Example [implemented]:

```ruby
x = 42                     # a local; Ruby locals are always reassignable
x = 43
```

A typed local `count: Int = 0` is [planned](../../TODO.md#t-typed-local), and so are constants such as `MAX = 3` [planned](../../TODO.md#t-constants). Works today [deprecated] (`let` inside a `def`, replaced by `count: Int = 0`; the `let` / `var` binding keywords are replaced by plain `x = 1`):

```ruby
def tally -> Int
  let count: Int = 0
  count + 1
end
```

### Primitive types

Built-in types [implemented]:

```text
Int  Int8  Int16  Int32  Int64
UInt8 UInt16 UInt32 UInt64
ISize USize
Float Float32 Float64
Bool Char String nil
```

Sized types are meant for FFI and storage; whether they stay in everyday code is an open question (OQ-8) [planned](../../TODO.md#t-oq).

### Literals

Example [implemented]:

```ruby
n = 1_000_000
h = 0xFF
b = 0b1010
o = 0o777
f = 2.5e-2
small = 255u8              # width suffix (OQ-8)
yes = true
nothing = nil
words = %w[apple banana cherry]
```

### Strings and characters

Example [implemented]:

```ruby
name = "Ada"
s = "owned string"          # a literal is already an owned String
greeting = "hi #{name}\n"   # interpolation and escapes
path = 'C:\Users\me'        # single quotes: no escapes, no interpolation
quoted = 'can have "quotes" inside'
text = """
multi
line
"""
c = ?a                      # Char
nl = ?\n
```

The heredoc `<<~EOS ... EOS` is [planned](../../TODO.md#t-heredoc); until then `"""..."""` is the multi-line form. Symbols (`:active`) are [planned](../../TODO.md#t-symbols).

### Control flow

Example [implemented]:

```ruby
x = 5
if x > 0
  puts "positive"
elsif x < 0
  puts "negative"
else
  puts "zero"
end

label = x > 0 ? "yes" : "no"

unless x == 0
  puts "nonzero"
end

i = 0
while i < 3
  i += 1
end
until i == 0
  i -= 1
end

for k in 0...3             # exclusive: 0, 1, 2
  puts k
end
for k in 0..3              # inclusive: 0, 1, 2, 3
  puts k
end

case x
when 1, 2 then puts "small"
when 3..9 then puts "medium"
else puts "large"
end
```

Statement modifiers (`return if done`) work inside a method body [implemented] and at the top level are [planned](../../TODO.md#t-toplevel-stmts). `loop do ... end` is [planned](../../TODO.md#t-loop-do), and `next` in `while` / `until` / `for` is [planned](../../TODO.md#t-next-loops). Works today [deprecated] (`loop ... end` and `continue`, replaced by `loop do ... end` and `next`):

```ruby
n = 0
loop
  n += 1
  if n < 3
    continue
  end
  break
end
```

### Pattern matching

Example [implemented]:

```ruby
def describe(v: Int) -> String
  case v
  in 0 then "zero"
  in 1 | 2 then "one or two"
  in n if n > 10 then "big #{n}"
  in _ then "other"
  end
end
```

Array, hash, typed record, range and pin patterns (`in [first, *rest]`, `in Point(x:, y:)`, `in 3..5`, `in ^expected`) are [planned](../../TODO.md#t-patterns). The `match v ... pat -> expr ... end` expression and `if let Some(x) = opt` are [deprecated], replaced by `case/in`; narrowing with `if (x = opt)` is [planned](../../TODO.md#t-narrowing).

### Methods

Example [implemented]:

```ruby
def greet
  puts "hi"
end

def double(x: Int) -> Int
  x * 2
end

def square(x: Int) -> Int = x * x                  # endless def

def with_default(name: String, suffix: String = "!") -> String
  "#{name}#{suffix}"
end

def show(v: Int) -> String = "int #{v}"            # overloads by type
def show(v: String) -> String = "string #{v}"

def identity[T](x: T) -> T
  x
end

def longest[a](x: &a String, y: &a String) -> &a String
  x.size > y.size ? x : y
end
```

The brace body `def square(x: Int) -> Int { x * x }` is [deprecated], replaced by the endless form. Keyword arguments at call sites and rest arguments (`*parts`) are [planned](../../TODO.md#t-kwargs) and [planned](../../TODO.md#t-rest-args).

### Classes

Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
class User
  name: String
  age: Int

  def init(@name: String, @age: Int)
  end

  def display -> String                 # reads self
    "#{@name} (#{@age})"
  end

  def var birthday                      # may change self
    @age += 1
  end

  def consume into_name -> String       # takes ownership of self
    @name.clone
  end

  def self.guest -> User                # class method
    User.new("guest", 0)
  end
end

class Admin < User                      # inheritance
end

u = User.new("Ada", 36)
u.birthday
puts u.display
```

`attr_reader` and friends are [planned](../../TODO.md#t-attr-accessors); today read a field through a method, or as `@name` inside the class.

### Value types and wrappers

Ruby forms [planned](../../TODO.md#t-data-define) (`Data.define`) and [planned](../../TODO.md#t-struct-new) (`Struct.new`):

```ruby
Point = Data.define(x: Int, y: Int)     # immutable value
Pixel = Struct.new(x: Int, y: Int)      # mutable value
UserId = Data.define(value: Int)        # one-field wrapper (newtype)
```

`struct Point ... end`, `newtype UserId(Int)` and tuple access `pair.0` are [deprecated], replaced by the forms above; until then, write a small class as in the Classes entry.

### Variants (enums)

Ruby form [planned](../../TODO.md#t-oq) (OQ-1, how a module declares its closed set of variants):

```ruby
module Status
  Pending = Data.define
  Running = Data.define(since: Int)
end

def label(s: Status) -> String
  case s
  in Status::Pending then "pending"
  in Status::Running(since:) then "running since #{since}"
  end
end
```

Works today [deprecated] (`enum` and `Status.Running(...)`, replaced by the form above):

```ruby
enum Status
  Pending
  Running(since: Int)
end

def label(s: &Status) -> String
  case s
  in Status.Pending then "pending"
  in Status.Running(since) then "running since #{since}"
  end
end

puts label(Status.Running(100))
```

### Modules and mixins

Namespaces, Example [implemented], with the `def init` constructor [deprecated] (replace with `def initialize` [planned](../../TODO.md#t-initialize)):

```ruby
module Http
  class Request
    path: String

    def init(@path: String)
    end
  end
end

r = Http::Request.new("/")
```

Load other files with `require "lib/thing"` and `require_relative "../thing"` [implemented]. The `use Http.Request` import and the `package.utils` path root are [deprecated], replaced by `require` and qualified names.

A module you `include` for shared behaviour is the Ruby form [planned](../../TODO.md#t-module-mixins):

```ruby
module Greetable
  def name -> String                    # required: no body
  def greet -> String                   # default
    "Hello, #{name}!"
  end
end
```

Works today [deprecated] (`mixin` and `self.name`, replaced by `module` and a bare `name` [planned](../../TODO.md#t-implicit-self)):

```ruby
mixin Greetable
  def name -> String
  def greet -> String
    "Hello, #{self.name}!"
  end
end

class Person
  first: String
  include Greetable

  def init(@first: String)
  end

  def name -> String
    @first.clone
  end
end

puts Person.new("Ada").greet
```

`include Comparable` [implemented] gives `<`, `<=`, `between?` and `clamp` from one `<=>`. Bounds on a type parameter are spelled `[T < Greetable]` [planned](../../TODO.md#t-d7-respell); `[T: Greetable]` and `where` clauses are [deprecated], replaced by that form. `some Greetable` and `any Greetable` work as parameter types [implemented].

### Borrowing

Example [implemented]:

```ruby
def total(xs: &Array[Int]) -> Int
  sum = 0
  for x in xs
    sum += x
  end
  sum
end

def add_one(xs: &var Array[Int])
  xs.push(1)
end

nums = [1, 2]
puts total(nums)            # the call site borrows: no & needed
add_one(&var nums)          # a writable borrow is explicit
copy = nums.clone           # an explicit deep copy
```

### Operators

All [implemented] unless the row says otherwise:

| Operators | Meaning |
|---|---|
| `+ - * / %` | arithmetic |
| `== != < > <= >=`, `<=>` | comparison |
| `&& \|\| !`, `and or not` | logic (operands must be `Bool` today; truthiness of `T?` [planned](../../TODO.md#t-truthiness)) |
| `& \| ^ << >>` | bitwise |
| `..` / `...` | inclusive / exclusive range |
| `&.` | safe navigation (nil-safe call) |
| `= += -= *= /=` | assignment |
| `=>` | hash literal pair |
| `=~` | regex match [planned](../../TODO.md#t-match-op) (`~=` is [deprecated]) |
| postfix `?` | try operator [deprecated], replaced by exceptions [planned](../../TODO.md#t-exceptions) |

### Missing values and errors

Optional values, Example [implemented]:

```ruby
def find(id: Int) -> String?
  if id == 1
    "ada"
  else
    nil
  end
end

puts find(2).nil?
puts find(1)&.upcase.nil?
```

Errors are Ruby exceptions in the Ruby form, [planned](../../TODO.md#t-exceptions):

```ruby
class NotFound < StandardError
end

def load(id: Int) -> String
  raise NotFound, "no user #{id}" if id != 1
  "ada"
end

begin
  puts load(2)
rescue NotFound => e
  warn e.message
end
```

Works today [deprecated] (`Result` with `Ok` / `Err` and `case/in`, replaced by `raise` / `rescue`):

```ruby
def load(id: Int) -> Result[String, String]
  if id == 1
    Ok("ada")
  else
    Err("no user #{id}")
  end
end

case load(2)
in Ok(name) then puts name
in Err(msg) then warn(msg)
end
```

`abort "msg"` and `exit(1)` end the process [implemented]. `Option[T]`, `Some(v)`, `unwrap!`, `expect!`, the `?` operator and `panic!` are [deprecated], replaced by `T?`, the value itself, `case/in`, exceptions and `raise` / `abort`.

### Collections

Example [implemented]:

```ruby
xs = [1, 2, 3]
words = %w[a b c]
ages = { "ada" => 36, "alan" => 41 }
puts ages["ada"]
puts ages.key?("alan")
seen = [1, 2, 2].to_set
```

The empty hash literal `{}` is [planned](../../TODO.md#t-empty-hash); today write a typed `Hash.new` inside a `def`.

### Blocks and lambdas

Example [implemented]:

```ruby
nums = [1, 2, 3]
nums.each { |n| puts n }
doubled = nums.map { |n| n * 2 }
loud = ["ada"].map(&:upcase)

double = ->(x: Int) { x * 2 }
puts double.(10)

def twice
  yield
  yield
end
twice do
  puts "hi"
end
```

A block is borrowed for one call (`Fn[(Int) -> Int]`); a lambda owns its captures and can be stored or returned (`Proc[(Int) -> Int]`); `move { |x| ... }` makes a block own its captures [implemented]. The empty-pipes block `{ || ... }` and the paren closure type `Fn(Int) -> Int` are [deprecated], replaced by `{ ... }` and `Fn[(Int) -> Int]`.

### Async

Example [implemented], with `let` [deprecated] (replaced by `x = ...`, which does not compile inside an `async def` yet [planned](../../TODO.md#t-await-gaps)) and `Duration.from_millis` [deprecated] (replaced by a Ruby `sleep` in the fiber model):

```ruby
async def compute -> Int
  41
end

async def fetch -> Int
  let n = compute().await
  n + 1
end

puts block_on(fetch())
block_on(Async.sleep(Duration.from_millis(50)))
```

Ruby's fiber scheduler (`Async do ... end`, `task.wait`) is [planned](../../TODO.md#t-fibers); when it lands, `async def`, `.await` and `block_on` become [deprecated].

### Threads

Ruby form [planned](../../TODO.md#t-thread-api):

```ruby
t = Thread.new { 40 + 2 }
puts t.value

lock = Mutex.new
lock.synchronize do
  puts "only one thread at a time"
end
```

Works today [deprecated] (`Thread.spawn`, `join!` and `lock!`, replaced by `Thread.new`, `value` and `synchronize`):

```ruby
handle = Thread.spawn { 40 + 2 }
puts handle.join!

m = Mutex.new(0)
g = m.lock!
g.set(g.get + 1)

shared = SharedSync.new(5)
other = shared.clone

hits = AtomicI64.new(0)
hits.fetch_add(1)
```

### FFI

Example [implemented]:

```ruby
lib "m"
  def sqrt(x: Float) -> Float
end

puts sqrt(2.0)
```

`unsafe ... end` blocks are [implemented]; whether they stay or raw pointers move into `lib` blocks only is an open question (OQ-9) [planned](../../TODO.md#t-oq). The `layout c` directive is [implemented] for C-compatible records; it goes in a `struct` body today, and `struct` is [deprecated] (replaced by `Data.define` [planned](../../TODO.md#t-data-define)).

### Comments and hints

Example [implemented]:

```ruby
# line comment
x = 1   # trailing comment

#= block
   #= nested =#
   comment =#

## doc comment for the next item
def foo
end

# fmt: off
kept   =   "exactly as written"
# fmt: on

class Hot
  inline def fast(x: Int) -> Int = x + 1
end
```

Ruby's `=begin ... =end` block comment is [planned](../../TODO.md#t-begin-comment); `#= ... =#` stays until then. `inline def` works on a method in a class body [implemented] and on a top-level method is [planned](../../TODO.md#t-toplevel-stmts).

### Project commands

All [implemented] (`--bin` names the output file; several binaries per package are [planned](../../TODO.md#t-multi-bin)):

```sh
ruxen new my_app
ruxen init
ruxen build [--release] [--bin out-name] [--target TRIPLE]
ruxen run [--release] [-- args]
ruxen check
ruxen test [filter] [--release] [--list]
ruxen bench file.rx [--filter name] [--iter-hint N]
ruxen fmt [--check] [--diff] [--stdin] [files]
ruxen add dep [--version v] [--git url] [--path p] [--dev]
ruxen remove dep
ruxen update [dep]
ruxen tree
ruxen verify
ruxen clean
ruxen explain CODE
ruxen compile file.rx [-o out]
ruxen upgrade --from-source PATH
ruxen publish [--dry-run]
ruxen repl
ruxen lsp
```

---

## Where next

Done! Pick wherever you want to go from here:

- [Chapter 01 — Getting Started](01-getting-started.md) for the very beginning.
- [Chapter 06 — Classes and Structs](06-classes-and-structs.md) for class design.
- [Chapter 11 — Error Handling](11-error-handling.md) for handling missing values and errors.
- [Chapter 17 — String Formatting](17-string-formatting-and-interpolation.md) for interpolation, `to_s` and `inspect`.
- [Chapter 24 — Async](24-async.md) for `async def` and `.await`.
- [Chapter 27 — Manifest and Deps](27-manifest-and-deps.md) for everything package-shaped.
- [Chapter 32 — Idiomatic Patterns](32-idiomatic-patterns.md) for the shapes that recur across the standard library.

The standard library itself (`library/std/`) is the best long-form example of idiomatic Ruxen. When in doubt about how to shape something, find a similar problem there and read.
