# B27 design spec: splat parameters and keyword arguments

Design document for batch **B27** in [`../QUEUE.md`](../QUEUE.md), enabler **E7**.
Design only: no Rust, no `.rx` implementation.

**Why now.** Keyword arguments are the shape of nearly every modern Ruby API
(`def initialize(name:, age: 0)`), and the whole folio surface (`where(name:
"ada")`, `create!`) and rondo surface assume them. Agents have already hit the
absence: the String batch could not spell `start_with?(*prefixes)` and shipped an
array form instead.

**The headline.** The probes below found that **none of the prerequisite surface
exists, including the part that is documented as DONE**. `language.md` §4 lists
"Named-argument call `f(w: 3, h: 4)`" as **DONE (probe PASS)**. It does not
parse, in any position tested, and there is no named-argument code in the parser
at all. The original "probe PASS" was almost certainly run inside a string
interpolation, which the same file documents as permissive. So B27 is not
"extend named arguments to keyword arguments"; it is **build the keyword-call
surface from nothing**.

All probes were run on 2026-07-29 with `./target/release/ruxen compile`, each in
its own source file (B75: the compile cache does not key on stdlib identity).

---

## 1. Probe results

### 1.1 Nothing in the parameter-list surface exists

| Probe | Program | Result |
|---|---|---|
| **K1** | `def f(*args)` | `error: expected expression, found RParen`. No positional splat. |
| **K2** | `def f(**opts)` | `error: expected expression, found RParen`. No keyword splat. |
| **K4** | `def f(a:)` | `error: expected type, found RParen`. No required-keyword parameter. |

### 1.2 Named arguments do not work, and the matrix says they do

| Probe | Program | Result |
|---|---|---|
| **K17** | `let b = box(w: 2, h: 3)` | `error: expected expression, found Colon` |
| **K18** | `let a = sq(w: 3)` (a **single** named argument) | `error: expected expression, found Colon` |
| **K19** | `show(w: 2, h: 3)` as a bare statement | `error: expected expression, found Colon` |
| **K15** | `let c = box(h: 3, w: 2)` (out of order) | `error: unexpected ','` |
| grep | `named_arg`, `NamedArg`, `arg_name` across `compiler/ruxen_core/src/parser/` | **no matches** |

**`language.md` §4's "Named-argument call `f(w: 3, h: 4)` | DONE | probe PASS" is
wrong.** Note the row cites "probe PASS" rather than a case pin, which is itself
the tell: the file's own process rule (README rule 4) is that a row never becomes
DONE without a `tests/release-e2e/` case, and there is no named-argument pin.

**How the false PASS happened, and the lesson.** Probe **K13** ran the same call
inside an interpolation, `puts "#{box(h: 3, w: 2)}"`. It **compiled and ran**,
printing three blank lines. `language.md` §11 already warns that "several
reflection calls appear to compile inside string interpolation because the
interpolation re-lexer is permissive, then parse-fail standalone and emit empty
output at runtime". That caution is filed under metaprogramming; it is actually a
**general hazard for any probe**. Never establish a language fact from inside
`"#{...}"`; a blank line is the signature of this trap, not of a working feature.

### 1.3 `f(*xs)` at a call site is a silent no-op

**K16:**

```
def first_of(a: Array[Int]) -> Int
  a[0]
end
let xs = [7, 8]
first_of(*xs)        # compiles, prints 7
```

The `*` is **silently discarded** and the array is passed as one argument. In
Ruby `first_of(*[7, 8])` means `first_of(7, 8)`. So a Ruby developer writing a
splat call today gets no error and no splat. Compare **K3**, `f(*xs)` against a
two-parameter `f`, which reports `function f expects 2 arguments, got 1`,
confirming the splat contributed exactly one argument.

**This must become an error even if B27 slips**, on the "no silent wrong answer
for a user-callable surface" rule in the root `CLAUDE.md`.

### 1.4 A defaulted parameter destroys a trailing block (new bug, B76)

| Probe | Call | Result |
|---|---|---|
| **K7** | `show do \|n\| … end` where `def show(prefix: String = "v=", &block: …)` | **`ruxen panic: yield called without a block in 'show'`** |
| **K9** | `show() do \|n\| … end`, same signature | **same panic**. Parens do not help. |
| **K10** | `show("p=") do \|n\| … end`, default supplied explicitly | works: `prefix=[p=]`, `n=1` |
| **K8** | `show("v=") do \|n\| … end` with **no default** in the signature | works |

**Rule: whenever a defaulted parameter is actually defaulted, a trailing
`do…end` block is lost.** This is distinct from B71 (which is paren-less-only and
concerns the default's *value*); here the block is lost with or without parens.
Filed as **B76**. `def each(sep = ", ", &block)` is an everyday Ruby shape, so
B27 must not be built on top of this.

### 1.5 B71 reproduces on a builtin head

**bh2**, a `String` method whose only parameter is defaulted:

```
s.probe_all_default     # [ab]      <- default vanished
s.probe_all_default()   # [a,b,]    correct
s.probe_all_default("|")# [a|b|]    correct
```

This confirms B71's predicted exposure surface ("methods whose arguments are all
defaulted, called Ruby-style without parens") on a real builtin head, and
confirms the corollary that `ljust`/`rjust`/`center` are safe because they take a
required argument.

### 1.6 Overload by arity already exists and works

**K11** and **K12**: two `def f` at different arities resolve correctly, for both
free functions (`f(1)` → 1, `f(1, 2)` → 3) and class methods. **This is the one
piece of relevant machinery that does exist**, and it constrains the design
(§5).

---

## 2. Keyword arguments: a distinct compile-time mechanism, not a hash

### Decision

**Keyword arguments are a call-site binding resolved entirely at compile time:
names map to parameter slots. There is no runtime hash, no `Symbol` dependency,
and no allocation.**

### Why not sugar over `Hash[Symbol, V]`

The Ruby model is that `**kwargs` is literally a hash with symbol keys, so
mirroring it looks natural. Three reasons it is wrong here, the first decisive:

1. **A homogeneous hash cannot type a realistic kwargs call.** `Hash[Symbol, V]`
   has one value type. `create(name: "ada", age: 36)` mixes `String` and `Int`.
   Ruxen has no `Any` and should not grow one, so the hash cannot express the
   ordinary case. This is not a performance objection, it is an expressiveness
   one, and it ends the argument.
2. **It would chain two dependencies onto every keyword call**, B29 (symbols, not
   built) and B73 (the `Hash` key protocol, currently a crash bug where an
   integer key ≥ 4096 SIGSEGVs). Keyword arguments would then be blocked behind a
   runtime defect, for no gain.
3. **It pays an allocation for a static fact.** The mapping from `name:` to a
   parameter slot is known at compile time in every call Ruxen can compile.
   Building a heap hash to communicate it, then tearing it down, is exactly the
   runtime-metaobject cost the README's tier-2 doctrine exists to avoid.

**Consequence: B27 does not depend on B29 semantically.** This matches the
conclusion in [`B29-symbols.md`](B29-symbols.md) §7, though it strengthens it:
that spec argued kwargs could be built on the existing named-argument machinery,
and probe K17 shows **no such machinery exists**. The conclusion survives, the
supporting claim does not, and `B29-symbols.md` §7 has been corrected.

They still interlock at **one point, the parser**: `:` will mean type ascription,
a symbol literal, a keyword argument, and the ternary. That disambiguation must
be designed once. **B29a still lands first**, because symbols are the constrained
party and retrofitting the rule afterwards is worse.

---

## 3. What to refuse

### `**opts` with arbitrary runtime keys: refused

A keyword splat that accepts keys unknown at compile time requires a
heterogeneous runtime map, which requires `Any`, which requires either boxing
with runtime type tags or a dynamic type system. Ruxen has neither and should
acquire neither for this.

**The substitute already exists and is better**: a typed options struct. That is
precisely what `gems-web.md` chose for strong params (`FromParams`) and what B33's
field-walk substrate generates. `create(**opts)` becomes `create(opts: CreateOpts)`,
where a missing field is a compile error rather than a runtime `nil`.

Refuse it with a diagnostic that names the substitute, not with a parse error.

### Heterogeneous `*args`: refused

Ruby's `*args` is a heterogeneous list. Ruxen's is `Array[T]` for one `T`. Every
real consumer in the queue is homogeneous (`start_with?(*prefixes)`,
`values_at(*idx)`, `puts a, b`, `Set[a, b, c]`), so the restriction costs almost
nothing and keeps the ABI honest.

The exception worth naming is `format("%s is %d", name, age)`, which is
genuinely heterogeneous. **Recommendation: `format` takes an explicit
`Array[FormatArg]`**, an enum over the formattable types, rather than bending the
splat to carry mixed types. It is more typing at the call site and it is checkable.

### `send`-style dynamic keyword dispatch: refused

Already refused in `B29-symbols.md` §8; restated so B27 does not reopen it.

---

## 4. Splat representation and ownership

`def f(*args: Int)` gives the callee `args: Array[Int]`, populated from the
call's trailing positional arguments.

| Question | Answer |
|---|---|
| Who materialises the pack | **The caller.** It knows the count statically at every call site Ruxen can compile. |
| How it is passed | **By borrow** (`&Array[T]`), so the callee never owns and never frees. |
| Who frees it | The caller, at the end of the enclosing statement, through ordinary scope-exit drop. |
| Where it lives | **Stack slice preferred**, heap fallback. The north star already lists "byte-slice, array-slice" among the permitted machine primitives, so a stack-allocated slice needs no new concept. |
| What if the callee stores it | It cannot, through a borrow. Storing requires `.clone` at the callee, which is explicit and visible. |

**Open question (§8.1):** whether escape analysis exists to choose stack versus
heap automatically, or whether v1 should simply always heap-allocate the pack and
optimise later. Recommendation: **always heap in v1**, because a wrong escape
analysis is a use-after-free and the allocation is not on any hot path the queue
currently cares about. Correct first.

**Forwarding.** With `**opts` refused, forwarding reduces to `*args, &blk`
passthrough, which the above supports directly: the borrowed pack is re-splatted
into the inner call. A Ruby-style `...` shorthand is **not v1**; it saves eight
characters and adds a parse rule.

---

## 5. Interaction with arity overloading

Overload by arity **exists and works** (K11, K12), and a splat makes arity
variadic, so the two collide. `f(a: Int)` and `f(*args: Int)` are both candidates
for `f(1)`.

Resolution rules, in order:

1. **An exact-arity match always wins.** `f(1)` picks `f(a: Int)`.
2. **A splat overload is the fallback**, chosen only when no exact arity matches.
3. **Two splat overloads for the same receiver are an error at definition**, not
   an ambiguity at the call site, so the diagnostic points at the declaration.
4. **A defaulted parameter already creates a range of arities.** `f(a, b = 1)`
   accepts 1 and 2, so a splat overload overlapping that range is an error at
   definition too.

Rule 4 is the one most likely to be missed, and it is the one that will bite:
defaults and splats both widen arity, and the existing overload resolver was
built when arity was a single number.

---

## 6. Keyword parameters: required, optional, and the missing-key error

| Form | Meaning |
|---|---|
| `def f(a: Int)` | positional, as today |
| `def f(a: Int = 1)` | positional with a default, as today |
| `def f(a:: Int)` or another spelling | **required keyword**: must be passed by name |
| `def f(a:: Int = 1)` | optional keyword |

**Open question (§8.2): the spelling.** Ruby writes `def f(a:)` and
`def f(a: 1)`, but Ruxen already uses `a: Int` for type ascription, so `def f(a:)`
collides head-on with a parameter whose type was omitted. The options are a
doubled colon (`a:: Int`), a keyword marker before the list, or requiring
keyword-ness at the call site only (any parameter is callable by name, which is
what `language.md` claims today and what does not actually work). **Recommend
deciding this before B27a**, because it is the surface every downstream API
inherits and it cannot be changed later without breaking every caller.

**The missing-required-keyword error** is a compile error, with its own code,
naming the parameter and the call site. It must not be a runtime `nil`, which is
the Ruby behaviour worth *not* copying.

---

## 7. `&block` composition

Parameter order is fixed: **positional, then defaulted, then `*splat`, then
`&block`.** The block is not part of the arity count, so it never participates in
overload resolution.

**B76 blocks this.** Today a defaulted parameter already destroys a trailing
block (§1.4), so building splat and keyword handling on the same call path would
inherit the defect and make it harder to isolate. **Fix B76 first**, and treat
its pins as B27's regression net.

---

## 8. Open questions

1. **Stack slice versus heap for the splat pack** (§4). Recommend heap in v1;
   correctness before allocation count.
2. **The required-keyword spelling** (§6). The most consequential open question
   in this spec, because it is permanent surface.
3. **Whether every parameter should be callable by name**, as `language.md`
   currently (wrongly) claims, or only ones declared as keywords. Ruby's answer
   is "only declared ones", and permitting all of them makes every parameter name
   part of the public API, so renaming a parameter becomes a breaking change.
   **Recommend Ruby's answer**, and note that this is a deliberate narrowing of
   what the matrix claimed.
4. **Whether `format`/`sprintf` should wait for a `FormatArg` enum** (§3) or ship
   with a homogeneous `Array[String]` and `to_s` at the call site. The latter is
   uglier and available immediately.
5. **Whether the splat pack should be `Array[T]` or a distinct `Slice[T]`.**
   `Array[T]` reuses everything; a slice type would avoid promising the callee an
   owned, growable collection it must not grow.

---

## 9. Sub-batches

| Sub-batch | Goal | Depends on | Risk |
|---|---|---|---|
| **B27a: `*args` parameters** | Parse `*name: T` in a parameter list, the pack ABI (§4), and the arity-overload resolution rules (§5). Plus turning today's silently-ignored `f(*xs)` call-site splat into a real splat. | **B76** (§7) | **Highest for blast radius.** It changes the call ABI and touches the overload resolver, which every call in the workspace goes through. |
| **B27b: call-site splat and forwarding** | `f(*xs)` against an ordinary parameter list, and `*args, &blk` passthrough. | B27a | Medium. |
| **B27c: keyword parameters and keyword calls** | The §6 spelling, required versus optional, call-site binding, the missing-required diagnostic, and the parser disambiguation against symbols, type ascription, and the ternary. | **B29a** (the shared `:` rule), B27a | **Highest for design risk.** There is no existing machinery (K17), the surface is permanent, and four grammar constructs compete for one token. |
| **B27d: the refusals** | `**opts` and heterogeneous `*args` rejected with diagnostics that name the typed-struct substitute. | B27a, B27c | Low, and it should ship **with** B27a rather than after, so the refusal is never a confusing parse error. |
| **B27e: first consumers** | `format`/`sprintf`, `puts a, b`, `String#start_with?(*prefixes)` replacing the array form the String batch had to ship, `values_at(*idx)`, `Set[a, b, c]`. | B27b | Low. This is where the batch proves itself. |

**Ship order: B76, then B27a with B27d, then B27b, then B27c (after B29a), then
B27e.**

**Pins:** B27 owns 1260-1269. If the five sub-batches need more, take a fresh
block run from 1960 per the split-batch rule in QUEUE.md §0.

---

## 10. Corrections this spec makes

- **`language.md` §4**, "Named-argument call `f(w: 3, h: 4)` | DONE | probe PASS":
  **wrong**, named arguments do not parse (K17, K18, K19) and no parser code
  exists. Recorded in QUEUE.md rather than edited here, since the matrix files
  are held by other agents.
- **`B29-symbols.md` §7**, which claimed Ruxen "already has named arguments
  without symbols" and that B27 could be built on that machinery: corrected in
  the same commit as this spec.
- **`language.md` §11's interpolation caution** is filed as a metaprogramming
  note but is a general probing hazard (§1.2). Recorded in `HANDOFF.md`.

---

*Written 2026-07-29 for QUEUE.md batch B27 (enabler E7). Every probe in §1 was
run against `./target/release/ruxen compile` on that date. One probe refuted a
DONE row in `language.md`, one refuted a claim in this author's own B29 spec, and
one uncovered a new defect now filed as B76. No implementation exists yet.*
