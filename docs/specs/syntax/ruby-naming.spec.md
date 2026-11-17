# Spec: Ruxen Language Surface (Ruby)

Status: Canonical (rewritten 2026-10-15; replaces the 2026-05-16 draft)
Owner: language
Scope: everything a user types: files, bindings, literals, methods,
       blocks, control flow, absence, errors, classes, value types,
       modules, typing annotations, the ownership surface, FFI, and
       the naming policy for the standard library.

**This file is the source of truth for how Ruxen looks on the page.**
When a tutorial, stdlib spec, requirements doc, error doc, fixture or
example disagrees with it, this file wins and the other one is updated.

---

## 0. The rule

**Ruxen is Ruby, statically typed and compiled to native code.** The goal
is full Ruby parity. Ruxen adds two things to Ruby and nothing else:

1. **Static types** (the Ruxen addition to Ruby): `name: Type` annotations
   on parameters and fields, `-> Type` return annotations, generic
   parameters `[T]` with RBS-style bounds `[T < Bound]`, and `T?` for
   "a `T` or `nil`". Everything else is inferred.
2. **Ownership** (the one thing taken from Rust): moves, borrows `&x` /
   `&var x`, receiver modes `def var` / `def consume`, `move` closures,
   lifetimes as a rare escape hatch, `Clone`/`Drop`, `Send`/`Sync`,
   `Box`/`SharedSync`. This is how Ruxen runs without a garbage collector.
   Section 15 lists the full surface.

Any other Rust syntax or vocabulary in the language is a mistake made by
earlier drafts and is being removed. Docs must not teach it.

<!-- tag-vocabulary -->

### 0.1 Status tags

Every construct below carries one tag:

| Tag | Meaning |
|---|---|
| **[implemented]** | The compiler accepts it today and it does what this file says. Verified on 2026-10-25 by a probe per claim on a fresh release build of `6bf146ac`: `ruxen compile --emit=ast` for syntax, `--emit=hir` for typing, and compile and run for behaviour. A claim that compiles but misbehaves keeps the tag with a "known issue" link. Appendix A lists the source locations. |
| **[planned]** | Decided by the owner, not built yet. Docs may show it only when marked as planned, and the tag links its open checkbox in `TODO.md` or `docs/TASKS.md`. |
| **[deprecated]** | A Rust form still accepted during the migration. The entry names its replacement. It will first get a warning with a fix-it, then become an error. |
| **[retired]** | Already rejected by the compiler. Listed only so docs stop teaching it. |

A tag followed by "(OQ-n)" means the form exists but its future is still
an open question (section 20).

### 0.2 Migration policy

This reverses the old draft's "no deprecation, old spellings stop parsing
the day the rename lands". The migration is:

1. Add the Ruby form ([planned] becomes [implemented]), with an e2e fixture
   and a negative test, in the compiler, REPL, LSP, IDE and formatter alike.
2. `ruxen fmt` learns to print the Ruby form, so it doubles as the codemod.
3. Rewrite the stdlib, fixtures and examples.
4. The Rust form gets a registered deprecation warning with a fix-it, then
   becomes an error.

**Stdlib methods are the exception:** a Rust-named method that has a Ruby
twin (`len` next to `size`, `to_string` next to `to_s`) becomes a
deprecated alias. It is never deleted.

<!-- /tag-vocabulary -->

---

## 1. Programs and files

| Construct | Ruby form | Status |
|---|---|---|
| Entry point | Top-level statements run in order; there is no `main`. | [implemented] (the parser moves top-level expression statements into a synthesized `main`). A top-level parallel assignment, statement modifier or `inline def` does not parse yet: [planned](../../../TODO.md#t-toplevel-stmts) |
| Explicit entry point | `def main ... end` | [deprecated], replace with top-level statements. The parser will decide the entry once and mark it, so later stages stop matching the name `main` [planned](../../../TODO.md#t-d3) |
| Loading code | `require "lib/thing"`, `require_relative "../thing"` | [implemented] |
| Importing names | `use std.io.Stdout`, `use a.b.{C, D}`, `use x as y` | [deprecated], replace with `require` and qualified names (`IO::Stdout`) |
| "This package" path root | `package.utils` | [deprecated], replace with `require_relative` |
| Namespace separator (expressions) | `Http::Request.new`, `Color::Red` | [implemented] (`.` is also accepted today) |
| Namespace separator (type positions) | `req: Http::Request` | [planned](../../../TODO.md#t-type-colons); today only `Http.Request` parses, plus a leading `::` for a root path |
| Line comment | `# text` | [implemented] |
| Doc comment | `## text` on the line(s) before an item | [implemented] |
| Block comment | `=begin ... =end` | [planned](../../../TODO.md#t-begin-comment) |
| Block comment (Ruxen form) | `#= ... =#` (nestable) | [implemented]; kept until `=begin` lands, then (OQ-12) |

Example [implemented]:

```ruby
require_relative "models/user"

## Greets everyone on the list.
def greet_all(names: Array[String])
  names.each { |n| puts "Hello, #{n}!" }
end

greet_all(["Ada", "Matz"])
```

---

## 2. Variables and constants

| Construct | Ruby form | Status |
|---|---|---|
| Local variable | `count = 0`; later `count += 1` | [implemented] on this branch: an assignment to an undeclared name declares a mutable local (`resolve/control_flow.rs`) |
| Typed local (Ruxen addition) | `bytes: Array[UInt8] = []` | [planned](../../../TODO.md#t-typed-local); today write `let bytes: Array[UInt8] = []` inside a `def` (a top-level `let` does not parse) |
| Immutable / mutable binding keywords | `let x = 1`, `var x = 1` | [deprecated], replace with `x = 1`. Ruby locals are always reassignable. |
| Constant | `MAX_RETRIES = 3` at top level or in a class/module body | [planned](../../../TODO.md#t-constants); today `const MAX_RETRIES = 3` |
| `const` item | `const NAME: Type = value` | [deprecated], replace with `NAME = value` |
| Parallel assignment | `a, b = 1, 2`; swap `a, b = b, a`; splat `first, *rest = xs` | [planned](../../../TODO.md#t-parallel-assign) for new names and at top level. Today it works inside a `def` when every target, the splat target included, is already declared (`a = 0; b = 0; a, b = 1, 2`) [implemented] |
| Destructuring a returned pair | `q, r = divmod(17, 5)` | [planned](../../../TODO.md#t-parallel-assign); today a single right-hand value destructures only from an Array, and pairs use `let (q, r) = ...` inside a `def` |
| Tuple / struct destructuring with `let` | `let (a, b) = pair`, `let Point { x, y } = p` | [deprecated], replace with `a, b = pair` and `case p in {x:, y:}` |
| Conditional and compound assignment | `x \|\|= 1`, `x &&= y`, `+=`, `-=`, `<<=` and the rest | [implemented] for `+=`, `-=`, `<<=` and the rest, and for `\|\|=` / `&&=` on a `Bool`; `\|\|=` on a `T?` or an `Int` [planned](../../../TODO.md#t-truthiness) (E0731 today) |

A local's type is fixed by its first assignment (static typing). Assigning
a value of another type later is a type error, not a new binding. A local
that may be absent is typed `T?` (section 7).

---

## 3. Literals

| Construct | Ruby form | Status |
|---|---|---|
| Absence / truth | `nil`, `true`, `false` | [implemented] |
| Integers | `42`, `1_000_000`, `0xff`, `0b101`, `0o17` | [implemented] |
| Floats | `3.14` | [implemented]; a whole Float prints without `.0` (`puts 3.0` prints `3`) [known issue](../../../TODO.md#t-float-to-s) |
| Interpolated string | `"hi #{name}\n"` | [implemented] |
| Literal string | `'C:\Users\me'` (no interpolation, no escapes) | [implemented] |
| Multi-line string | `<<~EOS ... EOS` | [planned](../../../TODO.md#t-heredoc); today `"""..."""` is the multi-line form |
| Character | `?a`, `?\n` (type `Char`, a Ruxen addition) | [implemented] |
| Word array | `%w[a b c]` | [implemented] |
| Symbol | `:active`, `%i[a b]` | [planned](../../../TODO.md#t-symbols). Today a `:name` token is read only in `private :a, :b` and `&:upcase`. |
| Array | `[1, 2, 3]` | [implemented] |
| Hash | `{ "k" => 1 }` | [implemented] |
| Hash with symbol keys | `{ name: "Ada", age: 36 }` | [planned](../../../TODO.md#t-symbols) |
| Empty Hash | `{}` | [planned](../../../TODO.md#t-empty-hash); today `{}` parses as an empty block, so write a typed `Hash.new` (`let h: Hash[String, Int] = Hash.new` inside a `def`); an untyped `Hash.new` panics on its first insert [known issue](../../../TODO.md#t-hash-new-key) |
| Ranges | `0..3` (inclusive), `0...3` (exclusive) | [implemented] in `for` and `case/when`; `Range` methods (`each`, `size`, `to_a`) fail to compile [known issue](../../../TODO.md#t-range-methods) |
| Regex | `/ab+c/i` | [implemented] |
| Regex match operator | `str =~ /re/` | [planned](../../../TODO.md#t-match-op); today the operator is spelled `~=` |
| Numeric suffixes | `200u8`, `3.14f32` | [implemented] (OQ-8) |
| Rust format specs in interpolation | `"#{x:?}"`, `"#{n:>5}"` | [deprecated], replace with `x.inspect`, `n.to_s.rjust(5)` or `format("%5d", n)` |
| `None`, `r"..."`, `'\r'` char, `'\u{...}'`, `..=`, `()` | | [retired] (E0008, E0009 and parse errors): use `nil`, `'...'`, `?\r`, `"\u{...}"`, `..`, `nil` |

Every string literal is an owned `String`. There is no `str` or `&str`
type (E0730); a borrowed string is `&String`.

---

## 4. Methods

| Construct | Ruby form | Status |
|---|---|---|
| Definition | `def area(w: Float, h: Float) -> Float ... end` | [implemented] (annotations are the Ruxen addition) |
| Default argument | `def greet(name: String = "you")` | [implemented] |
| Endless method | `def double(x: Int) = x * 2` | [implemented] |
| Brace body | `def f -> T { expr }` | [deprecated], replace with the endless form |
| Implicit return | the last expression is the value; `return x` also works | [implemented] |
| Predicate and bang names | `def empty?`, `def save!` | [implemented]. User methods may end in `!` (B87); only `panic!` and the retired collection macros are compiler macros. |
| Operator methods | `def +(o: Vec2) -> Vec2`, `def ==`, `def <=>`, `def [](i: Int)`, `def []=`, `def <<`, unary `def -@` | [implemented] |
| Class methods | `def self.parse(s: String) -> Version` | [implemented] |
| Aliases | `alias size length` (space form, no comma) | [implemented] |
| `alias_method :new, :old` | | [planned](../../../TODO.md#t-alias-method) |
| Visibility | `private` / `protected` / `public` as section markers, and `private :a, :b` | [implemented] |
| Protected semantics | Ruby's: callable with an explicit receiver of the same class family from inside it | [planned](../../../TODO.md#t-protected); the 2026-05 draft specified subclass-only visibility, so check behaviour against this |
| Calls without parentheses | `puts "hi"`, `box.button "Save" do ... end` | [implemented] for a method call with arguments and for `puts`, `print`, `p`, `warn` and `abort` with an argument. Other bare calls (`exit 1`, `sleep 0`), an argument that starts with `[` (`puts [1, 2].sum`), a bare zero-argument call used as a value (`puts five` passes the function) and a brace block on a bare call with no block parameters (`run { ... }`) are [planned](../../../TODO.md#t-f45); write the parentheses today |
| Keyword arguments at call sites | `connect(host: "x", port: 80)`; `def connect(*, host: String, port: Int = 80)` for keyword-only | [planned](../../../TODO.md#t-kwargs) (OQ-4 decided 2026-11-03, Crystal's rule: any parameter may be passed by name, positional arguments first; a bare `*` makes the following parameters keyword-only; definitions keep `name: Type`). Today `f(w: 3)` is a parse error |
| Rest arguments | `def log(*parts: String)`, `**opts` | [planned](../../../TODO.md#t-rest-args) |
| Inline hint (Ruxen addition) | `inline def fast(x: Int) -> Int` | [implemented] on a method in a class body; on a top-level function [planned](../../../TODO.md#t-toplevel-stmts) |
| Explicit receiver parameter | `def m(&self)`, `def m(self, x)` | [retired] outside `lib` blocks (`parse_param` requires `name: Type`) |

---

## 5. Blocks, procs and lambdas

| Construct | Ruby form | Status |
|---|---|---|
| Block with a call | `xs.each { \|x\| puts x }`, `xs.each do \|x\| ... end` | [implemented] |
| `yield` | `yield`, `yield x, y` inside a method that receives a block | [implemented] |
| Named block parameter | `def each(&block: Fn[(T) -> nil])` | [implemented] (block type spelling: OQ-5) |
| `block_given?` | | [implemented] (alias of `block_defined?`) |
| Symbol to proc | `names.map(&:upcase)` | [implemented] |
| Block forwarding | `@items.each(&block)` | [implemented] (`&block` / bare `&` in argument position; the receiving parameter still needs its type, `&block: Fn[...]`) |
| `next` | ends the current block iteration, optionally with a value | [implemented] inside blocks only (`next 0 if x == 2` in `map`); a bare `next if cond` does not parse and `next` inside `each` fails to compile [known issue](../../../TODO.md#t-next-block); see section 6 for loops |
| Calling a stored block | `blk.(x)` | [implemented] |
| `blk.call(x)` | | [implemented] |
| `blk[x]` | | [planned](../../../TODO.md#t-lambda-call) |
| Lambda | `double = ->(x: Int) { x * 2 }`, `-> { work }`, `->(x) do ... end` | [implemented]; owns its captures (capturing a non-Copy variable moves it). See `docs/decisions/blocks-procs-lambdas.md` |
| `lambda { \|x\| ... }`, `proc { \|x\| ... }`, `Proc.new { ... }` | | [implemented]; own their captures. Strict arity and `return` semantics for procs: [planned](../../../TODO.md#t-lambda-call) |
| Closure types | `Fn[(Int) -> Int]` for a block (borrowed for the call, cannot be stored: E1018); `Proc[(Int) -> Int]` for a stored proc or lambda | [implemented]; OQ-5 decided 2026-10-17 |
| Closure type, paren form | `Fn(Int) -> Int`, `\|Int\| -> Bool` | [deprecated], replace with `Fn[(Int) -> Int]` |
| Capture-capability types | `FnMut(...)`, `FnOnce(...)` bounds | [implemented] (OQ-5) |
| Empty pipes | `{ \|\| work }` | [deprecated], replace with `{ work }` |
| `move` capture | `move { \|x\| x + n }`, `move do ... end` | [implemented]; ownership surface, see section 15 |

---

## 6. Control flow

| Construct | Ruby form | Status |
|---|---|---|
| Conditionals | `if` / `elsif` / `else` / `end`, `unless ... else ... end` | [implemented] |
| Loops | `while`, `until`, `for x in xs` | [implemented] |
| Statement modifiers | `return if done`, `x += 1 while x < 10`, `unless`, `until` | [implemented] in a method body; at top level [planned](../../../TODO.md#t-toplevel-stmts) |
| Ternary | `ok ? "yes" : "no"` | [implemented] |
| Low-precedence logic | `and`, `or`, `not` | [implemented] |
| Value dispatch | `case x when 1, 2 then ... when 3..9 then ... else ... end` | [implemented] |
| Pattern matching | `case v in pattern [if guard] then ... else ... end` | [implemented] (`then` or a newline separates the pattern from the body) |
| `->` arm separator inside `case/in` | `in 1 -> "one"` | [deprecated], replace with `then` or a newline |
| `match` expression | `match v; pat -> expr; end` | [deprecated], replace with `case/in`. Frees the name for `String#match`. |
| `if let` / `while let` | `if let Some(x) = opt` | [deprecated], replace with `if (x = opt)` [planned](../../../TODO.md#t-narrowing) or `case opt in nil ... in x ...` [implemented] |
| Infinite loop | `loop do ... end` (a `Kernel` method taking a block) | [planned](../../../TODO.md#t-loop-do); today the `loop ... end` keyword form, which is [deprecated] |
| Skip to next iteration | `next` in `while`, `until`, `for` | [planned](../../../TODO.md#t-next-loops); `next` only works inside blocks today |
| `continue` | | [deprecated], replace with `next` |
| `break` with optional value | | [implemented] |
| Truthiness of `T?` | `if user`, `user \|\| default` | [planned](../../../TODO.md#t-truthiness); today `&&`, `\|\|`, `and`, `or` require `Bool` operands (E0731) |

### 6.1 Patterns (`case/in`)

| Pattern | Example | Status |
|---|---|---|
| Literal | `in 0`, `in "x"`, `in true` | [implemented] |
| Range of literals | `in 3..5` | [planned](../../../TODO.md#t-patterns) |
| `nil` | `in nil` | [implemented]; a binding arm after it (`in x`) binds the `T?`, not the value [known issue](../../../TODO.md#t-case-in-nil) |
| Binding, wildcard | `in n`, `in _` | [implemented] |
| Alternatives | `in 1 \| 2` | [implemented] |
| Guard | `in n if n > 10` | [implemented] |
| Typed record pattern | `in Point(x:, y:)`, `in Shape::Circle(radius:)` | [planned](../../../TODO.md#t-patterns); today `TypeName(field: pat)` and `Enum.Variant(pat)` parse |
| Array pattern | `in [first, *rest]` | [planned](../../../TODO.md#t-patterns) |
| Hash pattern | `in {name: String => name}` | [planned](../../../TODO.md#t-patterns) |
| Class check with binding | `in Integer => n` | [planned](../../../TODO.md#t-patterns) |
| Pin | `in ^expected` | [planned](../../../TODO.md#t-patterns) |
| `Some(x)`, `Ok(v)`, `Err(e)` patterns | | [deprecated], replace with `in nil` / `in x` for absence, and `rescue` for errors |

---

## 7. Absence: `T?` and `nil`

There is one way to say "maybe missing": the type `T?` and the value `nil`.

| Construct | Ruby form | Status |
|---|---|---|
| Optional type | `def find(id: Int) -> User?` | [implemented] |
| Absent value | `nil` | [implemented] |
| Present value | the value itself: `return user` in a `-> User?` method | [implemented] for typed bindings, the tail expression and `return` (automatic wrapping) |
| Safe navigation | `user&.name&.upcase` | [implemented] |
| Default when nil | `name = user&.name \|\| "guest"` | [planned](../../../TODO.md#t-truthiness) (needs truthiness, section 6) |
| Narrowing | `if (user = find(id))` ... `user` is `User` inside | [planned](../../../TODO.md#t-narrowing) |
| Must be present | `find(id) or raise NotFound` | [planned](../../../TODO.md#t-exceptions) (needs section 8) |
| `Some(x)` constructor | | [deprecated], replace with `x` |
| `Option[T]` in user code | | [deprecated], replace with `T?` |
| `??` operator | `a ?? b` | [deprecated], replace with `a \|\| b` |
| `?.` | `find(1)?.name` (today: try operator, then a call) | [deprecated], replace with `&.` |
| `unwrap!`, `expect!("...")`, `unwrap_or`, `ok_or`, `map_err`, `and_then` | | [deprecated], replace with `x \|\| default`, `x or raise ...`, `hash.fetch(k)`, `case/in` |
| `nil` as the raw-pointer null in FFI | `ptr == nil` inside a `lib` wrapper | [implemented] |

References (`&T`, `&var T`) are never `nil`.

`Option` remains the compiler's internal representation of `T?`. Users do
not write it.

---

## 8. Errors: Ruby exceptions

Errors are Ruby exceptions, checked statically. The compiler lowers them
to `Result`-shaped returns internally, so there is no stack unwinding and
no hidden runtime cost. Users never write `Result`, `Ok`, `Err` or `?`.

| Construct | Ruby form | Status |
|---|---|---|
| Raise | `raise "boom"` (a `RuntimeError`), `raise NotFound, "no user #{id}"`, `raise NotFound.new(id)` | [planned](../../../TODO.md#t-exceptions). The parser already takes `raise` as a paren-less call name, but nothing defines it. |
| Re-raise | bare `raise` inside `rescue` | [planned](../../../TODO.md#t-exceptions) |
| Handle | `begin ... rescue NotFound => e ... rescue => e ... else ... ensure ... end` | [planned](../../../TODO.md#t-exceptions) |
| Method-level handlers | `def load ... rescue IOError => e ... ensure ... end` | [planned](../../../TODO.md#t-exceptions) |
| Retry | `retry` inside `rescue` restarts the `begin` body | [planned](../../../TODO.md#t-exceptions) |
| Exception classes | `class NotFound < StandardError; end` with `message` | [planned](../../../TODO.md#t-exceptions); replaces `mixin Error` (`def message`) in `std/core` |
| Abort the process | `abort "msg"`, `abort(msg)`, `exit(1)`, `exit!(1)` | [implemented] (`std/core`, `std/process`); the parenless `exit 1` [planned](../../../TODO.md#t-f45) |
| `panic!("msg")` | | [deprecated], replace with `raise "msg"` (recoverable) or `abort "msg"` (not recoverable) |
| `Result[T, E]` return types, `Ok(v)`, `Err(e)` | | [deprecated], replace with returning `T` and raising |
| Postfix `?` try operator | `read(path)?` | [deprecated], replace with nothing: exceptions propagate on their own |
| `Into[T]` error conversion for `?` | | [deprecated] with `?` |

Static rules:

- Each method has a **raise set**: the exception classes it can raise and
  does not rescue, including the raise sets of the methods it calls. The
  compiler infers it; it is part of the method's type. (Whether a method
  may also declare it explicitly is OQ-2.)
- A `rescue` clause narrows the raise set of its `begin` body. `rescue => e`
  with no class catches `StandardError`, as in Ruby.
- `ensure` runs on both the normal and the raising exit. It lowers onto
  the same cleanup path as `drop` (section 15).
- An exception that escapes the top level prints its message and class to
  STDERR and exits with status 1, as Ruby does.

The Ruby form [planned](../../../TODO.md#t-exceptions):

```ruby
class NotFound < StandardError
end

def find_user(id: Int) -> User
  users.find { |u| u.id == id } or raise NotFound, "no user #{id}"
end

attempts = 0
begin
  user = find_user(42)
  puts user.name
rescue NotFound => e
  attempts += 1
  retry if attempts < 3
  warn e.message
ensure
  log.close
end
```

---

## 9. Classes

Classes are heap objects with identity.

| Construct | Ruby form | Status |
|---|---|---|
| Class and inheritance | `class Admin < User ... end` | [implemented] |
| Construction | `User.new("Ada")` | [implemented] |
| Constructor | `def initialize(name: String)` | [planned](../../../TODO.md#t-initialize); today the constructor is `def init` |
| `def init` | | [deprecated], replace with `def initialize` |
| Instance variables | `@name` (read), `@name = v` (write) | [implemented] (sugar for the field) |
| A bare field name in a method | `name` for the field `@name` | Never a field read: a bare name is a local variable or a method call. With neither in scope it is E0734, whose help says to read it as `@name` (or, in a class method, from an instance) [implemented] |
| Assigning parameters to ivars | `def initialize(@name: String, @age: Int)` | [implemented] in the `def init(@name: String)` spelling |
| `self.name` field access inside the class | | [deprecated], replace with `@name` |
| Ivar type declaration (Ruxen addition) | `name: String` in the class body | [implemented] (OQ-3: possible RBS-style `@name: String`) |
| Accessors | `attr_reader :name`, `attr_writer :name`, `attr_accessor :name` | [planned](../../../TODO.md#t-attr-accessors) |
| Ivar privacy | ivars are private; reach them from outside through accessors | [planned](../../../TODO.md#t-ivar-privacy); today declared fields are public by default and readable as `obj.name` |
| `super` | `super`, `super(args)` | [implemented] inside `init` (`super(n)`); in any other method [planned](../../../TODO.md#t-super) (E1127 today) |
| Reopening a class | `class String; def shout = upcase + "!"; end` | [planned](../../../TODO.md#t-reopen-class). Today a second `class String` collides with the stdlib type (E0727). |
| Nested classes and constants in a class body | `class Http; class Request; end; TIMEOUT = 30; end` | [planned](../../../TODO.md#t-nested-class); today nest inside `module` |
| `Self` in signatures (Ruxen addition) | `def <=>(other: &Self) -> Int` | [implemented] |
| `layout c` (FFI, Ruxen addition) | first line of the class body | [implemented] |

The Ruby form; `initialize` and `attr_reader` are [planned](../../../TODO.md#t-initialize):

```ruby
class Account
  balance: Int
  attr_reader :balance

  def initialize(@balance: Int)
  end

  def var deposit(amount: Int)
    @balance += amount
  end

  private

  def audit = puts "balance is #{@balance}"
end
```

(`def var` marks a method that changes the receiver; it is part of the
ownership surface, section 15.)

---

## 10. Value types: `Struct.new` and `Data.define`

Small values without identity are Ruby `Struct` (mutable) and `Data`
(immutable). Both are [planned](../../../TODO.md#t-data-define). The typed spelling below is the proposed
form (OQ-1); the owner decision is "value types are `Struct.new` /
`Data.define`".

Proposed form [planned](../../../TODO.md#t-data-define):

```ruby
Point = Data.define(x: Int, y: Int)          # immutable value
Point = Data.define(x: Int, y: Int) do       # with methods
  def +(o: Point) -> Point = Point.new(x: x + o.x, y: y + o.y)
end

Pixel = Struct.new(x: Int, y: Int)           # mutable value

p = Point.new(x: 1, y: 2)                    # or Point.new(1, 2)
q = p.with(y: 5)
p.x                                          # reader
```

| Construct | Ruby form | Status |
|---|---|---|
| Immutable value type | `Point = Data.define(x: Int, y: Int)` | [planned](../../../TODO.md#t-data-define) |
| Mutable value type | `Pixel = Struct.new(x: Int, y: Int)` | [planned](../../../TODO.md#t-struct-new) |
| Copy semantics | a `Data` value whose fields are all Copy is Copy | [planned](../../../TODO.md#t-data-define); today Copy requires `struct` (E0603) |
| `struct Name ... end` | | [deprecated], replace with `Data.define` or `Struct.new`. Without `layout c` it fails typeck today [known issue](../../../TODO.md#t-struct-typeck) |
| `newtype UserId(Int)` | | [deprecated], replace with `UserId = Data.define(value: Int)` |
| Tuples `(a, b)`, tuple types `(Int, String)`, `.0` access | | [deprecated], replace with arrays `[a, b]`, the tuple type `[Int, String]` (RBS style) [planned](../../../TODO.md#t-tuple-type), indexing `pair[0]` and `a, b = pair` |
| Struct literal `Point { x: 1, y: 2 }` | | [retired]: never implemented (no AST node), remove from docs |
| `layout c` / `layout packed` / `layout transparent` | in a `struct` body | [implemented] for FFI (`layout packed` and `layout transparent` hit the `struct` typeck failure above, [known issue](../../../TODO.md#t-struct-typeck)); moves to `Data.define` bodies with `Data` (OQ-9) |

---

## 11. Enums: `Data` variants and symbols

Symbols are [planned](../../../TODO.md#t-symbols) and `Data` variants are [planned](../../../TODO.md#t-data-define). Pick symbols for plain tags and `Data` variants
when a case carries values. Match either one with `case/in`.

Proposed form [planned](../../../TODO.md#t-data-define):

```ruby
# Plain tags: symbols
status = :active
case status
in :active then start
in :paused | :stopped then idle
end

# Cases with data: one Data type per variant, grouped in a module
module Shape
  Circle = Data.define(radius: Float)
  Square = Data.define(side: Float)
end

def area(s: Shape) -> Float
  case s
  in Shape::Circle(radius:) then 3.14159 * radius * radius
  in Shape::Square(side:) then side * side
  end
end
```

| Construct | Status |
|---|---|
| Symbol tags matched with `case/in` | [planned](../../../TODO.md#t-symbols) (needs symbols, section 3) |
| `Data` variants grouped in a module and used as one type (`s: Shape`) with exhaustive `case/in` | [planned](../../../TODO.md#t-oq) (OQ-1: how the closed set of variants is declared) |
| `enum Color ... end`, payload variants `Circle(radius: Float)`, `Color.Red` | [deprecated], replace with the forms above |

---

## 12. Modules and mixins

| Construct | Ruby form | Status |
|---|---|---|
| Namespace | `module Http ... end`; refer to members as `Http::Request` | [implemented] |
| Mixin module | `module Greetable ... end`, then `include Greetable` in a class | [planned](../../../TODO.md#t-module-mixins); today the mixin is declared with `mixin Greetable` |
| `mixin Name ... end` | | [deprecated], replace with `module Name ... end` |
| Required method (Ruxen addition) | a signature with no body inside the module: `def name -> String` | [implemented] in `mixin` bodies; carries over to `module` |
| Default method | a full `def` inside the module | [implemented] in `mixin` bodies as `def ... end`, calling another method of the mixin as `self.name`; an endless `def` there, or a bare `name` call, does not compile yet [planned](../../../TODO.md#t-implicit-self) |
| `include` | `include Comparable` | [implemented] |
| Several modules at once | `include A, B` | [planned](../../../TODO.md#t-include-many); today one `include` per module |
| `extend` (class-level methods from a module) | `extend Forwardable` | [planned](../../../TODO.md#t-extend) |
| Adding methods to an existing class | reopen it: `class Array ... end` | [planned](../../../TODO.md#t-reopen-class) |
| `extension Type ... end` | | [deprecated], replace with reopening the class |
| Methods that exist only when a type argument meets a bound | reopen with a bound: `class Box[T < Comparable] ... end` | [planned](../../../TODO.md#t-oq) (OQ-6) |
| `extension C[T] where T: B` | | [deprecated], replace with the line above |
| Mixin inheritance `mixin A: B` | | [deprecated], replace with `include B` inside `module A` |
| Two included modules defining the same method | Ruby's rule: the module included last wins; the class's own method beats both | [planned](../../../TODO.md#t-oq); the 2026-05 draft made this an error (OQ-7) |
| `super` from an override | reaches the superclass method first, then the module default (Ruby's lookup order) | [planned](../../../TODO.md#t-super); `super` outside `init` is E1127 today |

### 12.1 The core modules

| Module | You provide | You get | Status |
|---|---|---|---|
| `Comparable` | `def <=>(other: &Self) -> Int` | `<`, `<=`, `==`, `>`, `>=`, `between?`, `clamp` | [implemented] (`std/core`, declared with `mixin` today) |
| `Enumerable` | `def each(f: any Fn[Fn(T) -> nil]) -> nil` today; `def each(&block: Fn[(T) -> nil])` is the target | `map`, `select`, `reverse_each`, `entries` and the rest of the combinator family defined once in `std/core` | [implemented] as `include Enumerable[T]`; plain `include Enumerable` with `T` inferred from `each` is [planned](../../../TODO.md#t-enumerable-infer) |
| `Kernel` | nothing (included everywhere) | `tap`, `then`, `yield_self`, plus the free functions `puts`, `print`, `p`, `warn`, `abort`, `exit`, `sleep` | [implemented] (`p` takes any value that includes `Inspect`) |

The Ruby form; `initialize`, Array `<=>` and the bare `major` call are [planned](../../../TODO.md#t-initialize):

```ruby
class Version
  include Comparable

  major: Int
  minor: Int

  def initialize(@major: Int, @minor: Int)
  end

  def <=>(other: &Version) -> Int
    [major, minor] <=> [other.major, other.minor]
  end
end

Version.new(1, 2) < Version.new(1, 3)   # => true
```

(Array `<=>` and implicit-self calls such as bare `major` in this example
are target forms; check them before relying on the example as a fixture. A
bare `major` needs [implicit-self calls](../../../TODO.md#t-implicit-self)
plus `attr_reader :major`; until then it is E0734 and the method reads
`@major`.)

---

## 13. The object protocol

Ruby's method names replace the Rust trait vocabulary.

| Purpose | Ruby form | Status | Replaces ([deprecated]) |
|---|---|---|---|
| Human text | `def to_s -> String`; used by `puts` and `"#{x}"` | [implemented]: a user `to_s` wins; every value has a default. Interpolation (`"#{x}"`) still prints the default for a user class [known issue](../../../TODO.md#t-interp-to-s) | `Display`, `Displayable`, `to_display`, `to_string` |
| Debug text | `def inspect -> String`; used by `p x` | [planned](../../../TODO.md#t-d9) | `Debug`, `to_debug`, `#{x:?}` |
| Default rendering | `#<data Point x=3, y=4>`, `#<struct Pixel x=1, y=2>`, `#<Account @balance=10>` | [planned](../../../TODO.md#t-d9); today the default prints the Rust shape `Point { x: 3, y: 4 }` | Rust `Debug` output |
| Equality | `def ==(other: &Self) -> Bool` | [implemented]. A generic bound that needs only equality becomes a check that the type has a `.rx` `==`, with no marker mixin [planned](../../../TODO.md#t-x1); value types get a structural `==` [planned](../../../TODO.md#t-d8) | `PartialEq`, `Eq`, `eq` |
| Hash-key identity | `def eql?(other) -> Bool` and `def hash -> Int` | [planned](../../../TODO.md#t-d8) for user types (`String#eql?` exists): hash keys use `hash` and `eql?`, `eql?` defaults to `==`, and `hash_code` becomes a [deprecated] alias of `hash` | `Hashable`, the `Hash` mixin |
| Ordering | `include Comparable` and `def <=>` | [implemented] | `Ord`, `PartialOrd`, `cmp`, `partial_cmp` |
| Arithmetic | `def +(o)`, `def -(o)`, `def *(o)`, ... | [implemented] | `Add` and the other operator mixins |
| Conversion | `to_s`, `to_i`, `to_f`, `to_a`, `to_h`, `to_set` | `to_s`, `to_i`, `to_f` [implemented]; the collection conversions per the stdlib specs | `Into[T]`, `From`, `from_iter`, `collect` |
| Copying a value | `dup` (shallow) and `clone` | `clone` [implemented]; `dup` [implemented] on `String` only | (`Clone` stays as the ownership marker, section 15) |
| Default value | arguments with defaults, or `initialize` with no parameters | arguments with defaults [implemented]; `def init` with no parameters [deprecated]; `initialize` [planned](../../../TODO.md#t-initialize) | `Default`, `default` |

Scalars and `String` will include `Comparable`, `Hashable` and `Clone` (and
`Default` where Ruby has a zero), with `==`, `<=>`, `to_s` and `inspect` in
`.rx`; the Rust-named mixins stay as [deprecated] `.rx` adapters [planned](../../../TODO.md#t-d6).

The marker modules `Copy`, `Clone`, `Drop`, `Send` and `Sync` are not on
this list; they belong to ownership (section 15).

---

## 14. Static typing (the Ruxen addition to Ruby)

Ruby has no type annotations. Ruxen adds the smallest set it needs; every
other type is inferred.

| Construct | Ruxen form | Status |
|---|---|---|
| Parameter and return annotations | `def f(x: Int) -> String` | [implemented] |
| Field (ivar) types | `name: String` in a class body | [implemented] |
| Typed local | `x: Int = 0` | [planned](../../../TODO.md#t-typed-local) (section 2) |
| Optional | `T?` | [implemented] |
| Generic class or method | `class Stack[T]`, `def first[T](xs: Array[T]) -> T?` | [implemented] |
| Bound on a type parameter (RBS style) | `[T < Comparable]` | [planned](../../../TODO.md#t-d7-respell) |
| Several bounds | `[T < Comparable & Hashable]` | [planned](../../../TODO.md#t-d7-respell) |
| `[T: Bound]`, `T: A + B` | | [deprecated], replace with `[T < Bound]`, `[T < A & B]` |
| `where` clauses | `where T: Display` | [deprecated], replace with bounds in the `[...]` list [planned](../../../TODO.md#t-d7-respell). Until then a method `where` bound (`def sum -> T where T: Add`) is the form that works for a method that needs a bound on its class's type parameter; OQ-6 stays open |
| Tuple type | `[Int, String]` (RBS) | [planned](../../../TODO.md#t-tuple-type); `(Int, String)` is [deprecated] |
| Type alias | `Id = Int` | [planned](../../../TODO.md#t-type-alias); `type Id = Int` is [deprecated] (OQ-10 for generic aliases) |
| Bottom type | `Never` (for `abort`, `exit`, `raise`) | [implemented] |
| Existentials | `some Mixin`, `any Mixin`, `Box[any Mixin]` | [implemented] (OQ-11) as parameter types; `some Mixin` as a return type is rejected today [planned](../../../TODO.md#t-some-return) |
| Associated types | `type Item` in a mixin, `type Item = Int` in the class, `Iterator[Item = Int]` | `type Item` and `type Item = Int` [implemented] (OQ-11); a binding in a type argument (`Container[Item = Int]`) does not parse, [planned](../../../TODO.md#t-oq) (OQ-11) |
| Fixed arrays and const generics | `[T; N]`, `[T, const N: USize]` | [implemented] (OQ-8) |
| Numeric types | `Int` and `Float` everyday; sized types (`Int8` ... `UInt64`, `USize`, `Float32`) | [implemented]; sized types for FFI and storage only (OQ-8) |
| Numeric conversion | `x.to_i`, `x.to_f` | [implemented] |
| Sized conversion | `x.to_i8`, `x.to_u8`, `x.to_i32`, ... | [planned](../../../TODO.md#t-sized-conv) |
| `as` cast | `n as UInt8` | [deprecated], replace with `n.to_u8` |
| `size` / `length` result type | `Int` | [planned](../../../TODO.md#t-size-int); today `USize`, which forces about 67 `as Int` casts in the stdlib |
| `str`, `&str` | | [retired] (E0730): use `String` / `&String` |
| HRTB `for[a]`, GATs, specialization | | [retired]: never implemented, remove from docs |

---

## 15. Kept from Rust: ownership

This is the entire Rust surface Ruxen keeps. Each item is spelled to read
like Ruby.

| # | Feature | Ruxen spelling | Status |
|---|---|---|---|
| 1 | Borrows | `&x` (read), `&var x` (write); types `&T`, `&var T`. `&mut` is never used. | [implemented]. Auto-borrowing at call sites (`f("x")` for a `&String` parameter) is OQ-13. |
| 2 | Receiver modes | `def m` reads `self`; `def var m` may change `self`; `def consume m` takes ownership of `self` | [implemented] |
| 3 | Mutability | Ruby locals are always reassignable, so `var` survives only in `&var`, `*var` and `def var` | target; `var x = ...` as a binding is [deprecated] (section 2) |
| 4 | `move` closures | `move { \|x\| x + n }`, `move do ... end`: every capture is owned (needed when a block outlives its scope, such as a thread body) | [implemented] |
| 5 | Lifetimes | rare escape hatch: lowercase names in the generic list and after `&`: `def longest[a](x: &a String, y: &a String) -> &a String`. Elision is the default. | [implemented]. The `'a` sigil is [retired] (it lexes as an unterminated string, E0002). |
| 6 | `Copy`, `Clone`, `Drop` | values of Copy types are copied; `clone` (Ruby twin `dup`) makes an explicit copy; `def drop` is the destructor (see `docs/decisions/drop-elaboration.md`) | [implemented]. `ensure` and block-scoped resources (`File.open(path) do \|f\| ... end`) lowering onto drop is [planned](../../../TODO.md#t-exceptions). |
| 7 | `Send`, `Sync` | thread-safety markers; opt out with `include !Send`, opt in for a hand-checked type with `unsafe include Send` | both spellings parse [implemented]. OQ-14: pick one opt-out spelling (`exclude Send` is proposed, not implemented) and settle whether the markers are inferred or written by hand (current docs disagree). |
| 8 | Owning pointers | `Box[T]` (single owner, heap), `SharedSync[T]` (atomic reference count, thread-safe) | `Box[T]` as a type and `SharedSync[T]` [implemented]; `Box.new(x)` is not defined yet [planned](../../../TODO.md#t-box-new). `Shared[T]` (single-threaded reference count) is [planned](../../../TODO.md#t-shared): the stdlib does not define it. Names: OQ-15. |
| 9 | Binding modes in patterns | `ref name`, `ref var name` | [implemented] (OQ-16) |

Raw pointers `*T` and `*var T` exist only for FFI (section 16).

Example [implemented]. Its constructor is still spelled `def init` [deprecated] until `initialize` lands [planned](../../../TODO.md#t-initialize):

```ruby
def longest[a](x: &a String, y: &a String) -> &a String
  x.size > y.size ? x : y
end

class Person
  name: String

  def init(@name: String)
  end

  def var rename(name: String)    # may change self
    @name = name
  end

  def consume into_name -> String  # takes ownership of self
    @name.clone
  end
end

total = 0
add = move { |x: Int| x + total }   # captures are owned
```

---

## 16. FFI

FFI stays as Ruxen's own `lib` block, which reads like Ruby FFI's
`attach_function`.

| Construct | Form | Status |
|---|---|---|
| Library block | `lib "m" ... end`, `lib "ssl", version: "3" ... end` | [implemented] (the trailing options are parsed and ignored today) |
| Renamed C symbol | `def open as "ruxen_file_open"(path: &String) -> Int` | [implemented]. This `as` is the C symbol name, not a cast; it stays. |
| Receiver mode of a C method | `def var push as "ruxen_vec_push"(v: T)`, `def consume drop as "ruxen_mutex_free"` | [implemented]. Same spellings as a Ruby-side `def var` / `def consume`; a `def consume drop` frees the handle's storage itself. |
| `def var` on a free `lib` function (outside a class) | | an error: mutation needs an explicit `&var` parameter [planned](../../../TODO.md#t-x5). Today the mode is silently ignored. |
| Owned C result | `def clone as "ruxen_closure_share" -> owned Self` | [implemented]. The caller owns the result outright (for a collection: its spine, buffer and every element) and frees it. Without `owned`, a C result is not assumed owned. |
| Library block inside a class | `class File; lib "ruxen_runtime" ... end; end` | [implemented] |
| Variadic C function | `def printf(fmt: *UInt8, ...) -> Int32` | [implemented] |
| Raw pointers | `*UInt8`, `*var UInt8`; null is `nil` | [implemented]; `*Void` is rejected as an undefined type [planned](../../../TODO.md#t-void-ptr) |
| `unsafe ... end` blocks | | [implemented] (OQ-9) |
| `extern "C"`, `@[link(...)]`, `@[repr(C)]` | | [retired] (E0607 for `@[...]`) |

---

## 17. Concurrency and async

| Construct | Form | Status |
|---|---|---|
| `async def`, `expr.await` | | [implemented]. Kept for now: it is the working model today. |
| Fiber scheduler | `Async do ... end`, `task.wait` (Ruby's `Fiber.scheduler` model) | [planned](../../../TODO.md#t-fibers). When it lands, `async def` / `.await` become [deprecated]. |
| `Future`, `Poll`, `Waker`, `block_on` in user code | | to be [deprecated] together with `async def` |
| Threads | `Thread.new { ... }.value`, `Mutex#synchronize`, `Thread::Queue`, `Thread.current[:key]` | [planned](../../../TODO.md#t-thread-api) (stdlib) |
| `JoinHandle`, `MutexGuard`, `lock() -> Result`, atomics with `Ordering`, mpsc channels, `thread_local!` | | [deprecated] as the user-facing shape, replaced by the line above |

---

## 18. Standard library naming

1. **Ruby names are canonical.** Use the Ruby class and method name
   wherever Ruby has one: `Array`, `Hash`, `Set`, `String`, `Integer`
   methods on `Int`, `Range`, `Regexp` methods on `Regex`, `File`, `Dir`,
   `ENV`, `ARGV`, `Time`, `Thread`, `Mutex`.
2. **Rust-named twins are deprecated aliases, never deleted.** They keep
   working and the deprecation warning names the Ruby method.
3. **No iterator layer.** Collections are `Enumerable`. There is no
   `iter`, `into_iter`, `iter_mut`, `collect`, `from_iter` or `Iterator`
   (the dead `mixin Iterable` in `std/core` goes too).
4. Per-module status lives in the stdlib specs (`docs/specs/stdlib/`), not
   here.

| Rust name ([deprecated] alias) | Ruby name |
|---|---|
| `len`, `is_empty` | `size` / `length`, `empty?` |
| `to_string` | `to_s` |
| `push_str` | `<<` |
| `to_upper`, `to_lower` | `upcase`, `downcase` |
| `starts_with`, `ends_with`, `contains` | `start_with?`, `end_with?`, `include?` |
| `trim` | `strip` |
| `with_capacity` | `Array.new` (capacity is an implementation detail) |
| `get`, `insert`, `get_mut` | `[]`, `[]=`, `fetch` |
| `filter`, `fold`, `position`, `retain`, `all`, `any`, `skip`, `enumerate` | `select`, `reduce`, `index`, `select!`, `all?`, `any?`, `drop`, `each_with_index` |
| `fs.read_to_string(path)` | `File.read(path)` |
| `path.join(a, b)` | `File.join(a, b)` |
| `env.args`, `env.var` | `ARGV`, `ENV[...]` |
| `process.command` | `system`, backticks |
| `Instant.now`, `Duration.from_millis` | `Time.now`, numeric seconds (`sleep 0.5`) |
| `Map[K, V]`, `HashMap`, `HashSet`, `Vec`, `Rc`, `Arc` | `Hash[K, V]`, `Hash`, `Set`, `Array`, `Shared`, `SharedSync` |

---

## 19. Reserved words

**Target keyword set:** Ruby's keywords (`alias and begin break case class
def defined? do else elsif end ensure false for if in module next nil not
or redo rescue retry return self super then true undef unless until when
while yield`) plus these Ruxen additions:

| Word | Why |
|---|---|
| `var`, `consume`, `move`, `ref` | ownership (section 15) |
| `lib`, `unsafe` | FFI (section 16; `unsafe` is OQ-9) |
| `layout`, `inline` | FFI layout and the inline hint |
| `Self` | static typing |
| `some`, `any` | existential types (OQ-11) |
| `async`, `await` | until the Fiber scheduler replaces them (section 17) |

**Keywords to free** (all [deprecated] or [retired] above): `let`,
`match`, `struct`, `enum`, `newtype`, `type`, `mixin`, `extension`, `use`,
`package`, `const`, `static`, `macro`, `where`, `as` (except in `lib`
blocks), `continue`, `init`, `Some`, `Ok`, `Err`, and `loop` (which becomes
a `Kernel` method). Freeing `match`, `where`, `any`, `type` and `use` lets
Ruby method names such as `String#match` and `Relation#where` work.
`macro`, `static` and `package` are reserved today but parse nothing.

`include`, `extend`, `private`, `protected`, `public`, `raise`, `require`,
`attr_reader` and `loop` are methods in Ruby. Ruxen may treat some of them
as keywords internally, but users must be able to read them as Ruby calls.

---

## 20. Open questions

| # | Question | Default until decided |
|---|---|---|
| OQ-1 | Typed spelling of `Struct.new` / `Data.define` (`Data.define(x: Int, y: Int)` proposed), and how a module declares the closed set of `Data` variants for exhaustive `case/in` | the proposal in sections 10 and 11 |
| OQ-2 | Can a method declare its raise set explicitly, and how? | inferred only |
| OQ-3 | Ivar type declarations: keep `name: Type` in the class body, or the RBS form `@name: Type`? | `name: Type` |
| OQ-4 | Keyword parameters in definitions: Ruby's `def f(host:, port: 80)` collides with `name: Type` annotations | Decided 2026-11-03 (Crystal's rule), [planned](../../../TODO.md#t-kwargs): definitions keep `name: Type`; any parameter may be passed by name (`connect(host: "x", port: 80)`, any order, or mixed `connect("x", port: 8080)` with positional first); a bare `*` makes the following parameters keyword-only (`def connect(*, host: String, port: Int = 80)`, so `connect("x")` is an error); the automatic constructor is a generated `init` over the fields and takes both forms; a keyword-only class writes `def init(*, x: Int, y: Int)` by hand, no class-level marker yet |
| OQ-5 | Block and proc types | Decided 2026-10-17: `Fn[(A) -> R]` is a borrowed block, `Proc[(A) -> R]` a stored proc or lambda. `FnMut`/`FnOnce` capability inference stays open |
| OQ-6 | Conditional methods via a bounded reopen (`class Box[T < Comparable]`): exact rule | as in section 12 |
| OQ-7 | Two included modules with the same default method: Ruby's "last include wins", or keep an error? | Ruby's rule is the target |
| OQ-8 | Sized integers: FFI and storage only? Literal suffixes (`200u8`) kept or replaced by `200.to_u8`? `[T; N]` and const generics kept (perhaps as `StaticArray[T, N]`) or dropped? | unchanged |
| OQ-9 | `unsafe ... end`: keep, or limit raw pointers to `lib` blocks? `layout c` versus `layout :c` | unchanged |
| OQ-10 | Generic type aliases (`type Pair[T] = ...` in RBS) | `X = Y` for plain aliases |
| OQ-11 | `some` / `any` existentials and associated types: keep, or is a plain module-typed parameter enough? (Associated types exist mainly for the retired iterator layer and `Future::Output`.) | unchanged |
| OQ-12 | `#= ... =#` block comments once `=begin ... =end` exists | keep both |
| OQ-13 | Auto-borrow at call sites so `f("x")` passes `&"x"` | Decided 2026-10-17: auto-borrow; callers never write `&` |
| OQ-14 | One opt-out spelling for `Send` / `Sync` (`exclude Send` proposed) | `include !Send` |
| OQ-15 | Names of the ownership types (`Box`, `Shared`, `SharedSync`; `Send` / `Sync`, for example `Shareable` after Ractor) | unchanged |
| OQ-16 | `ref` / `ref var` patterns, or default binding modes | unchanged |

---

## 21. Migration table: Rust form to Ruby form

Item numbers follow the 2026-10 Rust-ism audit inventory. "Status" is the
status of the Ruby replacement.

| # | Rust form | Ruby form | Status of Ruby form | Detect |
|---|---|---|---|---|
| 1 | `let x = 1`, `var x = 1` | `x = 1`; constants `MAX = 3` | `x = 1` [implemented]; constants [planned](../../../TODO.md#t-constants) | `^\s*(let\|var)\s+[\w(]` |
| 2 | `Option[T]`, `Some(x)` | `T?`, `nil`, the bare value | [implemented] | `\bSome\(\|\bOption\[` |
| 3 | `Result[T, E]`, `Ok`, `Err`, "no exceptions" | `raise`, `begin/rescue/ensure`, `retry` | [planned](../../../TODO.md#t-exceptions) | `\bResult\[\|\bOk\(\|\bErr\(` |
| 4 | postfix `?` | exceptions propagate on their own | [planned](../../../TODO.md#t-exceptions) (with item 3) | `\)\?(?=[\s.;)]\|$)` |
| 5 | `?.` taught as safe navigation | `&.` | [implemented] | `\)\?\.\w` |
| 6 | `match ... pat -> expr ... end` | `case ... in ... then ... end` | [implemented] | `^\s*match\s+\S` |
| 7 | `if let` / `while let` | `if (x = opt)`, `while (x = q.pop)`, `case/in` | `case/in` [implemented]; assignment-in-condition narrowing [planned](../../../TODO.md#t-narrowing) | `\b(if\|elsif\|while)\s+let\b` |
| 8 | `panic!` and the `!` macro convention | `raise "msg"`, `abort "msg"` | `abort` [implemented]; `raise` [planned](../../../TODO.md#t-exceptions) | `\bpanic!` |
| 9 | `unwrap!`, `expect!`, `unwrap_or`, `ok_or`, `map_err`, `and_then` | `x \|\| default`, `fetch`, `x or raise` | [planned](../../../TODO.md#t-truthiness) | `\.(unwrap\|expect!\|unwrap_or\w*\|ok_or\|map_err\|and_then)(?!\w)` |
| 10 | `struct`, `Point { x: 1 }` literals | `class`, `Struct.new`, `Data.define`, `Point.new(x: 1)` | `class` [implemented]; `Struct.new` [planned](../../../TODO.md#t-struct-new); `Data.define` and `Point.new(x: 1)` [planned](../../../TODO.md#t-data-define) | `^\s*struct\s+[A-Z]\|\b[A-Z]\w*\s*\{\s*\w+:\s` |
| 11 | `enum` with payload variants | `Data.define` per variant, symbols, `case/in` | [planned](../../../TODO.md#t-data-define) | `^\s*enum\s+[A-Z]` |
| 12 | `newtype`, tuples with `.0`, `type X =` | one-field `Data`, arrays and `a, b = pair`, `X = Y` | one-field `Data` [planned](../../../TODO.md#t-data-define); arrays for tuples [planned](../../../TODO.md#t-tuple-type); `X = Y` [planned](../../../TODO.md#t-type-alias) | `^\s*newtype\b\|(?<=[a-z_)\]])\.\d+\b\|^type\s+[A-Z]\w*(\[[^\]]*\])?\s*=` |
| 13 | unit `()`, `Result[(), E]` | `nil` | [implemented] (`()` is [retired]) | `->\s*\(\)\s*$\|=\s*\(\)\s*$\|\[\(\)\s*,` |
| 14 | `[T; N]`, const generics, `const NAME =` | `MAX = 3`; maybe `StaticArray[T, N]` | `MAX = 3` [planned](../../../TODO.md#t-constants); `StaticArray` (OQ-8) [planned](../../../TODO.md#t-oq) | `^\s*const\s+[A-Z]` |
| 15 | `Display`, `Debug`, `PartialEq`, `Eq`, `Ord`, `PartialOrd`, `Default`, `Into`, `Add`, `Hashable` | `to_s`, `inspect`, `==`, `eql?` + `hash`, `include Comparable` + `<=>`, defaults, `to_*`, `def +` | `to_s`, `==`, `include Comparable` + `<=>`, `def +`, `to_i`/`to_f` and arguments with defaults [implemented]; `inspect` [planned](../../../TODO.md#t-d9); `eql?` + `hash` [planned](../../../TODO.md#t-d8) | `\binclude\s+(Display\|Displayable\|Debug\|PartialEq\|Eq\|Ord\|PartialOrd\|Into\|Add)\b\|\.(to_display\|to_debug\|partial_cmp)\b` |
| 16 | `derive`, `@[attr]`, `#[attr]` | implicit includes, body directives | [implemented] (the attribute forms are [retired], E0607) | `^\s*(derive\b\|@\[\|#\[)` |
| 17 | `iter`, `into_iter`, `collect`, `Iterator`, `FromIterator`, `type Item` | `Enumerable` (`each`, `map`, `to_a`) | [implemented] | `\.(iter\|into_iter\|iter_mut\|collect)\b\|\b(FromIterator\|Iterator)\b` |
| 18 | `len`, `is_empty`, `to_string`, `push_str`, `to_upper`, `starts_with`, `trim`, `with_capacity`, `get_mut`, `insert`/`get` | `size`, `empty?`, `to_s`, `<<`, `upcase`, `start_with?`, `strip`, `[]`, `[]=` | mostly [implemented]; Rust names become [deprecated] aliases. `<<` on a `String` does not append today (use `push_str`) [known issue](../../../TODO.md#t-string-shovel) | `\.(len\|is_empty\|to_string\|push_str\|to_upper\|to_lower\|starts_with\|ends_with\|contains\|trim\|with_capacity\|get_mut)\b` |
| 19 | Rust `std::io/fs/env/process/path/time/net` shapes | `File.read`, `File.join`, `ARGV`, `ENV`, `system`, `Time.now`, `TCPServer` | [planned](../../../TODO.md#t-stdlib-shapes) (stdlib) | `\bfs\.\w+\|\benv\.(args\|var)\b\|\bprocess\.command\b\|\bInstant\.now\b\|\bDuration\.from_\w+` |
| 20 | `JoinHandle`, `MutexGuard`, `lock() -> Result`, atomics with `Ordering`, mpsc, `thread_local!` | `Thread.new { }.value`, `Mutex#synchronize`, `Thread::Queue`, `Thread.current[:x]` | [planned](../../../TODO.md#t-thread-api) (stdlib) | `\b(JoinHandle\|MutexGuard\|mpsc)\b\|\bthread_local!\|\bOrdering::` |
| 21 | `println!`, `format!`, `dbg!`, `assert!` with `{}` | `puts`, `p`, `format("%d", n)` | `puts` and `p` [implemented]; `format` [planned](../../../TODO.md#t-d9) | `\b(println\|eprintln\|format\|dbg\|assert\|assert_eq\|assert_ne\|write\|writeln\|vec)!` |
| 22 | `#{x:?}`, `#{n:>5}`, Debug output `Foo { x: 1 }` | `inspect`, `rjust`, `format`, `#<Foo x=1>` | `rjust` [implemented]; the rest [planned](../../../TODO.md#t-d9) | `#\{[^}]*:[?<>^#.0-9]` |
| 23 | `&str`, `str`, `&static str` | `String`, `&String` | [implemented] (`str` is [retired], E0730) | `&('static\s+\|static\s+)?str\b\|[:>]\s*str\b` |
| 24 | `Fn(A) -> R`, `FnMut`, `FnOnce`, `FnVar`, `{ \|\| }`, bare `\|x\| e` | `&block` with `yield`, `Fn[(A) -> R]` (or `Proc`), `->(x) { }` | `&block`/`yield`/`Fn[...]` and lambdas `->(x) { }` [implemented] | `\bFn(Var)?\(\|\{\s*[\|][\|]` |
| 25 | `(&self)`, `(self, ...)`, `self: &Self` receivers | `def m`, `def var m`, `def consume m` | [implemented] | `\bdef\s+\S+?\s*\(\s*&?(var\s+\|mut\s+)?self\b\|\bself:\s*&` |
| 26 | `use a.b.{C, D}` | `require`, `require_relative` | [implemented] | `^\s*use\s+[\w.:]+\|\bpackage\.\w` |
| 27 | `as` numeric casts | `to_i`, `to_f`, `to_i8`-style methods | `to_i`/`to_f` [implemented]; sized [planned](../../../TODO.md#t-sized-conv) | `\bas\s+(U?Int(8\|16\|32\|64)?\|USize\|Float(32\|64)?\|Char\|Bool)\b` |
| 28 | `USize`, `UInt8`, ... everywhere; `255u8` suffixes | `Int`/`Float`; sized types for FFI only | [planned](../../../TODO.md#t-oq) (OQ-8) | editorial |
| 29 | `some`/`any`, `[T: B]`, `where`, associated types, GAT, HRTB, specialization | `[T < B]`; no GAT, HRTB or specialization | `[T < B]` [planned](../../../TODO.md#t-d7-respell); `some`/`any` and associated types (OQ-11) [planned](../../../TODO.md#t-oq) | `\[\s*[A-Z]\w*\s*:\s*[A-Z]\|\bwhere\s+[A-Z]\w*\s*:\|\bfor\[` |
| 30 | `async def`, `.await`, `Future`, `Poll`, `Waker`, `block_on` | Fiber scheduler, `Async do ... end`, `task.wait` | [planned](../../../TODO.md#t-fibers) | editorial |
| 31 | `unsafe` blocks and modifiers | `lib` FFI only | [planned](../../../TODO.md#t-oq) (OQ-9) | editorial |
| 32 | `loop ... end`, `continue`, `next` only in blocks | `loop do ... end`, `next` in every loop | `loop do` [planned](../../../TODO.md#t-loop-do); `next` in every loop [planned](../../../TODO.md#t-next-loops) | `^\s*loop\s*(#.*)?$\|^\s*continue\b` |
| 33 | `def f -> T { e }` | `def f = e` | [implemented] | `^\s*def\s[^=#]*\{[^}]*\}\s*$` |
| 34 | `mixin`, `extension`; reopening a class is an error | `module` with `include`; reopening a `class` | `module` [planned](../../../TODO.md#t-module-mixins); reopening [planned](../../../TODO.md#t-reopen-class) | `^\s*(mixin\|extension)\s+[A-Z]` |
| 35 | `init`, `self.field`, no `attr_*` | `initialize`, `@x`, `attr_reader` | `@x` [implemented]; `initialize` [planned](../../../TODO.md#t-initialize); `attr_*` [planned](../../../TODO.md#t-attr-accessors) | `\bdef\s+init\b\|\bself\.[a-z_]\w*\s*=(?!=)` |
| 36 | explicit `def main` | top-level statements | [implemented] | `^\s*def\s+main\b` |
| 37 | keywords that block Ruby method names (`match`, `where`, `any`, `loop`, `type`, `use`, `static`, `macro`, `package`); `~=` | free them; `=~` | freed keywords [planned](../../../TODO.md#t-free-keywords); `=~` [planned](../../../TODO.md#t-match-op) | `\s~=\s` |
| 38 | `fn`, `pub`, `impl`, `mod`, `::<>`, `Box<dyn>`, `Lazy<T>`, `Vec`, `Arc` in examples | `def`, `include`, `Array`, `SharedSync` | Ruby forms [implemented]; Rust forms never parsed (docs only) | `^\s*(pub\s+)?fn\s+\w+\s*[(<]\|\bimpl\s*[<A-Z]\|^\s*(pub\s+)?mod\s+\w\|::<\|\bBox<\|\bdyn\s+[A-Z]\|\bLazy<\|\bVec[<\[]\|\bArc[<\[]\|^\s*pub\s` |
| 39 | `Map[K, V]` | `Hash[K, V]` | [implemented] | `\bMap\[` |
| 40 | `None`, `r"..."`, `'\r'`, `'\u{...}'`, `..=` | `nil`, `'...'`, `?\r`, `"\u{...}"`, `..` | [implemented] (Rust forms [retired]) | `\bNone\b\|\br\"\|\.\.=\|'\\[a-z]'\|'\\u\{` |
| 41 | editions, `no_std`, `cfg(feature)`, stability attributes, `r#ident`, `macro_rules!` | semver, Bundler-style groups, deprecation warnings | not language syntax; remove from language docs (`no_std` lives in `docs/decisions/phase4-no-std-wasm.md`) | editorial |
| 42 | Rust prose and compiler internals in user docs ("matches Rust's ...", `Ty::Infer`, `crates/` paths) and Rust wording in `##` comments | Ruby and Ruxen wording | docs rule | editorial |
| 43 | dead plumbing: `TokenKind::Lifetime` / `Mut` / `AmpMut`, empty `derive_traits`, unparsed `macro`/`static`/`package` keywords, formatter printing `'a` | delete | compiler cleanup | editorial |

---

## 22. Conformance

- Every [implemented] form has at least one positive e2e fixture in
  `tests/release-e2e/cases/`, and parses identically in the compiler, REPL,
  LSP, IDE and formatter (`compiler/ruxen_core/tests/parser_feature_parity.rs`).
- A `[planned]` form moves to `[implemented]` only with a positive fixture, a
  negative unit test asserting its error code, and an update to this file.
- A [deprecated] form gets a registered warning code with a `docs/errors/`
  page and a fix-it that produces the Ruby form.
- Diagnostic code ranges come from `compiler/ruxen_core/src/diagnostics/codes.rs`.
  The old draft's placeholder names (`E-NIL-AMBIGUOUS`, `E-MIX-MISSING`,
  ...) were never registered and are not used.

---

## Appendix A. Status evidence (2026-10-25)

Every [implemented] claim above was probed on 2026-10-25 on a fresh release
build of `6bf146ac`: `ruxen compile --emit=ast` for syntax, `--emit=hir` for
typing, and compile and run with the output compared for behaviour. The rows
below are the source locations behind the claims. Paths are relative to
`compiler/ruxen_core/src/` unless they start with `library/` or `tests/`.

| Claim | Evidence |
|---|---|
| Keyword set (`let`, `var`, `match`, `loop`, `continue`, `struct`, `enum`, `newtype`, `type`, `mixin`, `extension`, `use`, `package`, `const`, `static`, `macro`, `where`, `as`, `some`, `any`, `init`, `Some`, `Ok`, `Err`, `async`, `await`, `unsafe`); no `begin`, `rescue`, `ensure`, `retry` | `lexer/token.rs`, `lookup_keyword` |
| Top-level statements become `main` | `parser/implicit_main.rs`; `parser/items.rs` (top-level expression arm) |
| `x = 1` declares a local | `resolve/control_flow.rs`, `resolve_bare_assignment_binding` |
| `require` / `require_relative` | `module_loader.rs` (`parse_requirement`) |
| `const` item; top-level `let` is not an item | `parser/items.rs` (`parse_top_level_item`, `parse_const_def`) |
| Parallel assignment and splat target | `parser/methods.rs` (statement parser); `typeck/infer/expr.rs` (Copy-element and Array rules) |
| `@x` read/write; `@param` auto-assign; `&block:` parameter | `parser/expr/atoms.rs` (`TokenKind::At`); `parser/methods.rs` (`parse_param`) |
| A bare field name is E0734 with a field hint, never a field read | `resolve/items.rs` (`resolve_class`: fields are not value-scope names); `resolve/helpers.rs` (`error_undefined_variable`); `tests/it/bare_field_name.rs` |
| Endless and brace-bodied `def` | `parser/methods.rs` (body parsing after the signature) |
| `case/when`, `case/in`, `->` still accepted in `case/in` | `parser/expr/case_expr.rs` |
| `match`, `if let`, `while let`, `loop ... end`, `unless`, `until`, ternary | `parser/expr/match_expr.rs`, `parser/expr/control_flow.rs` |
| `next` only inside blocks; `{}` is an empty block | `parser/expr/atoms.rs` (`closure_depth > 0`; `parse_brace_map_or_closure`) |
| `&.`, postfix `?` try, `.await` | `parser/expr/calls.rs` |
| `??`; `&&`/`\|\|` need `Bool` (E0731) | `parser/expr/mod.rs`; `typeck/infer/ops.rs` |
| `~=` match operator; `%w[...]`; `'...'` raw strings; `?c` chars; `"""` strings; `#= =#` comments | `lexer/token.rs`; `lexer/tokens.rs`; `lexer/strings.rs`; `lexer/mod.rs` |
| No symbol literal node | `parser/ast.rs` (no symbol variant; `:name` handled only in `parse_visibility_name_list` and `try_parse_block_pass`) |
| `T?` desugars to `Option[T]`; automatic wrapping of bare values | `parser/types.rs` (`parse_type`); `typeck/infer/mod.rs` (`auto_wrap_option_some`); `typeck/tests.rs` (B84 tests) |
| Generic params `[T: B]`, `const N`, bare lowercase lifetimes | `parser/types.rs` (`parse_generic_param`) |
| `::` in expressions; type paths use `.` | `parser/expr/atoms.rs`; `parser/types.rs` (`parse_type_path`) |
| Class body accepts fields, `def`, `include` (single, `!`, `unsafe`), `lib`, `layout`, `alias`, `type X =` | `parser/classes.rs` (`parse_class_def`); `parser/methods.rs` (`parse_include_directive`) |
| Built-in macro `panic!`; the collection macros (`vec!` and the rest) are rejected | `parser/macros.rs` (`BUILTIN_MACRO_NAMES`) |
| `unwrap!` / `expect!` intrinsics | `mir/lower/expr/field_access.rs` |
| `raise` only allowed as a paren-less call name | `parser/expr/calls.rs` (`is_bare_call_fn`) |
| `block_given?` | `resolve/exprs.rs` |
| `Comparable` (`<=>`), `Kernel`, `Enumerable[T]` (`each`), dead `Iterable`, `Error`, Rust-named marker mixins, `abort`, `exit!`, `p` overloads | `library/std/core/src/lib.rx` |
| User `include Comparable` + `def <=>`; user `include Enumerable[Int]` + `def each`; user `to_s` wins over the default | `tests/release-e2e/cases/965_comparable_spaceship.rx`, `982_user_class_enumerable_map.rx`, `810_to_s_universal.rx` |
| Default `to_s` prints `Point { x: 3, y: 4 }` | `tests/release-e2e/expected/810_to_s_universal.out`; `mir/lower/expr/method_call.rs` |
| `String#size` returns `USize`; `String#dup`, `rjust`, `eql?` | `library/std/string/src/string.rx` |
| `Box` type; `SharedSync` class; no `Shared` class | `resolve/types.rs`; `library/std/sync/src/shared_sync.rx` |
| Redefining a stdlib class name is E0727 | `docs/errors/E0727.md` |
| `@[...]` attributes rejected with E0607 | `parser/items.rs` |
