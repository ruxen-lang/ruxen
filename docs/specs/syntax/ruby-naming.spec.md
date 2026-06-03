# Spec — Ruxen Surface Syntax (Ruby Flavor)

Status: Draft (2026-05-16)
Owner: language
Scope: end-state surface syntax — keywords, declaration forms,
       visibility, mixins, attributes, stdlib type names.
       Internal compiler naming (HIR/MIR/codegen) follows the same
       vocabulary but is incidental — covered in implementation
       phases, not here.

When any other document in the repo (tutorial, stdlib spec,
requirements doc, fixture) disagrees with this file on a surface
form, this file wins and the other should be updated.

This document is the **canonical source** for what Ruxen looks like
on the page. Every tutorial, requirements doc, fixture, and example
conforms to the forms below.

---

## 1. Motivation

Ruxen's tagline is *reads like Ruby, compiles like Rust*. The first
half is the design promise the user makes a hiring decision on; the
second half is what the implementation provides. Where the two
collide on a naming choice, the surface bends toward Ruby — the
compile-time guarantees survive in semantics, not in spelling.

The forms below pick the Ruby-flavored spelling at every decision
point, retaining the Rust-flavored spelling **only when the concept
itself is foreign to Ruby** (lifetimes, mixin existential types) and
no Ruby idiom maps cleanly.

---

## 2. Goals & Non-goals

### Goals

- **G1.** A Ruby developer can read any Ruxen source file without a
  Rust dictionary.
- **G2.** Declaration metadata lives **in the body of the thing it
  modifies**, the way `private`, `attr_accessor`, and `include` live
  in Ruby class bodies. No prefix-annotation syntax.
- **G3.** Visibility follows Ruby's section-marker model:
  public-by-default, `private` / `protected` directives gate
  subsequent declarations until the next marker.
- **G4.** Mixins do two jobs that classical Ruby modules do — they
  carry method bodies (provision) AND declare required methods
  (contract). Same keyword, both jobs.
- **G5.** Existential parameter types use `some Mixin` (one concrete
  type, picked by the compiler per call site) and `any Mixin` (any
  conforming type, dispatched at runtime through a vtable). The
  words are chosen for English readability, not protocol-language
  precedent.
- **G6.** Stdlib type names use the Ruby word where one exists
  (`Array`, `Set`, `Map`), the descriptive English word where Ruby
  has no analogue (`Shared`, `SharedSync`), and the existing name
  where Ruxen made a deliberate semantic departure (`Option`,
  `Result`, `Box`, `String`, `&str`).
- **G7.** Lifetime parameters appear in the same `[...]` slot as
  type parameters, lowercase-named, no sigil.

### Non-goals

- **NG1.** Changing what the language *means* — ownership, borrow
  checking, monomorphization, vtable layout, drop semantics are all
  untouched. This is a surface-syntax pass.
- **NG2.** Adding parallel old-form syntax as a transition. Old
  spellings stop parsing the day the rename lands. No deprecation
  warnings — the build either takes the new form or fails.
- **NG3.** Renaming internal compiler structs/enums except where it
  reduces friction for contributors reading both the source and the
  user-facing docs.

---

## 3. End-state syntax

### 3.1 Variable bindings

Two keywords. `let` is immutable, `var` is mutable. No two-word
forms.

```ruxen
let name = "Ruxen"
let pi = 3.14

var counter = 0
counter = counter + 1
counter += 1
```

Reassignment to a `let` is a compile error.

Type annotations attach the same way:

```ruxen
let x: Float = 42
var bytes: Array[UInt8] = Array.new
```

#### Module-level `let` as a constant

Module-level `let` bindings serve as program-wide constants —
naming convention is `SCREAMING_SNAKE_CASE`:

```ruxen
let MAX_RETRIES = 3
let DEFAULT_PORT: UInt16 = 8080
```

A module-level `let` whose initializer is a const-evaluable
expression (literal or arithmetic on literals — the same expression
language the const-generic argument position accepts) is usable in
type-level positions: const-generic arguments, fixed array sizes,
pattern guards. The compiler validates const-evaluability at the
*use site* that requires it — there is no separate `const` keyword
to declare the intent up front. Use sites that need a compile-time
value emit `E-NOT-CONST-EVAL` when given a `let` whose RHS isn't
const-evaluable.

#### Shadowing

A `let` or `var` may shadow a same-named binding from an enclosing
scope. The new binding takes a fresh type; the old binding is
unreachable until the new binding goes out of scope.

```ruxen
let x = 1            # x: Int
let x = "two"        # x: &str — shadows
puts "#{x}"
```

#### Destructuring

`let` and `var` accept irrefutable destructuring patterns:

```ruxen
let (a, b) = pair                  # tuple
let Point { x, y } = origin        # struct
```

Refutable patterns (`Some(x)`, `Ok(v)`, enum variants) must use
`if let`, `while let`, or `match` — a bare `let Some(x) = opt` is a
compile error because the match could fail.

### 3.2 Visibility

Public by default. Three section markers — `public`, `private`,
`protected` — gate subsequent declarations within `class`, `struct`,
`module`, and `mixin` bodies. Each marker stays in effect until the
next marker or the end of the body:

```ruxen
class Account
  balance: Int                  # public field (default)

  def init(@balance: Int) end

  def get_balance -> Int        # public method
    self.balance
  end

  private

  def normalize_balance         # private — until next marker
    self.balance.max(0)
  end

  protected

  def admin_dump -> String      # protected — subclass-visible
    "Account(#{self.balance})"
  end

  public                        # switch back to public

  def freeze -> Account         # public again
    Account.new(self.balance)
  end
end
```

Field visibility uses the same markers:

```ruxen
class User
  name: String                  # public field
  email: String                 # public field

  private
  audit_id: Int                 # private field
  audit_log: Array[String]      # private field
end
```

A method-name-list form is also accepted, matching Ruby:

```ruxen
class User
  def helper_a; ...; end
  def helper_b; ...; end
  def public_thing; ...; end

  private :helper_a, :helper_b
end
```

This form is applied as a final pass after the body is parsed — it
re-marks the listed methods, overriding any section marker they
were under. Useful for forward-declared visibility when readability
benefits from defining helpers near where they're used. The list
form may only re-mark items declared in the same immediate body —
it cannot reach across `module` or `extension` boundaries.

#### Scopes of visibility

A `private` declaration is visible only within the body of its
immediate container:

| Container body                  | `private` means visible inside…                            |
|---------------------------------|------------------------------------------------------------|
| `class C` / `struct C` / `enum C` | That type's body and any `extension` blocks on the same type, in any file of the same package. |
| `mixin M`                       | That mixin's body only.                                    |
| `module M`                      | That module's body across every file that re-opens it (within the same package). |
| Package root (no enclosing item) | That package only — invisible to dependent packages.       |

`protected` is **subclass-visible** (Java / C# / Swift semantics —
not Ruby's per-instance protected). A `protected` method is callable
on `self` and on any instance of a subclass of the declaring class
through `self`. It is not callable on an unrelated instance of the
same class from outside the type hierarchy.

`public` is the default and the most permissive — anything that can
name the item can call it. The `public` keyword as a section marker
exists only to flip back from a prior `private`/`protected` section.

### 3.3 Lifetimes

Lifetimes are generic parameters in the same `[...]` slot as types.
**Lowercase identifier = lifetime, uppercase = type.** No sigil.

There is **no `'a` sigil** (G7). A leading `'` always opens a raw
string (§3.10a); a stray `'a` is rejected as an unterminated raw
string (E0002), not read as a lifetime.

```ruxen
def longest[a](x: &a String, y: &a String) -> &a String
  if x.size > y.size; x; else; y; end
end

class Slice[T, a]
  data: &a Array[T]
  start: USize
  len: USize
end
```

The vast majority of code never names a lifetime — elision rules
infer them. Explicit lifetimes are an escape hatch for the cases
elision can't resolve.

#### Where lifetime identifiers are recognized

A lowercase identifier is read as a lifetime **only** in these
positions:

- Inside a generic-parameter list: `def f[a, T](...)`, `class C[T, a]`.
- After a reference marker: `&a String`, `&a var T`.
- In a `where` clause referencing a lifetime parameter already
  declared in the enclosing `[...]`.

Everywhere else, a lowercase identifier is a value binding. A user
variable named `a` does not collide with a lifetime parameter named
`a` — the two live in disjoint syntactic positions and the parser
never has to guess.

#### The `static` lifetime

`static` is the one named lifetime that exists without being
declared in a `[...]` slot. It means "outlives every other scope."
References to compiled-in constants (string literals, `const` items)
have lifetime `static` by default. A bound `T: static` on a type
parameter means "the type contains no non-`static` references."

```ruxen
let s: &static str = "compiled-in literal"
def spawn[T: static](value: T) -> JoinHandle[T] ...
```

### 3.4 Mixins

A `mixin` is the contract-and-provision unit. Methods inside a
mixin body come in two flavors:

- **Required** — signature with no body. Including class must
  provide.
- **Default** — signature with body. Including class gets the body
  for free, may override.

```ruxen
mixin Greetable
  def name -> String                  # required
  
  def greet -> String                 # default
    "Hello, #{self.name}!"
  end
end
```

A class adopts a mixin with `include`. Required methods become
obligations the compiler checks; default methods are pulled into
the class as if defined inline; class-defined methods with the same
name override the mixin's default.

```ruxen
class User
  name: String
  def init(@name: String) end
  
  include Greetable                   # name satisfied by field; greet inherited
end

class FormalUser
  name: String
  def init(@name: String) end
  
  include Greetable
  
  def greet -> String                 # overrides default
    "Good day, #{self.name}."
  end
end
```

Multiple includes stack; order is source order. The class's own
definition of a method always beats every included mixin's default.
If two included mixins each provide a default for the same method
name and the class itself has no override, the compiler rejects
with `E-MIX-AMBIGUOUS-DEFAULT` and requires the class to define
its own method to disambiguate. A required method may be satisfied
by any included mixin's default.

#### `super` inside an override

Inside a method that overrides a mixin's default, `super` calls
that mixin's default body. If the class also inherits from a
superclass that defines the same method, the superclass override
takes precedence — `super` reaches the superclass first; the mixin
default is reached only when no superclass implementation exists in
the chain.

Mixins support inheritance: `mixin Sortable: Ord` means
`Sortable`'s contract extends `Ord`'s. A class that
`include Sortable` must satisfy both.

Mixins support associated types:

```ruxen
mixin Iterator
  type Item
  def var next -> Option[Self.Item]
end
```

#### Existentials: `some Mixin`, `any Mixin`

A function parameter or return type may reference a mixin in one of
two ways:

**`some Mixin`** — the compiler picks one concrete conforming type
per call site. The function body is monomorphized; the receiver
type is not visible to callers but is fixed for any given call.
Zero runtime cost. Methods may inline.

```ruxen
def print_it(item: &some Greetable)
  puts item.greet
end

print_it(&User.new("Alice"))   # specialized for User
print_it(&Robot.new(42))       # specialized for Robot
```

**`any Mixin`** — the value carries a vtable at runtime; one
function body handles all conforming types. Required for
heterogeneous collections.

```ruxen
def shout_all(crowd: &Array[Box[any Greetable]])
  for member in crowd
    puts member.greet.upcase
  end
end
```

Coercions to existential types happen implicitly at assignment and
call boundaries. Accepted sites:

| From            | To                      | When                                              |
|-----------------|-------------------------|---------------------------------------------------|
| `&T`            | `&some Mixin`           | `T` satisfies the mixin (structurally or via `include`) |
| `&T`            | `&any Mixin`            | `T` includes the mixin nominally                  |
| `Box[T]`        | `Box[any Mixin]`        | `T` includes the mixin nominally                  |
| `Shared[T]`     | `Shared[any Mixin]`     | same                                              |
| `SharedSync[T]` | `SharedSync[any Mixin]` | same                                              |

Existentials are read-only views or owned values — there is **no**
`&var some Mixin` or `&var any Mixin`. If you need to mutate
through an existential receiver, take ownership (`Box[any Mixin]`,
`Shared[any Mixin]`, etc.) and call writing methods on it; if you
need exclusive writable access to a value, name its concrete type.

Container generics over a non-existential `T` are **invariant** —
`Array[T1]` does not coerce to `Array[T2]` even when `T1: T2`, and
`Array[T] → Array[any Mixin]` is rejected. Wrap each element with
`Box.new(...)` first and use `Array[Box[any Mixin]]`.

A mixin is *object-safe* (usable through `any`) if every method
satisfies: no `Self`-by-value in arg/return, no per-method generic
parameters, no class-method (`def self.foo`) entries, no
`consume self` receiver. Associated types must be bound at the use
site: `any Iterator[Item = Int]` is OK; `any Iterator` is not.

Structural satisfaction is accepted for `some Mixin` only. `any
Mixin` requires an explicit `include Mixin` directive in the
implementing class.

The default lifetime of an unannotated `any Mixin` value follows the
container: `&any Mixin` borrows for the surrounding reference's
lifetime; `Box[any Mixin]` / `Shared[any Mixin]` / `SharedSync[any
Mixin]` default to `static` for their inner-type bound (the contained
concrete type must itself satisfy `static`).

### 3.4a Methods on a type

Method names may carry Ruby's conventional suffixes: `?` for
predicates (`empty?`, `include?`, `any?`) and `!` for in-place / bang
variants (`sort!`). The suffix is part of the name. Because `?`
belongs to method names, **safe navigation is `&.`** (Ruby), not
`?.`: `user&.name&.upcase`. (The standalone `?` after a call is still
the try operator: `parse(s)?`; uppercase `T?` is still an optional
type.)

A class, struct, or enum's methods live inside the type body — there
are no separate "methods-for-this-type" blocks for the common case:

```ruxen
class Container[T]
  items: Array[T]

  def init
    self.items = Array.new
  end

  def count -> Int
    self.items.size
  end
end
```

For methods that should exist **only when a type parameter satisfies
a bound**, use an `extension` block:

```ruxen
extension Container[T] where T: Display
  def print_all
    for item in self.items
      puts item.to_display
    end
  end
end
```

An `extension` may also add methods unconditionally to a foreign or
generic type, without re-opening the original definition. The same
syntax applies; the `where` clause is optional.

The body of a `class`, `struct`, `enum`, or `extension` block may
also carry `include` directives. The destructor pattern is one
example:

```ruxen
class SafeBuffer
  ptr: *var UInt8
  len: USize

  def init(size: USize)
    unsafe
      self.ptr = malloc(size) as *var UInt8
      self.size = size
    end
  end

  include Drop

  def var drop
    unsafe
      free(self.ptr as *var Void)
    end
  end
end
```

`include Drop` declares the type as a `Drop` participant. The
mixin requires a `def var drop` method; the class provides one.
Same `include` directive, same scattered-method rule as §3.4.

### 3.5 Layout directives

`struct` bodies may carry a `layout` directive at the top of the
body. Three forms:

```ruxen
struct Point
  layout c                    # field order preserved, native align,
                              # ABI-compatible with C struct
  x: Int
  y: Int
end

struct Header
  layout packed               # no inter-field padding
  kind: UInt8
  flags: UInt32
  length: UInt64
end

struct UserId
  layout transparent          # single-field newtype, inherits inner layout
  inner: Int
end
```

`layout transparent` is rejected on multi-field structs.

Without a `layout` directive, the compiler may reorder fields for
size optimization (declaration order is **not** guaranteed).

### 3.6 Implicit includes for structural mixins

`Debug`, `Clone`, `Eq`, `Hash`, etc. are mixins. The `include`
directive (§3.4) is the only mechanism for adopting a mixin —
there is no separate auto-synthesis concept and no `derive` keyword.

For a fixed set of **structural mixins**, `include` is **implicit**.
The compiler treats the class as if `include` had been written, when
the fields structurally support the mixin. The implicit-include set
and its rule:

| Mixin       | Implicit when…                                              |
|-------------|-------------------------------------------------------------|
| `Debug`     | Always — formats as `TypeName(field=value, ...)`.           |
| `Clone`     | Every field is `Clone`.                                     |
| `Eq`        | Every field is `Eq`. Field-wise `==`.                       |
| `PartialEq` | Same as `Eq`, partial.                                      |
| `Hashable`  | Every field is `Hashable`. FNV mixer over fields in source order. |
| `Default`   | Every field has a default value.                            |
| `Ord`       | Every field is `Ord`. Lexicographic by source order.        |
| `PartialOrd`| Same as `Ord`, partial.                                     |
| `Send`      | Every field is `Send`. Auto-mixin (see note).                |
| `Sync`      | Every field is `Sync`. Auto-mixin (see note).                |

`Copy` is structural and ownership-affecting:

- A `struct` whose every field is `Copy` implicitly `include`s `Copy`.
- A `struct` with any non-`Copy` field does not.
- A `class` never implicitly `include`s `Copy` (reference semantics).

For any **other** mixin — `Display`, `Greetable`, `Drawable`, custom
mixins — `include` is required at the use site.

#### `Send` and `Sync` are auto-mixins

`Send` and `Sync` differ from the other structural mixins on the
table: they are **never written explicitly with `include`** in
ordinary code. The compiler infers them per the field-rule in
`docs/requirements/tier1_02_concurrency.md §4.2`. A type opts out
with `exclude Send` / `exclude Sync` in its body. A type opts in for
an inference-incompatible structure (e.g. a hand-rolled lock-free
queue) via `unsafe include Send` / `unsafe include Sync` — the only
legal use of `unsafe include`.

```ruxen
class RawHandle
  fd: *var Void
  exclude Send       # opt out — inference would also reach !Send
  exclude Sync
end
```

The canonical names of the structural mixins above are also the
names exposed by `std.prelude` — see
`docs/specs/stdlib/prelude.spec.md` for the auto-import list. Names
no longer accepted: `Hash` (use `Hashable`), `Displayable` (use
`Display`), `Comparable` (use `Ord` and/or `PartialOrd`), `Iterable`
(absorbed into `Iterator`).

#### Writing the include explicitly

For documentation clarity or when you want the inclusion to fail
loudly if the rule no longer applies, write it:

```ruxen
struct Point
  x: Int
  y: Int
  include Debug, Clone, Eq, Hashable    # all four are also implicit; loud form
end
```

If an explicit `include Hashable` is written and a field doesn't
support `Hashable`, the error fires at the **include site** (early).
With implicit-only, the same error fires at the **use site** (later,
but still caught at compile time).

#### Overriding a default method

Default methods of any included mixin (implicit or explicit) can be
overridden by defining the method in the class body:

```ruxen
struct Point
  x: Int
  y: Int

  def to_debug -> String              # overrides implicit Debug
    "(#{self.x}, #{self.y})"
  end
end
```

The user's definition wins; the implicit `include` does not provide
a duplicate.

### 3.7 FFI

External libraries are declared with `lib`. The string after `lib`
is the link name. Options follow as keyword arguments:

```ruxen
lib "m"
  def sin(x: Float) -> Float
  def cos(x: Float) -> Float
end

lib "sqlite3", version: "3"
  def sqlite3_open(path: *UInt8, db: *var *Void) -> Int32
end

lib "c", path: "/usr/lib"
  def malloc(n: USize) -> *var Void
end
```

No separate `extern` block keyword. Every FFI declaration goes
inside a `lib "..." ... end` block.

Calls into a `lib` block are inherently unsafe (the compiler can't
verify the C side); they must appear inside `unsafe` blocks.

#### Pointer ownership at FFI signatures

Pointer types at FFI signatures are **non-owning by default**. A
`*UInt8` parameter is borrowed for the duration of the call; the
called C function must not retain it beyond that call. To transfer
ownership across the boundary, the wrapper must spell the transfer
out — e.g. `String.from_raw(ptr, len)` to take ownership of a
returned C buffer, or `Box.from_raw(ptr)` for a single allocation.
Ruxen does not insert any implicit drop for raw pointers — the FFI
boundary is where the user takes responsibility for memory.

Strings cross the FFI boundary in one of three forms:

| Ruxen type        | C side                                                  |
|-------------------|---------------------------------------------------------|
| `&str`            | `const char *` — borrowed, valid for the call           |
| `String`          | C must not retain after call returns                    |
| `*UInt8` (raw)    | bare read-only pointer; ownership convention is whatever the wrapper documents |
| `*var UInt8`      | writable raw pointer; same convention rule              |

#### Variadic C functions

The parser accepts a trailing `...` inside `lib` block `def`
signatures:

```ruxen
lib "c"
  def printf(fmt: *UInt8, ...) -> Int32
end
```

The variadic arguments at a call site must be primitive scalars or
pointer values — aggregate types passed through `...` produce a
typeck error.

### 3.8 Inline

`inline` is a modifier on a `def` declaration:

```ruxen
inline def fast_path(x: Int) -> Int
  x * 2 + 1
end
```

Or, Ruby-style, a directive that names a previously defined method:

```ruxen
def fast_path(x: Int) -> Int
  x * 2 + 1
end
inline :fast_path
```

`inline` is a hint, not a guarantee. The codegen backend treats it
as `alwaysinline` when LLVM is wired (v2); Cranelift currently
ignores it.

### 3.9 Packages and module paths

The compilation unit is a **package**. `package` is the keyword
that refers to "this package" in import paths:

```ruxen
use package.utils.format
use package.models.User
```

`use` is unchanged — the same `use` Ruby and Swift readers expect
for namespace imports.

The path separator is `.` (a single dot) **everywhere** — `use`
paths (`std.io.Stdout`), qualified type names, enum variants
(`Color.Red`), and class methods (`Array.new`). The `::` separator
is not accepted. Any older spec or test fixture that still uses `::`
is out of date.

Group imports and aliases:

```ruxen
use std.io.{ Stdin, Stdout, Stderr }
use std.io.Stdout as Out
use std.collections.{ Map, Set }
```

### 3.9a User modules

A `module Name ... end` block declares a namespace inside a file.
Items inside the body follow the same visibility rules as top-level
items (§3.2). Nested modules are legal. The body may carry any
top-level item: `class`, `struct`, `enum`, `mixin`, `extension`,
`def`, `use`, `type`, `newtype`, `let`, `lib`.

```ruxen
module Http
  class Request
    url: String
    def init(@url: String) end
  end

  def get(url: &str) -> Result[Response, HttpError]
    # ...
  end
end

use Http.Request
```

> **Implementation status (v1):** the parser accepts user modules
> fully; the resolver handles `std.*` paths and direct same-file
> nested access. Cross-file user-module path resolution (`use
> mymod.X` where `mymod` is user-defined) is partial — see
> `docs/specs/system/user-modules.spec.md` for the current envelope.
> The full resolver lands in v1.1. Until then, code that needs
> cross-file modularity should organize by file/`use` rather than
> nested user `module` blocks.

### 3.10 Nil — the universal absence literal

`nil` is the **single empty literal**. It is polymorphic across three
syntactic positions:

1. The absence case of `Option[T]` (safe code):
   ```ruxen
   def find(id: Int) -> Option[User]
     if id < 0; return nil; end
     # ...
   end
   ```
2. The raw-pointer null literal at `*T` / `*var T` (FFI / unsafe):
   ```ruxen
   unsafe
     let ptr: *var UInt8 = nil
     if some_ptr == nil; return Err("got null"); end
   end
   ```
3. The unit value and the unit return type — "this carries / returns
   nothing":
   ```ruxen
   def log(msg: &String) -> nil
     puts msg
     nil
   end
   let r: Result[nil, String] = Ok(nil)
   ```

**`None` is not a valid spelling.** There is no `None` keyword or
identifier; writing `None` is rejected at lex time with `E0008`
("use `nil`"). The empty case of an `Option[T]` is always `nil`.

**`()` is not a valid spelling either.** The Rust-style unit literal /
unit type `()` is rejected by the parser in both positions (a fix-it
points at `nil`). Write `nil` for the unit type (`def f -> nil`,
`Result[nil, E]`) and the unit value (`Ok(nil)`).

Equality comparisons (`==` / `!=`) with `nil` pick whichever of the
two domains matches the other operand's type.

#### Inference rule (resolved priority order)

The type of a `nil` literal is determined by the surrounding
expected-type context in this priority order:

1. The declared type of the binding or parameter (`let ptr: *var UInt8 = nil`).
2. The ambient return type when `nil` appears in a return position
   (`return nil` inside a `def f -> Option[User]`).
3. The peer type of an `==` / `!=` comparison (`x == nil` where
   `x: Option[T]` → `nil: Option[T]`).
4. The function-parameter type at a call site (`f(nil)` where `f`
   takes `*var Void` → `nil: *var Void`).

If two or more of these contexts produce *different* candidate
types that are simultaneously well-typed (e.g. a function returning
`Option[*var T]` evaluating `return nil` — both `Option::None` and
`Some(*var T)`'s null payload would type-check), the compiler emits
`E-NIL-AMBIGUOUS` and requires the user to disambiguate explicitly
(`Some(nil)`, `nil as *var T`, or refactor the signature).

A pointer-typed `nil` is itself only legal inside an `unsafe`
block — a bare `nil` resolving to a pointer type outside `unsafe`
emits `E-NIL-RAW-OUTSIDE-UNSAFE`. The `Option[T]` use case never
triggers either diagnostic.

`Some(value)` remains the present-case constructor for `Option[T]`.
The asymmetry is intentional — presence has to wrap a value;
absence is just `nil`.

Ruxen references (`&T`, `&var T`) cannot be `nil` — they are valid
by construction. If you want a possibly-missing reference, use
`Option[&T]`.

Array literals use bare `[...]` and produce an `Array[T]` (see
§4.4). Map literals use bare `{ k => v, ... }` and produce a
`Map[K, V]`. There is no dedicated `Set` literal — use
`Set.from_iter([…])` since `{…}` is reserved for `Map` (the parser
distinguishes the struct/enum literal form `Path { field: value }`
from a Map literal by the presence of a leading identifier or path —
see §3.22).

### 3.10a String and character literals

Ruxen has three text-literal forms, distinguished by their delimiter:

| Form        | Delimiter | Escapes | `#{}` interpolation | Notes |
|-------------|-----------|---------|---------------------|-------|
| Interpolated string | `"…"` | yes | yes | the default string form |
| Raw string  | `'…'`     | no      | no    | content verbatim; can hold `"` but not `'` |
| Character   | `?c`      | via `?\…` | n/a | a single `Char` (Unicode scalar) |

```ruxen
let greeting = "hi #{name}\n"     # interpolated: name spliced in, \n is newline
let path     = 'C:\Users\me'      # raw: backslashes literal, no interpolation
let newline  = ?\n                # Char (the newline scalar)
let letter   = ?A                 # Char
```

Rationale and rules:

- **Single quotes are raw strings, not char literals.** The Rust-style
  `r"…"` / `r#"…"#` raw-string prefix is retired — single quotes cover
  the raw case. A raw string cannot contain a `'`; reach for a `"…"`
  string (escaping as needed) when you need one.
- **Character literals use the `?c` form** (Ruby-style): `?a`, `?\n`,
  `?\t`, `?\\`, `?\'`, `?\u{1F600}`. A `?` is only read as a char
  literal in an *expression-context* position so postfix `?` (try),
  `?.` (safe-navigation), and optional-type `T?` keep their operator
  meaning.
- **Lifetimes** still use the leading-quote form (`'a`, `'input`): a
  single quote with no closing quote on the same line is a lifetime,
  not a raw string.

### 3.10b Ranges

Ranges follow **Ruby** semantics, not Rust's:

| Form     | Meaning   | Example   | Iterates |
|----------|-----------|-----------|----------|
| `a..b`   | inclusive | `0..3`    | 0, 1, 2, 3 |
| `a...b`  | exclusive | `0...3`   | 0, 1, 2 |

```ruxen
for i in 0..n      # inclusive: 0 through n
for i in 0...n     # exclusive: 0 through n-1
```

The Rust `..=` inclusive form **does not exist** — `..=` is rejected
at lex time (E0009) with a fix-it pointing at `..`. (The `..` rest
pattern in array/struct destructuring is unchanged; it's not a range.)

### 3.11 Stdlib type names

| Type            | Role                                     |
|-----------------|-------------------------------------------|
| `Array[T]`      | Growable heap-allocated sequence          |
| `Set[T]`        | Hash-based unique set                     |
| `Map[K, V]`     | Hash-based key-value map                  |
| `Option[T]`     | `Some(v)` or `nil` — replaces nullability  |
| `Result[T, E]`  | `Ok(v)` or `Err(e)` — replaces exceptions |
| `Box[T]`        | Owning heap pointer                       |
| `Shared[T]`     | Reference-counted, single-threaded        |
| `SharedSync[T]` | Atomically reference-counted, thread-safe |
| `String`        | Owned, heap-allocated, growable string    |
| `&str`          | Borrowed string slice (UTF-8 view)        |

Stdlib import paths and constructor names follow:

```ruxen
let words: Array[String] = Array.new
let seen: Set[Int] = Set.new
let counts: Map[String, Int] = Map.new
let boxed: Box[Point] = Box.new(Point.new(1, 2))
let shared: Shared[Counter] = Shared.new(Counter.new)
let cross: SharedSync[State] = SharedSync.new(State.new)
```

The canonical constructor convention:

- `.new(args)` — no-conversion constructor; arguments are stored as-is
  (`Array.new`, `Map.new`, `User.new("alice")`).
- `.from(value)` — conversion constructor; takes a value of a related
  type and converts it (`String.from(&str)`, `Box.new(point)` —
  note: `Box` uses `.new` because no conversion is implied).
- `.from_iter(iter)` — drain an iterator into a fresh collection
  (`Set.from_iter([1, 2, 3])`, `String.from_iter(chars)`).
- `.with_capacity(n)` — pre-allocate a heap-backed collection of size
  `n` (`Array.with_capacity(64)`).

A type may expose more than one. Tutorials and stdlib specs must
pick the canonical form per type and use it consistently.

The complete list of prelude-imported type and mixin names (the
names available without any `use`) lives in
`docs/specs/stdlib/prelude.spec.md`. The table above is the
syntax-level subset that this spec pins.

### 3.12 Self-mode terminology

A method's relationship to its receiver is one of four modes:

| Form              | Mode       | Meaning                                |
|-------------------|------------|----------------------------------------|
| `def m`           | reading    | Receiver is read-only (default).       |
| `def var m`       | writing    | Receiver is writable; method may mutate `self`. |
| `def consume m`   | consuming  | Takes ownership; receiver is moved.    |
| `def self.m`      | class      | No receiver — module-style call.       |

`var` here is the same keyword as the binding form `var x = ...` —
it consistently means "writable." A `let`-bound value cannot
receive a `def var` method call; a `var`-bound value can.

Tutorials and specs describe these in mode terms ("a reading
method", "a writing method", "a consuming method"). The internal
references-to-self notation is not user-facing.

### 3.13 The `!` macro suffix

Methods ending in `!` follow Ruby's convention for danger/mutation:

```ruxen
let value = option.unwrap!
let value = result.expect!("must be present")
panic!("unrecoverable")
```

Reserved for compiler-aware forms. User-defined methods may not end
in `!`.

### 3.14 Reserved keywords

| Category       | Keywords                                                                 |
|----------------|--------------------------------------------------------------------------|
| Binding        | `let`, `var`, `move`, `ref`                                              |
| Types          | `class`, `struct`, `enum`, `mixin`, `newtype`, `type`                    |
| Functions      | `def`, `init`, `self`, `Self`, `super`, `return`, `yield`                |
| Modes          | `consume`, `inline`                                                      |
| Control flow   | `if`, `elsif`, `else`, `match`, `while`, `for`, `in`, `loop`, `do`, `end`, `break`, `continue` |
| Type system    | `where`, `as`, `some`, `any`, `static`, `const`, `layout`, `include`, `exclude`, `extension` |
| Visibility     | `public`, `private`, `protected`                                         |
| Modules        | `module`, `use`, `package`                                               |
| Safety         | `unsafe`                                                                 |
| Literals       | `true`, `false`, `nil`, `Some`, `Ok`, `Err`                              |
| FFI            | `lib`                                                                    |
| Async (reserved) | `async`, `await`                                                       |
| Actor (reserved, v2) | `spawn`, `actor`, `send`, `receive`                                |

Keywords are lowercase by convention. Type-level identifiers (`Self`,
`String`, the auto-mixin names `Send` / `Sync`) are capitalized and
live in a separate namespace — the keyword `send` and the auto-mixin
`Send` do not collide at the lexer.

The keyword `var` plays four positional roles, all of them meaning
"writable":

- Binding: `var x = 0`
- Reference type: `&var T`
- Raw pointer type: `*var T`
- Method receiver: `def var bump`

The opposite "read-only" is the absence of `var` in each position
(`let x`, `&T`, `*T`, `def m`). There is no separate `mut`
keyword.

`static` is a keyword only inside type-system positions (lifetime
names and lifetime bounds). It is not an item-level storage class —
process-global state is expressed through module-level `let`
bindings for compile-time-known values (see §3.1) and through
library types (`SharedSync[Mutex[T]]`, `Once`) for runtime state.

`const` is a keyword only inside generic-parameter lists, marking
a const-generic parameter: `[T, const N: USize]`. There is no
`const` binding form for ordinary values — module-level `let` is
the binding form for constants.

### 3.15 Comments

Three comment forms:

```ruxen
# line comment — runs to end of line
let x = 1  # inline comment

#= block comment
   may span multiple lines
   #= nested block comments are legal =#
=#

## Doc comment — attaches to the following item.
##
## Supports Markdown. Captured into HIR for documentation generation
## and LSP hover; the first paragraph is the summary.
def find_user(id: Int) -> Option[&User]
  # ...
end
```

Doc comments (`##`) attach to the next item in the same file. They
are not retained on expressions — only on items (`def`, `class`,
`struct`, `enum`, `mixin`, `type`, `newtype`, `module`,
`extension`, `lib`).

### 3.16 String literals

Four forms:

```ruxen
let plain  = "hello"                # &str literal — borrows compiled-in bytes
let interp = "hi #{name}"           # interpolation — produces an owned String
let raw    = r"no\escape\here"      # raw string — no escape processing
let raw_q  = r#"can have "quotes""# # raw with explicit `#` delimiter

let multi = """
  Indented multiline string.
  The leading indentation is stripped to the closing delimiter's
  column.
"""
```

Interpolation `#{...}` is accepted in `"..."` and `"""..."""` but
not in raw strings. The interpolated expression routes through the
`Display.fmt` dispatch path (§3.6). Format specs use the syntax
`"#{expr:[<fill><align>][<width>][.<precision>][?]}"` documented in
`docs/specs/stdlib/fmt.spec.md`.

Plain `"..."` literals have type `&static str`. Any operation that
produces new bytes (`+`, interpolation, `.to_string`, `.clone`)
returns an owned `String`.

Standard character escapes inside `"..."`: `\n`, `\r`, `\t`, `\\`,
`\"`, `\'`, `\0`, `\u{XXXX}` for arbitrary Unicode scalars up to
`U+10FFFF`.

### 3.17 Closures

One closure concept, two syntactic spellings (single-line brace
form and multi-line `do`/`end` form) producing identical values:

```ruxen
let double = { |x: Int| x * 2 }         # brace form
let total = nums.fold(0) do |acc, n|
  acc + n
end                                     # do/end form (trailing)
```

A closure value is called with `.(...)`:

```ruxen
let result = double.(10)                # 20
```

Closure types name the call-capacity, capitalized type-level
identifiers:

| Type                | Meaning                                                |
|---------------------|--------------------------------------------------------|
| `Fn(A, B) -> R`     | Callable any number of times; captures borrowed read-only. |
| `FnVar(A, B) -> R`  | Callable any number of times; captures borrowed writably.  |
| `FnOnce(A, B) -> R` | Callable once; consumes its captures.                  |

The pipe form `|A, B| -> R` is an equivalent short spelling of
`Fn(A, B) -> R` in parameter and return positions. The two forms
compile to the same type; both `def f(pred: |Int| -> Bool)` and
`def f(pred: Fn(Int) -> Bool)` are accepted.

Captures are inferred per use. A `move` prefix forces every capture
to be owned (used when the closure must outlive the captured scope
— e.g. crossing a thread boundary):

```ruxen
def make_adder(n: Int) -> some Fn(Int) -> Int
  move { |x| x + n }
end
```

A trailing-block call passes the closure as the last argument:

```ruxen
nums.each do |n|
  puts n
end

# is equivalent to:
nums.each({ |n| puts n })
```

Inside a method body, the keyword `yield` invokes the implicit
trailing-block argument:

```ruxen
def with_timing
  let start = Instant.now
  yield
  puts "took #{start.elapsed.as_nanos} ns"
end
```

`yield` is *only* legal where the caller passed a trailing block;
calling a `yield`-using method without a block is a compile error.

### 3.18 Tuples

Fixed-arity heterogeneous values, written `(a, b, ...)`. Type form
is `(T1, T2, ...)`. Single-element tuple is `(T,)` (with trailing
comma — `(T)` is a parenthesized expression).

```ruxen
let point: (Int, Int) = (3, 4)
let (x, y) = point                      # destructuring

def divmod(a: Int, b: Int) -> (Int, Int)
  (a / b, a % b)
end
```

Tuples participate in the structural mixin rules of §3.6: a tuple of
`Copy` / `Clone` / `Eq` / etc. elements is itself `Copy` / `Clone` /
`Eq` / etc. Tuple field access is positional via `.0`, `.1`, ….

The unit type `()` is the zero-arity tuple. It is the implicit
return type of any `def` that has no `-> T` annotation; it is the
`Unit` referenced in stdlib specs.

### 3.19 Newtype

`newtype Name(InnerType)` declares a single-field wrapper that is
type-distinct from its inner type but shares its representation
(equivalent to `struct Name; layout transparent; inner: InnerType;
end`).

```ruxen
newtype UserId(Int)
newtype Email(String)

let id   = UserId(42)                   # constructor — same name as the newtype
let raw  = id.0                         # positional inner access
```

A newtype inherits its inner type's `Copy` / `Clone` status. It
does **not** inherit any other mixin includes — adopt them
explicitly with `include` in an `extension` block:

```ruxen
extension UserId
  include Display

  def fmt(f: &var Formatter) -> Result[(), FmtError]
    f.write_str("user/#{self.0}")
  end
end
```

### 3.20 Numeric literal coercion

A bare numeric literal (`42`, `3.14`) has a default type but
participates in inference for its assignment context:

| Context                   | Result                                            |
|---------------------------|---------------------------------------------------|
| `let x = 42`              | `x: Int` (default `Int64`)                        |
| `let x = 3.14`            | `x: Float` (default `Float64`)                    |
| `let x: Float = 42`       | literal narrows; `x: Float = 42.0`                |
| `let x: UInt8 = 200`      | literal narrows; legal because `200 < 256`        |
| `let x: UInt8 = 300`      | compile error `E-NUM-OVERFLOW` (out of range)     |
| `let x: Int8 = -200`      | compile error `E-NUM-OVERFLOW`                    |

Implicit widening across already-typed *values* is not performed:
`let a: Int8 = 1; let b: Int64 = a` is rejected — write `a as Int64`.
The `as` cast is the only narrowing/widening conversion among
numeric types.

Suffix forms (`i8`, `i16`, `i32`, `i64`, `u`, `u8`, `u16`, `u32`,
`u64`, `isize`, `usize`, `f32`, `f64`) pin a literal's type:

```ruxen
let b: UInt8 = 200u8
let pi32     = 3.14f32                  # Float32
```

Integer literal prefixes: `0x` hex, `0b` binary, `0o` octal.
Underscores `_` are accepted as digit separators (`1_000_000`).

### 3.21 `for` loops

The `for x in iter` loop expects `iter` to satisfy `Iterator` (its
required method is `def var next -> Option[Self.Item]`). The loop
desugars to:

```ruxen
var __it = <iter expression>
loop
  match __it.next
    Some(x) -> # loop body
    nil    -> break
  end
end
```

For ergonomics on collections, the iter expression follows these
rules:

| Source                     | What `__it` is                  |
|----------------------------|---------------------------------|
| `for x in collection`      | `collection.into_iter()` — moves the collection; elements bind by value |
| `for x in &collection`     | `collection.iter()` — read-only borrow; elements bind as `&T` |
| `for x in &var collection` | `collection.iter_var()` — writable borrow; elements bind as `&var T` |

A type that includes `Iterator` directly may be used as the source
expression with no further desugaring — the type *is* the iterator.

`for i in 0..10` uses the inclusive range form `..`, producing an
`Iterator[Item = Int]` covering `0` through `10`. `0...10` is the
exclusive form, covering `0` through `9`.

### 3.22 Struct and enum literals

A struct or enum value may be constructed with `.new(...)`
(positional) or with the field-literal form (named):

```ruxen
let p1 = Point.new(1, 2)                # positional
let p2 = Point { x: 1, y: 2 }           # named — same value

let c  = Color.Red                      # unit variant
let m  = Color.Custom { r: 1, g: 2, b: 3 }  # enum struct-variant
let s  = Some(42)                       # enum tuple-variant
```

The named form is **disambiguated from `Map` literals** by what
precedes the opening `{`:

- `{ ... }` with no preceding identifier or path → `Map` literal
  (`{ "k" => v }`).
- `Capitalized.Path { field: value, ... }` → struct or enum
  literal.

Field order in the named form is irrelevant; every field must
appear exactly once. Functional-update / `..rest` syntax is
deferred to v2.

The `Path.new(...)` form remains the canonical constructor for stdlib
types and for any type that has a custom `def init(...)` body
(constructors may run logic; the field-literal form bypasses
`init`). The field-literal form is preferred for the round-trip with
`Debug` output and for simple data structs without an explicit
`init`.

---

## 4. Worked examples

### 4.1 A class with a mixin and visibility

```ruxen
mixin Ord
  def cmp(other: &Self) -> Ordering

  def less_than(other: &Self) -> Bool
    self.cmp(other) == Ordering.Less
  end
end

class Version
  major: Int
  minor: Int
  patch: Int

  def init(@major: Int, @minor: Int, @patch: Int) end

  include Ord

  def cmp(other: &Version) -> Ordering
    let m = self.major.cmp(&other.major)
    if m != Ordering.Equal; return m; end
    let n = self.minor.cmp(&other.minor)
    if n != Ordering.Equal; return n; end
    self.patch.cmp(&other.patch)
  end

  private

  def self.parse(s: &str) -> Result[Version, ParseError]
    # ...
  end
end

let a = Version.new(1, 2, 3)
let b = Version.new(1, 3, 0)
if a.less_than(&b)
  puts "a < b"
end
```

Note: `Ord` (full order) is the loud form of the same `cmp` method
shape — see §3.6 for the structural-mixin table. The example
re-declares `mixin Ord` for clarity; in normal user code, `Ord` is
auto-included by §3.6 whenever every field is `Ord`, and the
`cmp` method is supplied by the type itself.

### 4.2 Heterogeneous collection with `any`

```ruxen
mixin Drawable
  def draw -> String
end

class Square
  side: Float
  def init(@side: Float) end
  include Drawable
  def draw -> String; "square(#{self.side})"; end
end

class Circle
  radius: Float
  def init(@radius: Float) end
  include Drawable
  def draw -> String; "circle(#{self.radius})"; end
end

def render_scene(shapes: &Array[Box[any Drawable]])
  for shape in shapes
    puts shape.draw
  end
end

var scene: Array[Box[any Drawable]] = Array.new
scene.push(Box.new(Square.new(1.0)))
scene.push(Box.new(Circle.new(2.5)))
render_scene(&scene)
```

### 4.3 FFI with `lib` options

```ruxen
lib "c"
  def malloc(n: USize) -> *var Void
  def free(p: *var Void)
end

lib "ssl", version: "3"
  def SSL_new(ctx: *var Void) -> *var Void
  def SSL_free(s: *var Void)
end

def allocate(size: USize) -> Result[*var UInt8, AllocError]
  unsafe
    let raw = malloc(size)
    if raw == nil
      Err(AllocError.OutOfMemory)
    else
      Ok(raw as *var UInt8)
    end
  end
end
```

### 4.4 Generic function with bound and lifetime

```ruxen
def first_match[T, a](haystack: &a Array[T], pred: |&T| -> Bool) -> Option[&a T]
  for item in haystack
    if pred(item); return Some(item); end
  end
  nil
end

let words = ["alpha", "beta", "gamma"]
let found = first_match(&words, |w| w.starts_with("b"))
```

### 4.5 Layout — derives are automatic

```ruxen
struct Header
  layout c
  magic: UInt32
  version: UInt16
  flags: UInt16
  payload_len: UInt64
end
```

`Debug`, `Clone`, and `Copy` are auto-synthesized for this struct
(every field is a primitive integer; primitives satisfy all three).
No declaration needed.

---

## 5. Diagnostics surface

Error code naming follows the new vocabulary. The prefix families:

- **Existentials** — `E-ANY-*` (object safety, missing assoc-type
  binding, multi-mixin, GAT, etc.) and `E-SOME-*` for the
  `some Mixin` side.
- **Layout** — `E-LAYOUT-*` (`E-LAYOUT-TRANSPARENT-MULTI`,
  `E-LAYOUT-PACKED-BORROW`).
- **Implicit-include** — implicit-include validators keep their
  existing E06xx numbering; message text uses the new vocabulary.
- **Visibility** — `E-VIS-PRIVATE`, `E-VIS-PROTECTED`.
- **Lifetime** — `E-LIFE-*` (no sigil change in error messages —
  lowercase generic param names appear as written).
- **Mixin defaults** — `E-MIX-AMBIGUOUS-DEFAULT` (§3.4),
  `E-MIX-MISSING` (required-method not satisfied).
- **Nil polymorphism** — `E-NIL-AMBIGUOUS` (§3.10 priority rule
  produced two well-typed candidates), `E-NIL-RAW-OUTSIDE-UNSAFE`
  (pointer-typed `nil` reached outside an `unsafe` block).
- **Const evaluability** — `E-NOT-CONST-EVAL` (a use site that
  requires a compile-time value received a `let` whose RHS is not
  const-evaluable; see §3.1).
- **Numeric literal range** — `E-NUM-OVERFLOW` (literal narrows to
  a type that can't hold it; §3.20).

Specific code allocations land in the diagnostics module during
implementation; this spec fixes the prefixes only. The full error
registry lives in `docs/errors/`.

---

## 6. Object-safety rules

A mixin is object-safe (usable through `any`) iff every method
satisfies:

- **S1.** No `Self`-by-value in arg or return position. `&Self`,
  `&var Self`, `Box[Self]`, `&any Mixin` are fine.
- **S2.** No additional generic parameters on the method.
- **S3.** Not a class method (`def self.m`).
- **S4.** Not consuming (`def consume m`).
- **S5.** Associated types must be bound at the `any` use site:
  `any Iterator[Item = Int]` works, `any Iterator` does not.
- **S6.** No generic associated types (GATs).
- **S7.** Every parent mixin must itself be object-safe, with its
  associated types bound.

Violations are caught at mixin declaration time (the trait is
either object-safe or not, once) and at `any` use sites (missing
associated-type bindings).

---

## 7. Vtable layout (informational)

`any Mixin` is a fat pointer: `(data_ptr, vtable_ptr)`, 16 bytes,
8-byte align on 64-bit targets.

Each `any Mixin` value points to a vtable with this fixed prefix:

| Slot | Field      | Purpose                                  |
|------|------------|------------------------------------------|
| 0    | drop       | fn(*var u8) — drop glue for the value    |
| 1    | size       | usize — bytes to dealloc                 |
| 2    | align      | usize — alignment for dealloc            |
| 3..  | methods    | mixin methods, in source declaration order |

This is not stable ABI for FFI use. Internal layout only.

---

## 8. What this spec deliberately does not say

- Internal HIR/MIR struct names — implementation choice. May or
  may not mirror the user-facing words.
- Specific token names in the lexer — implementation choice. The
  *keywords the user types* are listed in §3.14; the enum variants
  in the lexer are an internal detail.
- Phasing of the migration — covered in a separate implementation
  plan that is deleted once the migration is complete.

---

## 9. Test matrix

### 9.1 Parser-level

- T-PARSE-01: `let x = 5` parses, immutable binding.
- T-PARSE-02: `var x = 5; x = 6` parses, mutable binding.
- T-PARSE-03: `class C; private; def f; end; end` makes `f` private.
- T-PARSE-04: `def f[a](x: &a Int)` parses, `a` is a lifetime param.
- T-PARSE-05: `mixin M; def f; end; end` parses with default body.
- T-PARSE-06: `include M` inside class body parses.
- T-PARSE-07: `&some M`, `&any M`, `Box[any M]` parse.
- T-PARSE-08: `layout c`, `layout packed`, `layout transparent` parse.
- T-PARSE-09: `derive` keyword is rejected as an unknown identifier.
- T-PARSE-10: `lib "c", version: "1"` parses with options.
- T-PARSE-11: `inline def f` and standalone `inline :f` parse.
- T-PARSE-12: `use package.x.y` parses.
- T-PARSE-13: `nil` parses as raw-pointer literal in unsafe context.

### 9.2 Semantic

- T-SEM-01: Including a mixin pulls in default methods.
- T-SEM-02: Required-but-missing methods produce E-MIX-MISSING.
- T-SEM-03: Class-defined method overrides mixin default.
- T-SEM-04: Object-safety check rejects `any GenericMethod`.
- T-SEM-05: `any Mixin` requires an explicit `include` block;
  structural match alone is rejected.
- T-SEM-06: `some Mixin` accepts structural match.
- T-SEM-07: `layout transparent` on multi-field rejected.
- T-SEM-08: Using a struct with a non-`Hash` field as a `Map` key is rejected with a use-site error naming the offending field.
- T-SEM-09: Private method called outside class rejected.
- T-SEM-10: Protected method callable from subclass.
- T-SEM-11: Lowercase `a` in `[a]` slot is a lifetime, not a type.

### 9.3 End-to-end

- T-E2E-01: A program defining `mixin Drawable`, two implementing
  classes, and `Array[Box[any Drawable]]` runs and prints both
  shapes.
- T-E2E-02: An `unsafe` FFI block calls `malloc`, checks `nil`,
  and frees.
- T-E2E-03: A class with `private` section can still be constructed
  through public `def init`.
- T-E2E-04: Inline-modifier method behaves identically to non-inline
  under Cranelift (semantic test; LLVM-specific test waits for v2).
- T-E2E-05: A struct with all-Clone fields auto-synthesizes `Clone` and round-trips through `.clone()` with no declaration.

---

## 10. Open questions

- **OQ-1.** Should `inline` accept a numeric hint (`inline 3`,
  cost-based)? Defer to v2.
- **OQ-2.** Resolved (2026-05-16). `nil` is a single token; type is
  determined by the priority rule in §3.10. A pointer-typed
  resolution outside `unsafe` is its own diagnostic
  (`E-NIL-RAW-OUTSIDE-UNSAFE`).
- **OQ-3.** Resolved (2026-05-16). Inside a method that overrides a
  mixin default, `super` calls the mixin default. When the class
  also inherits from a superclass that overrides the same method,
  the superclass wins. Spec'd in §3.4 under "`super` inside an
  override."
- **OQ-4.** Resolved (2026-05-16). Promoted into §3.4: ambiguous
  stacked-mixin defaults are an error
  (`E-MIX-AMBIGUOUS-DEFAULT`); the class must define its own
  implementation to disambiguate.
- **OQ-5.** Should the `..rest` functional-update syntax for struct
  literals ship in v1? Recommendation: defer to v2 — the
  disambiguation rules around range expressions inside `{}` blocks
  are more nuance than v1 needs.
- **OQ-6.** Should the `actor` / `spawn` / `send` / `receive`
  keywords be unreserved if the actor model never ships? Status:
  reserved for v2 per §3.14. Re-evaluate at v2 planning.

---

## 10a. Migration mapping (working reference)

For the duration of the rename, the following equivalences apply.
Once §11 cleanup is done, this section is deleted along with any
mention of the prior forms.

| Prior form                          | New form                                                 |
|-------------------------------------|-----------------------------------------------------------|
| `let mut x = ...`                   | `var x = ...`                                             |
| `pub` prefix on item                | omit (public is default); use `private` / `protected` section markers |
| `trait T ... end`                   | `mixin T ... end`                                         |
| `impl T for U ... end` (block)      | `include T` directive in `U`'s body; methods scattered    |
| `impl U ... end` (inherent block)   | move methods into `U`'s body directly                     |
| `impl[T: B] C[T] ... end`           | `extension C[T] where T: B ... end`                       |
| `impl Drop ... end` inside class    | `include Drop` + `def var drop` in class body             |
| `&impl T` (param/return type)       | `&some T`                                                 |
| `&dyn T` (param/return type)        | `&any T`                                                  |
| `'a` lifetime sigil                 | bare lowercase identifier in `[...]`: `[a]`, `&a T`       |
| `@[derive(D1, D2)]`                 | DELETE — structural mixins are implicitly included (§3.6); user override wins; loud form is `include D1, D2` |
| `derive D1, D2` in-body             | `include D1, D2` (if you want the loud form); else DELETE  |
| `None` (`Option` variant constructor) | `nil`                                                   |
| `@[repr(C)]`                        | `layout c` at top of type body                            |
| `@[repr(packed)]`                   | `layout packed`                                           |
| `@[repr(transparent)]`              | `layout transparent`                                      |
| `@[link(name = "x")]`               | options on `lib`: `lib "x", ...`                          |
| `@[inline]`                         | `inline def f` modifier; or `inline :f` directive         |
| `@[...]` syntax overall             | retired                                                   |
| `@[test]` / `@[ignore]` / `@[should_panic]` | in-body `test` / `ignore` / `should_panic` directives in a test fn's body |
| `@[bench]`                          | in-body `bench` directive                                 |
| `@[deprecated]` / `@[stable]` / `@[unstable]` | in-body `deprecated` / `stable` / `unstable` directives |
| `@[no_std]`                         | package-level `no_std` directive (one line at the top of the package root) |
| `@[opt_out_send]` / `@[unsafe_impl_send]` | body-level `opt_out_send` / `unsafe_impl_send` directives |
| `extern "C" ... end`                | `lib "<linkname>" ... end`                                |
| `crate` (in path or keyword)        | `package`                                                 |
| `null` (FFI null literal)           | `nil`                                                     |
| `Vec[T]`                            | `Array[T]`                                                |
| `HashMap[K, V]`                     | `Map[K, V]`                                               |
| `HashSet[T]`                        | `Set[T]`                                                  |
| `Rc[T]`                             | `Shared[T]`                                               |
| `Arc[T]`                            | `SharedSync[T]`                                           |
| `[…]` macro                     | bare `[…]` literal — produces an `Array[T]`               |
| `{…}` macro                    | bare `{ k => v, … }` literal — produces a `Map[K, V]`     |
| `set!{…}` macro                     | `Set.from_iter([…])` (stdlib constructor; no dedicated literal — `{…}` is reserved for `Map`) |
| `::` path separator                 | `.` everywhere (`std.io`, `Color.Red`, `package.utils`)   |
| `extern "C" ... end` with no link name | `lib "c" ... end`                                      |
| tutorial language `&self` / `&mut self` | "reading method" / "writing method"                   |
| `Hash` (mixin name)                 | `Hashable`                                                |
| `Displayable` (mixin name)          | `Display`                                                 |
| `Comparable` (mixin name)           | `Ord` (full order) and/or `PartialOrd` (partial order)    |
| `Iterable` (mixin name)             | `Iterator` (one mixin covers both — a type that yields items via `def var next` is iterable) |
| `derive` keyword                    | DELETE — implicit-include for structural mixins (§3.6); loud form is `include D1, D2` |
| `@[derive(D1, D2)]` prefix attribute | same as above                                           |
| `T::method` qualified call          | `T.method`                                                |
| `'a` lifetime sigil in error text   | bare `a` (no sigil — error messages show the identifier as written) |
| `None` literal                      | `nil`                                                     |
| `null` literal (FFI)                | `nil` (the same token; context disambiguates per §3.10)   |
| `Hash[K, V]` (as collection alias)  | `Map[K, V]` only (`Hash` as a type alias for `Map` is retired — the noun is the mixin `Hashable`) |
| `HashSet[T]` / `HashMap[K, V]`      | `Set[T]` / `Map[K, V]` (the public names; internal type names may still appear in compiler source) |
| `File.read_string(path)`            | `fs.read_to_string(path)` (canonical stdlib spelling)     |
| `String.new(s)` (for converting from `&str`) | `String.from(s)` — `.new` is reserved for the no-arg / pre-allocated constructor |
| `def mut foo`                       | `def var foo` — method's receiver is writable                |
| `&mut T` (reference type)           | `&var T`                                                  |
| `*mut T` (raw pointer)              | `*var T`                                                  |
| `for x in &mut coll`                | `for x in &var coll`                                      |
| `iter_mut()` (method name)          | `iter_var()`                                              |
| `deref_mut()` (method name)         | `deref_var()`                                             |
| `FnMut(...) -> R` (closure type)    | `FnVar(...) -> R`                                         |
| `mut` keyword (anywhere)            | DELETE — `var` covers every "writable" position           |

## 11. Implementation handoff

The migration plan (which references both end-state and prior
forms for the duration of the work) lives in
`docs/specs/syntax/_migration-plan.md` and is deleted on
completion. After deletion, no document in the repo references
prior syntax forms — Ruxen source files, tutorials, requirements
docs, examples, and orchestration prompts all conform to this
spec.
