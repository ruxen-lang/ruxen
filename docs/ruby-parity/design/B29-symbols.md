# B29 design spec: symbols

Design document for batch **B29** in [`../QUEUE.md`](../QUEUE.md), enabler **E5**.
Design only: no Rust, no `.rx` implementation.

**Why now.** A Ruby developer cannot write idiomatic Ruxen without symbols. They
are the key type of every options hash, every `attr`-style declaration, every
ActiveRecord-shaped API, and every JSON key mapping. B44 (serialization) and the
folio ORM line assume them, so a wrong shape here propagates into every
downstream API.

**The headline, up front.** The probes below change what this batch is. The
single highest-value "symbol" feature in Ruby, `&:upcase`, **already works in
Ruxen today with no symbol type at all**, and three of Ruxen's `Hash` behaviours
are broken in ways that make the obvious `Symbol` designs unimplementable. B29 is
therefore not "add a Symbol type"; it is "add a compile-time-closed symbol
primitive, on top of a `Hash` that must be fixed first".

All probe results were produced on 2026-07-29 with
`./target/release/ruxen compile`, and every one is reproduced below with its
literal output.

---

## 1. Probe results

### 1.1 What `:name` does today

| Probe | Program | Result |
|---|---|---|
| **S1** | `let x = :name` | `error: expected expression, found Colon (at 3:11)`. Stops in the **parser**. The lexer has no symbol token at all (`grep -i "symbol\|intern" compiler/ruxen_core/src/lexer/token.rs` is empty), so `:` arrives as a bare `Colon` and no expression rule accepts it. |
| **S2** | `let h = { a: 1 }` | `error: expected expression, found Colon (at 3:14)`. Same failure, so hash shorthand is blocked on exactly the same token. |
| **S4** | `let a = %i[x y]` | `error: expected expression, found Percent`. `%w[...]` is implemented (pin 974), `%i` is not. |
| **S5** | `take(:name)` (symbol in argument position) | `error: expected RParen, found Identifier("name")`. **Different failure**, and it matters: in a call-argument list the `:` is consumed by the **named-argument** rule (`f(w: 3, h: 4)`, which is DONE), so `:name` collides with existing syntax rather than simply being unknown. This is the parser hazard of §6. |

**So: no lexing, no parsing, no type. `:sym` stops at the parser, and in argument
position it stops in a way that is entangled with named arguments.**

### 1.2 `&:sym` already works, with no symbol type (the important one)

```
let a = ["ab", "cd"]
let b = a.map(&:to_upper)     # prints AB then CD
```

**S3 compiles and runs correctly.** Reading the implementation explains why:
`parser/expr/calls.rs::try_parse_block_pass` special-cases the three-token
sequence `&` `:` `identifier` into `BlockPass(ToProc(name))`, and
`resolve/control_flow.rs` desugars that at **resolve time** into the closure
`{ |__proc_arg| __proc_arg.<method_name> }`.

No symbol value is ever created. The name is a compile-time string folded into a
direct method call. This is the tier-2 doctrine working exactly as the README
describes, and it is **strictly better** than Ruby's runtime `Symbol#to_proc`.

### 1.3 No interning or symbol-like machinery exists

`grep -rn "Symbol" compiler/ruxen_core/src/lexer/` is empty. There is no intern
table, no atom type, and no `library/std/symbol` package. The only symbol-shaped
thing in the compiler is `BlockPassKind::ToProc(String)`, which is an AST node
carrying a method name, not a value type.

### 1.4 What `Hash` requires of a key, and three bugs it exposes

This is where the design is actually decided. `library/std/hash/runtime/hash.c`
stores every key in an `int64_t` slot and decides **how to compare keys with a
memory-probing heuristic**, `ruxen_hash_looks_like_string`, resolved on the first
insert: if the key's bits look like a pointer to a NUL-terminated ASCII region,
the whole table switches to `strcmp` plus FNV-1a over the bytes; otherwise it
compares raw bits.

Three probes, three defects:

| Probe | Program | Result |
|---|---|---|
| **H1** | `var h = Hash[String, Int].new; h["a"] = 42` then read `h.size` | `after []=: size=0 key?=false`. **`Hash[k] = v` is a silent no-op.** `h.insert("b", 7)` works and `h["b"]` then reads back `7`, so the subscript **read** is fine and only the subscript **assign** is dead. Filed as **B72**. |
| **H2** | `h.insert(5000, 42)` on a `Hash[Int, Int]` | **SIGSEGV** (exit 139). Keys `7` and `4095` work; `5000` and `100000` crash. The heuristic short-circuits below `0x1000` and otherwise **dereferences the key value as a pointer**, so any integer key ≥ 4096 reads address 4096+. Filed as **B73**. |
| **H3** | a `struct` or `class` used as a `Hash` key | `h.insert(SymI.new(3), 99)` then `h.key?(SymI.new(3))` is **false**. User types are keyed by object identity; the `Hashable` mixin's `hash_code` and structural `==` are **never consulted** by the C table. Filed as **B73** (same root cause: the key-kind protocol). |

**Consequence for this batch: no user-defined `.rx` type can be a working `Hash`
key today.** Only `String` (via the strcmp path) and `Int` below 4096 (via the
raw-bits path) work at all. Since being a hash key is a symbol's whole reason to
exist, this constrains the representation more than any aesthetic argument.

### 1.5 What `String` and `Char` equality give a symbol for free

| Probe | Result |
|---|---|
| **E1** | `"x" == "x"` is `true`. String content equality works. |
| **E2** | `struct` structural equality works, **including a `String` field**: `SymS.new("x") == SymS.new("x")` is `true`. |
| **E3** | **`class` structural equality does not**: `C.new(1) == C.new(1)` is `false`, with or without an explicit `include PartialEq`, even for a single `Int` field. Pin `133_implicit_partial_eq.rx` covers a `struct`, which is why this has gone unnoticed. Filed as **B74**. |

So a symbol built as an `.rx` **struct** would get correct `==` for free, and one
built as a **class** would not. Neither can be a `Hash` key (§1.4).

---

## 2. Representation

### Decision

**`Symbol` is a compiler primitive: a 64-bit integer id into a static intern
table emitted into the binary at compile time. Its method surface lives in `.rx`
(`library/std/symbol`), exactly as `Int`'s and `String`'s do.**

### Why a primitive rather than an `.rx` type

The `.rx`-first rule requires proving a feature cannot be expressed in `.rx`. For
symbols the proof is three-part and each part is probed, not argued:

1. **The literal cannot come from `.rx`.** `:name` is surface syntax, and the
   north star explicitly places literals in the compiler ("syntactic protocols: a
   literal lowers to a *primitive*"). No `.rx` library can make `:` lex.
2. **An `.rx` type cannot be a `Hash` key** (H3). A symbol that cannot key a hash
   is not a symbol; it is a wordy `String`.
3. **An `.rx` `class` cannot even do `==`** (E3), and a `struct` that could still
   fails (2).

This is the same shape the north star sanctions elsewhere: build the one generic
primitive in the compiler, then define the *feature surface* in `.rx` on top. The
compiler learns a literal and a machine representation; `Symbol#to_s`,
`#length`, `#<=>`, `#succ`, `#inspect` are all `.rx`.

### Why an integer id rather than an interned pointer

An interned `char*` is the classic representation and it is **wrong here**, for a
reason that only shows up in this codebase:

- The `Hash` key-kind heuristic (§1.4) would classify an interned pointer as a
  **string key** and hash it **by its bytes**, walking the name on every lookup.
  That discards the entire point of interning: `Hash[Symbol, V]` would be exactly
  as fast as `Hash[String, V]`, never faster.
- Pointer values are not stable across runs, so any output that leaks ordering
  (iteration order, a `sort` by identity) becomes nondeterministic, which breaks
  the `expected/*.out` diff harness that every pin depends on.

An integer id takes the raw-bits fast path, is stable across runs, is trivially
`Copy`, needs no allocation, and compares in one instruction.

**But it is only safe once B73 lands**, because ids ≥ 4096 currently segfault as
hash keys (H2). Ids below 4096 would work today, and a 4096-symbol ceiling is not
a language. **B73 is therefore a hard prerequisite for B29c**, not a nice-to-have.

### Memory model, ownership, and the table

| Question | Answer |
|---|---|
| Who owns the table | The **binary**. It is a static, read-only array of name pointers plus lengths, emitted at compile time and indexed by id. No allocator, no lock, no teardown. |
| Per-process or per-thread | Neither, in the usual sense: it is immutable static data, so it is shared by every thread with no synchronisation, and `Symbol` is trivially `Send` and `Sync`. |
| Id assignment | Deterministic: sorted by name at link time so a given program always assigns the same ids, keeping output reproducible. |
| Size | One `i64`. A `Symbol` is a machine word, `Copy`, with no drop. |
| Compile-time versus runtime creation | **Every symbol exists at compile time. There is no runtime creation.** See §3. |

---

## 3. Interning, and the denial-of-service question

Ruby's symbols were a DoS vector for years: `params[:x].to_sym` on attacker
input created a permanently-live symbol, so a hostile client could exhaust memory.
Ruby's fix was to make dynamically created symbols garbage-collectable, which
costs a GC that Ruxen does not have and will not grow.

### Decision: the symbol set is closed at compile time

**No symbol can be created at runtime.** The table is static and read-only, and
`String#to_sym` **looks up** rather than interns:

```
def to_sym -> Option[Symbol]     # Some(sym) if this exact name was interned at
                                 # compile time, nil otherwise
```

This is a **deliberate, visible deviation from Ruby**, and it is the right one:

- It removes the DoS vector **by construction** rather than by mitigation. There
  is no unbounded table, so there is nothing to exhaust.
- It requires no GC, no refcount, and no lock, which matters because Ruxen's
  whole identity is ownership without a garbage collector.
- It matches the tier-2 doctrine the README already commits to: the set of
  symbols is closed at compile time in exactly the way the set of methods, types,
  and fields already is. A program that needs an open-ended interned string wants
  a `String`, and saying so is more honest than half-supporting it.
- `Option` return makes the failure explicit at the call site instead of silently
  minting garbage.

The cost, stated plainly: `"user_" + n.to_s` cannot become a symbol, and Ruby
code that round-trips arbitrary input through `to_sym` will not port unchanged.
That code is rare, usually a latent bug in the original, and always expressible
with a `String` key.

**`:"#{x}"` (dynamic symbol literals) follows from this and is refused**, with a
diagnostic pointing at `to_sym` and at using a `String`.

---

## 4. Equality, ordering, hashing, and `Hash` keys

| Operation | Semantics | Cost |
|---|---|---|
| `==` / `!=` | id comparison. Two symbols are equal exactly when their names are, because interning is total at compile time. | one integer compare |
| `<=>` | **by name**, matching Ruby (`:a < :b`). Implemented in `.rx` over `to_s`, so it costs a string compare and is not the fast path. | string compare |
| `hash` / as a `Hash` key | the id, through the existing raw-bits path. | one hash of an integer |
| `Hash[Symbol, V]` | works **only after B73**. Today it would segfault above 4095 symbols and is fragile below it. | |

Note the asymmetry, and keep it: equality is by id (fast, and the common case),
ordering is by name (correct, and rare). Ordering by id would be faster and
**wrong**, since ids are assignment-order artifacts.

---

## 5. The literal surface, and what is v1

| Surface | v1? | Note |
|---|---|---|
| `:name` | **yes** | The batch is pointless without it. |
| `:"quoted name"` | **yes** | Same lexer path, and it is the only way to name a symbol that is not an identifier. Still compile-time-closed, so no interpolation. |
| `{ a: 1 }` hash symbol-key shorthand | **yes** | The highest-value consumer, and the reason B29 is ranked where it is. |
| `%i[a b c]` | **yes** | Rides directly on the existing `%w[...]` implementation (pin 974). Cheap once `:name` exists. |
| `:+` and other operator symbols | **no, deferred** | Their real uses are `send(:+)` (refused, §7) and `reduce(:+)` (better served by the block form that already works). Deferring costs nothing. |
| `:"#{x}"` dynamic symbols | **no, refused** | Follows from §3. Refused with a diagnostic, not silently unsupported. |
| Hash patterns `in { a:, b: }` | **no, its own batch** | Belongs with B31 (pattern-matching completion), which has its own parser work. |

---

## 6. The parser hazard: `:` is already taken

Probe S5 is the warning. In Ruxen, `:` already means three things, and symbols
add a fourth:

1. **type ascription**: `let x: Int = 1`, `def f(a: T)`
2. **named arguments** at a call site: `f(w: 3, h: 4)` (DONE)
3. **ternary**: `c ? a : b` (DONE, pin 958, already whitespace-disambiguated from
   the `?` try operator)
4. **symbol literal**: `:name`

The collision that bites is (2) against (4): in `f(:name)` the argument parser
sees `:` and is already primed for `name:`. Probe S5's error
(`expected RParen, found Identifier("name")`) is that confusion, not a missing
rule.

**The disambiguation is mechanical but must be specified before code is written**,
because getting it wrong produces confusing errors in ordinary code rather than a
clean failure:

- A `:` **immediately followed by an identifier with no space**, in expression
  position, is a symbol. A named argument is `ident` `:` `value`, so the `:`
  follows an identifier rather than preceding one. The two are distinguishable by
  one token of lookbehind.
- Inside a hash literal, `{ a: 1 }` versus `{ :a => 1 }` must both parse, and both
  must produce the same thing.
- The ternary rule already uses whitespace to disambiguate, so **`c ? a : b` with
  spaces stays a ternary and `:b` attached is a symbol**, which is the same
  convention pin 958 established. Reusing that precedent is better than inventing
  a second rule.

This is why **B29a (lexer plus parser plus disambiguation) is the riskiest
sub-batch**, and why it should ship on its own with no semantics attached.

---

## 7. Interaction with B27 (kwargs) and B28 (Proc)

### Symbols land first, and kwargs does not actually depend on them

In Ruby, `**kwargs` is literally a `Hash` with `Symbol` keys, so the two look
inseparable. **In Ruxen they are not.**

> **Correction (2026-07-29).** An earlier version of this section said Ruxen
> "already has named arguments without symbols: `f(w: 3, h: 4)` is DONE", and
> concluded that B27 could be built on that machinery. **That was wrong.**
> Probing for the B27 spec found that named arguments **do not parse at all**
> (`sq(w: 3)` gives `error: expected expression, found Colon`), and there is no
> named-argument code in the parser. `language.md` §4's DONE row is false, and
> its "probe PASS" was almost certainly run inside a string interpolation, which
> is permissive. See [`B27-splat-kwargs.md`](B27-splat-kwargs.md) §1.2.

The conclusion below survives the correction, on a **stronger** argument than the
one it originally rested on:

- **Semantically, kwargs does not need symbols.** The typed-language answer to
  `**kwargs` is a typed struct, which is what `gems-web.md` already decided for
  strong params (`FromParams`), not a `Hash[Symbol, Any]` (Ruxen has no `Any`).
  A symbol-keyed hash cannot even *type* a realistic kwargs call, since
  `create(name: "ada", age: 36)` is heterogeneous and `Hash[Symbol, V]` has one
  `V`. So B27 builds a compile-time binding mechanism from scratch and never
  touches `Symbol`.
- **Syntactically, they collide** in exactly one place, §6's argument-position
  `:`. That collision does not exist *today* (nothing claims the colon in
  argument position yet), but B27 will create it, so the disambiguation still has
  to be designed once and is far easier to design with symbols in hand.

**Recommendation: B29a before B27.** Symbols first, because the parser rule is
shared and symbols are the constrained party. The rest of B29 can proceed in
parallel with B27.

### `&:sym` stays exactly as it is

Probe S3 shows `&:upcase` already works, desugared at resolve time into a
closure. **Do not route it through `Symbol#to_proc` when symbols land.** It would
be slower (a real symbol value, a real proc, a dynamic dispatch) and it would
regress a working, zero-cost feature to buy conceptual tidiness.

`Symbol#to_proc` as a *method on a symbol value* is a separate, rarely needed
thing. It requires B28's `Proc` and it is **deferred**. This means `&:sym` and
`Symbol#to_proc` are two mechanisms with one surface syntax, which is worth a
comment in the code so a future maintainer does not "unify" them.

---

## 8. `send` and dynamic dispatch: refused, explicitly

**`send(sym, ...)` with a symbol not known at compile time is out of scope, and
will stay out of scope.** Ruxen compiles to native code with no method table and
no metaobject, so there is nothing to look a name up in.

What is offered instead, and it covers the real uses:

- **A literal symbol folds to a direct call**: `x.send(:foo)` is `x.foo`. That is
  B34's job (compile-time reflection folding), and it is where the literal-symbol
  half of `send` lives.
- **A runtime symbol from a bounded set** becomes a generated `match` over the
  declared method set, also B34.

Refusing the unbounded case explicitly, with a diagnostic that names both
substitutes, is better than a half-working `send` that fails at link time on the
first name the compiler did not foresee. `core.md` already classifies it this way
("`send(name)` where name is unknown until runtime: OUT-OF-SCOPE"), and this spec
confirms it rather than reopening it.

---

## 9. Conversions

| Method | v1? | Shape |
|---|---|---|
| `Symbol#to_s` | yes | looks the name up in the static table, returns an owned `String` |
| `Symbol#inspect` | yes | `":name"` |
| `Symbol#length` / `#size` | yes | `.rx`, over `to_s` |
| `Symbol#<=>` | yes | `.rx`, by name (§4) |
| `Symbol#succ` / `#[]` / `#upcase` | deferred | Ruby has them, nothing needs them, and each would mint a new symbol, which §3 forbids. They would have to return `String`, which is a semantic deviation worth avoiding until asked for. |
| `String#to_sym` / `#intern` | yes | returns **`Option[Symbol]`**, per §3 |
| `Symbol#to_proc` | deferred | needs B28; `&:sym` already covers the surface (§7) |
| `Symbol#id2name` | yes | alias of `to_s`, one line |

---

## 10. Sub-batches

| Sub-batch | Goal | Depends on | Risk |
|---|---|---|---|
| **B29a: lexer and parser** | A `Symbol` token, `:name` and `:"quoted"` literals, and the §6 disambiguation against named arguments, type ascription, and the ternary. Parses to an AST node that **typeck rejects with a clear "symbols are not yet a type" error**, so the syntax lands with no semantics. | nothing | **Highest in the batch.** Four meanings of `:` in one grammar, and a mistake shows up as confusing errors in ordinary code that has nothing to do with symbols. |
| **B29b: the primitive and the table** | The `Symbol` machine type, the static compile-time intern table with deterministic id assignment, `==`, `hash`, and the `library/std/symbol` `.rx` surface (`to_s`, `inspect`, `length`, `<=>`, `id2name`). | B29a | Medium. |
| **B29c: hash shorthand and `%i`** | `{ a: 1 }` and `%i[a b]`, which is the batch's payoff. | B29b **and B73** | Medium, but **gated**: `Hash[Symbol, V]` cannot work until the key-kind protocol is fixed (§1.4), so shipping this before B73 would deliver a hash shorthand whose hashes are broken. |
| **B29d: conversions** | `String#to_sym` returning `Option[Symbol]` over the closed set, plus the deferred-method diagnostics that point at `String`. | B29b | Low. |
| **B29e: hash patterns** | `in { a:, b: }`. Belongs to **B31**, listed here only so it is not lost. | B29b, B31 | Medium, and not part of this batch's budget. |

**Ship order: B29a, B29b, then B29d, with B29c waiting on B73.**

**Pins:** B29 owns 1280-1289 under the QUEUE.md formula. That is enough for
B29a through B29d if each takes two or three pins. If it is not, take a fresh
block run from 1960 per the split-batch rule in QUEUE.md §0.

---

## 11. What this batch does not do

- No runtime symbol creation, no dynamic symbol literals (§3).
- No `send` by runtime name (§8).
- No `Symbol#to_proc` in v1; `&:sym` already covers the surface (§7).
- No operator symbols (`:+`) in v1 (§5).
- No hash patterns; that is B31 (§10).
- **No change to `&:sym`.** It works, it is faster than a symbol-based
  implementation, and it stays.

---

## 12. Open questions

1. **Where the intern table is emitted.** A static array in the binary is the
   obvious answer, but cross-package compilation means two packages can each
   intern `:name` and must agree on the id. Options: assign ids at link time
   (deterministic, needs a link-time pass), or key symbols by a hash of the name
   (no coordination, but collisions are catastrophic rather than merely
   surprising). Recommend link-time assignment, and probe how the existing
   cross-package machinery in `docs/decisions/q17-cross-package-monomorphization.md`
   handles the analogous problem before committing.
2. **Whether `Symbol` should be `Comparable`.** `<=>` by name is correct but
   costs a string compare, which is surprising in a type that advertises itself
   as cheap. Alternative: provide `<=>` but not `include Comparable`, so sorting
   symbols is possible but never accidental.
3. **Whether `to_sym` returning `Option` is too annoying in practice.** The
   alternative is a `Result` with a message naming the closed-set rule, which is
   friendlier to debug and heavier to use. Worth revisiting once a real consumer
   (folio's column names) exists.
4. **`{ "a" => 1 }` versus `{ a: 1 }` key types.** Once both parse, a hash
   literal mixing them is `Hash[?, V]`. Recommend rejecting mixed-key literals
   with a clear diagnostic rather than inferring a union, since Ruxen has no
   union type.
5. **Whether the deviation in §3 should be louder.** A program that ports Ruby
   code calling `to_sym` on input gets an `Option` and a compile error at the
   unwrap. That is the correct outcome, but it may deserve a dedicated `E####`
   with a written explanation rather than a plain type error.

---

*Written 2026-07-29 for QUEUE.md batch B29 (enabler E5). Every probe in §1 was
run against `./target/release/ruxen compile` on that date. Three probes uncovered
pre-existing defects now filed as B72, B73, and B74, and B73 is a hard
prerequisite for B29c. No implementation exists yet.*
