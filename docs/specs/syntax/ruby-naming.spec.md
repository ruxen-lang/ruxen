# Spec — Riven Surface Syntax (Ruby Flavor)

Status: Draft (2026-05-15)
Owner: language
Scope: end-state surface syntax — keywords, declaration forms,
       visibility, mixins, attributes, stdlib type names.
       Internal compiler naming (HIR/MIR/codegen) follows the same
       vocabulary but is incidental — covered in implementation
       phases, not here.

This document is the **canonical source** for what Riven looks like
on the page. Every tutorial, requirements doc, fixture, and example
conforms to the forms below.

---

## 1. Motivation

Riven's tagline is *reads like Ruby, compiles like Rust*. The first
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

- **G1.** A Ruby developer can read any Riven source file without a
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
  where Riven made a deliberate semantic departure (`Option`,
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

```riven
let name = "Riven"
let pi = 3.14

var counter = 0
counter = counter + 1
counter += 1
```

Reassignment to a `let` is a compile error.

Type annotations attach the same way:

```riven
let x: Float = 42
var bytes: Array[UInt8] = Array.new
```

### 3.2 Visibility

Public by default. Section markers within `class`, `struct`,
`module`, and `mixin` bodies gate subsequent declarations:

```riven
class Account
  balance: Int                # public field

  def init(@balance: Int) end

  def get_balance -> Int      # public method
    self.balance
  end

  private

  def normalize_balance       # private — until next marker
    self.balance.max(0)
  end

  protected

  def admin_dump -> String    # protected — subclass-visible
    "Account(#{self.balance})"
  end
end
```

Field visibility uses the same markers:

```riven
class User
  name: String                # public field

  private
  audit_id: Int               # private field
end
```

The Ruby `private :method_name` alternate form is also accepted at
the end of the body for forward-declared visibility (parser
preserves source order, applies as a final pass).

### 3.3 Lifetimes

Lifetimes are generic parameters in the same `[...]` slot as types.
**Lowercase identifier = lifetime, uppercase = type.** No sigil.

```riven
def longest[a](x: &a String, y: &a String) -> &a String
  if x.len > y.len; x; else; y; end
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

### 3.4 Mixins

A `mixin` is the contract-and-provision unit. Methods inside a
mixin body come in two flavors:

- **Required** — signature with no body. Including class must
  provide.
- **Default** — signature with body. Including class gets the body
  for free, may override.

```riven
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

```riven
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

Multiple includes stack; order is source order. If two included
mixins both provide defaults for the same method name, the later
`include` wins; the class's own definition still beats both. A
required method may be satisfied by any included mixin's default.

Mixins support inheritance: `mixin Sortable: Comparable` means
`Sortable`'s contract extends `Comparable`'s. A class that
`include Sortable` must satisfy both.

Mixins support associated types:

```riven
mixin Iterator
  type Item
  def mut next -> Option[Self.Item]
end
```

#### Existentials: `some Mixin`, `any Mixin`

A function parameter or return type may reference a mixin in one of
two ways:

**`some Mixin`** — the compiler picks one concrete conforming type
per call site. The function body is monomorphized; the receiver
type is not visible to callers but is fixed for any given call.
Zero runtime cost. Methods may inline.

```riven
def print_it(item: &some Greetable)
  puts item.greet
end

print_it(&User.new("Alice"))   # specialized for User
print_it(&Robot.new(42))       # specialized for Robot
```

**`any Mixin`** — the value carries a vtable at runtime; one
function body handles all conforming types. Required for
heterogeneous collections.

```riven
def shout_all(crowd: &Array[Box[any Greetable]])
  for member in crowd
    puts member.greet.upcase
  end
end
```

Coercions `&T → &some Mixin` and `&T → &any Mixin` are implicit at
assignment and call boundaries when `T` includes the mixin.
`Box[T] → Box[any Mixin]` is the unsized owning coercion.

A mixin is *object-safe* (usable through `any`) if every method
satisfies: no `Self`-by-value in arg/return, no per-method generic
parameters, no class-method (`def self.foo`) entries, no
`consume self` receiver. Associated types must be bound at the use
site: `any Iterator[Item = Int]` is OK; `any Iterator` is not.

Structural satisfaction is accepted for `some Mixin` only. `any
Mixin` requires an explicit `include Mixin` directive in the
implementing class.

### 3.4a Methods on a type

A class, struct, or enum's methods live inside the type body — there
are no separate "methods-for-this-type" blocks for the common case:

```riven
class Container[T]
  items: Array[T]

  def init
    self.items = Array.new
  end

  def count -> Int
    self.items.len
  end
end
```

For methods that should exist **only when a type parameter satisfies
a bound**, use an `extension` block:

```riven
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

```riven
class SafeBuffer
  ptr: *mut UInt8
  len: USize

  def init(size: USize)
    unsafe
      self.ptr = malloc(size) as *mut UInt8
      self.len = size
    end
  end

  include Drop

  def mut drop
    unsafe
      free(self.ptr as *Void)
    end
  end
end
```

`include Drop` declares the type as a `Drop` participant. The
mixin requires a `def mut drop` method; the class provides one.
Same `include` directive, same scattered-method rule as §3.4.

### 3.5 Layout directives

`struct` bodies may carry a `layout` directive at the top of the
body. Three forms:

```riven
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

### 3.6 Derives

Derived trait impls are declared with `derive` inside the type body:

```riven
struct Point
  x: Int
  y: Int
  derive Debug, Clone, PartialEq
end

enum Color
  Red
  Green
  Blue
  derive Debug, Clone, Eq, Hash
end
```

The set of derivable mixins is `Debug`, `Clone`, `Copy`, `PartialEq`,
`Eq`, `Hash`, `Default`, `Ord`, `PartialOrd`.

### 3.7 FFI

External libraries are declared with `lib`. The string after `lib`
is the link name. Options follow as keyword arguments:

```riven
lib "m"
  def sin(x: Float) -> Float
  def cos(x: Float) -> Float
end

lib "sqlite3", version: "3"
  def sqlite3_open(path: *UInt8, db: *mut *Void) -> Int32
end

lib "c", path: "/usr/lib"
  def malloc(n: USize) -> *mut Void
end
```

No separate `extern` block keyword. Every FFI declaration goes
inside a `lib "..." ... end` block.

Calls into a `lib` block are inherently unsafe (the compiler can't
verify the C side); they must appear inside `unsafe` blocks.

### 3.8 Inline

`inline` is a modifier on a `def` declaration:

```riven
inline def fast_path(x: Int) -> Int
  x * 2 + 1
end
```

Or, Ruby-style, a directive that names a previously defined method:

```riven
def fast_path(x: Int) -> Int
  x * 2 + 1
end
inline :fast_path
```

`inline` is a hint, not a guarantee. The codegen backend treats it
as `alwaysinline` when LLVM is wired (v2); Cranelift currently
ignores it.

### 3.9 Packages

The compilation unit is a **package**. `package` is the keyword
that refers to "this package" in import paths:

```riven
use package.utils.format
use package.models.User
```

(`use` is unchanged from before — it's the same `use` Ruby and
Swift readers expect for namespace imports.)

### 3.10 Nil

`nil` is the raw-pointer null literal. Valid only at `*T` / `*mut T`
types, which themselves only appear in `unsafe` and FFI contexts:

```riven
unsafe
  let ptr: *mut UInt8 = nil
  if some_ptr == nil
    return Err("got null")
  end
end
```

Using `nil` where a non-pointer type is expected is a type error.
Riven references (`&T`, `&mut T`) cannot be `nil` — they are always
valid by construction. For optional values, use `Option[T]` with
`Some(value)` / `None`; `nil` is **not** an `Option` value.

A `vec!`-style array literal macro for these types is spelled
`array![...]` (see §4.4).

### 3.11 Stdlib type names

| Type            | Role                                     |
|-----------------|-------------------------------------------|
| `Array[T]`      | Growable heap-allocated sequence          |
| `Set[T]`        | Hash-based unique set                     |
| `Map[K, V]`     | Hash-based key-value map                  |
| `Option[T]`     | `Some(v)` or `None` — replaces nullability |
| `Result[T, E]`  | `Ok(v)` or `Err(e)` — replaces exceptions |
| `Box[T]`        | Owning heap pointer                       |
| `Shared[T]`     | Reference-counted, single-threaded        |
| `SharedSync[T]` | Atomically reference-counted, thread-safe |
| `String`        | Owned, heap-allocated, growable string    |
| `&str`          | Borrowed string slice (UTF-8 view)        |

Stdlib import paths and constructor names follow:

```riven
let words: Array[String] = Array.new
let seen: Set[Int] = Set.new
let counts: Map[String, Int] = Map.new
let boxed: Box[Point] = Box.new(Point.new(1, 2))
let shared: Shared[Counter] = Shared.new(Counter.new)
let cross: SharedSync[State] = SharedSync.new(State.new)
```

### 3.12 Self-mode terminology

A method's relationship to its receiver is one of three modes:

| Form              | Mode       | Meaning                              |
|-------------------|------------|--------------------------------------|
| `def m`           | reading    | Borrows the receiver immutably.      |
| `def mut m`       | mutating   | Borrows the receiver mutably.        |
| `def consume m`   | consuming  | Takes ownership; receiver is moved.  |
| `def self.m`      | class      | No receiver — module-style call.     |

Tutorials and specs describe these in mode terms ("a reading
method", "a mutating method", "a consuming method"). The internal
references-to-self notation is not user-facing.

### 3.13 The `!` macro suffix

Methods ending in `!` follow Ruby's convention for danger/mutation:

```riven
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
| Modes          | `mut`, `consume`, `inline`                                               |
| Control flow   | `if`, `elsif`, `else`, `match`, `while`, `for`, `in`, `loop`, `do`, `end`, `break`, `continue` |
| Type system    | `where`, `as`, `some`, `any`, `derive`, `layout`, `include`, `extension` |
| Visibility     | `private`, `protected`                                                   |
| Modules        | `module`, `use`, `package`                                               |
| Safety         | `unsafe`                                                                 |
| Literals       | `true`, `false`, `nil`, `None`, `Some`, `Ok`, `Err`                      |
| FFI            | `lib`                                                                    |
| Async (reserved) | `async`, `await`, `spawn`, `actor`, `send`, `receive`                  |

---

## 4. Worked examples

### 4.1 A class with a mixin and visibility

```riven
mixin Comparable
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

  include Comparable

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

### 4.2 Heterogeneous collection with `any`

```riven
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

```riven
lib "c"
  def malloc(n: USize) -> *mut Void
  def free(p: *mut Void)
end

lib "ssl", version: "3"
  def SSL_new(ctx: *mut Void) -> *mut Void
  def SSL_free(s: *mut Void)
end

def allocate(size: USize) -> Result[*mut UInt8, AllocError]
  unsafe
    let raw = malloc(size)
    if raw == nil
      Err(AllocError.OutOfMemory)
    else
      Ok(raw as *mut UInt8)
    end
  end
end
```

### 4.4 Generic function with bound and lifetime

```riven
def first_match[T, a](haystack: &a Array[T], pred: |&T| -> Bool) -> Option[&a T]
  for item in haystack
    if pred(item); return Some(item); end
  end
  None
end

let words = array!["alpha", "beta", "gamma"]
let found = first_match(&words, |w| w.starts_with("b"))
```

### 4.5 Derive + layout

```riven
struct Header
  layout c
  magic: UInt32
  version: UInt16
  flags: UInt16
  payload_len: UInt64
  derive Debug, Clone, Copy
end
```

---

## 5. Diagnostics surface

Error code naming follows the new vocabulary. The migrations are:

- The mixin-existential codes are `E-ANY-*` (object safety, missing
  assoc binding, multi-mixin, GAT, etc.). The `some`-side codes
  are `E-SOME-*`.
- Layout errors are `E-LAYOUT-*` (`E-LAYOUT-TRANSPARENT-MULTI`,
  `E-LAYOUT-PACKED-BORROW`).
- Derive errors keep their existing E06xx numbering but message
  text uses the new vocabulary.
- Visibility errors are `E-VIS-PRIVATE`, `E-VIS-PROTECTED`.
- Lifetime errors are `E-LIFE-*` (no sigil change in error
  messages — lowercase generic param names appear as-is).

Specific code allocations land in the diagnostics module during
implementation; this spec fixes the prefixes only.

---

## 6. Object-safety rules

A mixin is object-safe (usable through `any`) iff every method
satisfies:

- **S1.** No `Self`-by-value in arg or return position. `&Self`,
  `&mut Self`, `Box[Self]`, `&any Mixin` are fine.
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
| 0    | drop       | fn(*mut u8) — drop glue for the value    |
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
- T-PARSE-09: `derive Debug, Clone` inside type body parses.
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
- T-SEM-08: `derive Hash` on type with non-`Hash` field rejected.
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
- T-E2E-05: A `derive Clone` struct round-trips through `.clone()`.

---

## 10. Open questions

- **OQ-1.** Should `inline` accept a numeric hint (`inline 3`,
  cost-based)? Defer to v2.
- **OQ-2.** Should `nil` be the same token whether in `unsafe`
  context or general code, and just produce a type error outside
  unsafe? Recommendation: yes, single token, context-sensitive
  acceptance.
- **OQ-3.** Should there be a way to spell "this mixin's default
  method even though I overrode it"? Ruby has `super`. Riven
  already reserves `super` for class-inheritance superclass
  dispatch — reusing it for mixin defaults could conflict.
  Recommendation: `super` inside an override calls the mixin's
  default if the class has no superclass override; otherwise
  superclass wins. Spec out separately if it bites.
- **OQ-4.** When two included mixins both provide a default for
  the same method name and the class does not override, is that
  an error or does "later wins"? Recommendation: error
  (`E-MIX-AMBIGUOUS-DEFAULT`), require the class to choose by
  defining its own method. Less surprising than implicit ordering.

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
| `impl Drop ... end` inside class    | `include Drop` + `def mut drop` in class body             |
| `&impl T` (param/return type)       | `&some T`                                                 |
| `&dyn T` (param/return type)        | `&any T`                                                  |
| `'a` lifetime sigil                 | bare lowercase identifier in `[...]`: `[a]`, `&a T`       |
| `@[derive(D1, D2)]`                 | `derive D1, D2` in type body (existing form, only form)   |
| `@[repr(C)]`                        | `layout c` at top of type body                            |
| `@[repr(packed)]`                   | `layout packed`                                           |
| `@[repr(transparent)]`              | `layout transparent`                                      |
| `@[link(name = "x")]`               | options on `lib`: `lib "x", ...`                          |
| `@[inline]`                         | `inline def f` modifier; or `inline :f` directive         |
| `@[...]` syntax overall             | retired                                                   |
| `extern "C" ... end`                | `lib "<linkname>" ... end`                                |
| `crate` (in path or keyword)        | `package`                                                 |
| `null` (FFI null literal)           | `nil`                                                     |
| `Vec[T]`                            | `Array[T]`                                                |
| `HashMap[K, V]`                     | `Map[K, V]`                                               |
| `HashSet[T]`                        | `Set[T]`                                                  |
| `Rc[T]`                             | `Shared[T]`                                               |
| `Arc[T]`                            | `SharedSync[T]`                                           |
| `vec![...]` macro                   | `array![...]`                                             |
| tutorial language `&self` / `&mut self` | "reading method" / "mutating method"                  |

## 11. Implementation handoff

The migration plan (which references both end-state and prior
forms for the duration of the work) lives in
`docs/specs/syntax/_migration-plan.md` and is deleted on
completion. After deletion, no document in the repo references
prior syntax forms — Riven source files, tutorials, requirements
docs, examples, and orchestration prompts all conform to this
spec.
