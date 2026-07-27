# `language.md` — Ruby syntax + semantics parity

Authoritative, no-skip audit of Ruby's **language surface** (syntax + core
semantics) against Ruxen. Sibling of `core.md` (classes/methods) and the gem
matrices. Read `README.md` first for the three-tier framework.

**Status legend**

| Status | Meaning |
|--------|---------|
| `DONE` | Ruby syntax/semantics work as-is in Ruxen. |
| `PARTIAL` | Some of the feature works; a real sub-case is missing or spelled differently. |
| `MISSING` | Direct-parity feature that is expressible in Ruxen's model but not yet built. A burn-down item. |
| `COMPILE-TIME` | The right tier is Ruxen compile-time machinery (`derive`, mixins, `extension`, a macro, attribute-driven codegen, a `match` over a fixed set). Ergonomics match Ruby; mechanism is codegen, never runtime self-mutation. Some rows are already built (`derive`, `extension`); others name the design still to build (see Backlog B). Never "out of scope". |
| `OUT-OF-SCOPE` | Irreducibly runtime (needs an interpreter/metaobject runtime). Tracked with WHY + substitute. |

**How this was audited.** Read of the compiler (`lexer/token.rs`,
`parser/ast.rs`, `resolve`, `typeck`), the 467 pinned cases under
`tests/release-e2e/cases/`, and the `Q1..Q56` ledger. Every `MISSING`/`PARTIAL`
row without a case citation was verified by compiling a probe with the installed
`ruxen 0.1.0` (`~/.ruxen/bin/ruxen compile`). Probes and results are reproduced
in the notes where the answer was non-obvious.

**The one-line model.** Ruxen keeps Ruby's *shape* (blocks, mixins, `def/end`,
`match`, ownership-safe iterators) but is a **pure compiler**: no interpreter, no
runtime class mutation, no `eval` of runtime strings. The strategy is to do at
**compile time** what Ruby does at runtime. So:

- **Control-flow keyword ergonomics and literals are Direct parity.** Many cheap,
  common Ruby surfaces are now built; symbols, keyword operators, richer splat
  forms, and percent literals remain in the direct-parity burn-down.
- **Ruby's load-time metaprogramming is re-expressed at compile time** (`derive` / mixins / `extension` / a new macro / attribute-driven codegen / a `match` over a fixed method set). `define_method`, `method_missing`, `attr_accessor`, `send`, `respond_to?`, `Struct`, delegation, and reopening-to-add-methods all get a **compile-time design that produces the same result** so the compiled program never mutates itself at runtime. These are `COMPILE-TIME` (a design + build task), not out-of-scope.
- **Only the irreducibly-runtime slice is `OUT-OF-SCOPE`**: `eval` of a string unknown until runtime, `binding`, `TracePoint`, `ObjectSpace` heap-walking, and live monkey-patching of *already-compiled* third-party code. Each such row still names the closest compile-time design that covers the real use case.
- **Ruby's exception model is a deliberate design substitution**, not a gap: `Result` + `?` + `Drop` + `panic!`. A `begin/rescue/ensure` *surface* could be offered later as compile-time sugar over that machinery.

---

## 1. Literals

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| Integer decimal `42` | DONE | `lexer` IntLiteral; case `02` | |
| Integer underscores `1_000_000` | DONE | probe PASS | |
| Integer hex `0xFF` | DONE | probe PASS; case `38` | |
| Integer binary `0b1010` | DONE | probe PASS | |
| Integer octal `0o17` | DONE | probe PASS | |
| Integer octal bare `017` | PARTIAL | probe parses | Leading-`0` accepted but octal *value* semantics unverified; Ruby treats `017`=15. Prefer `0o`. |
| Sized int suffixes `i8..i64/u8..u64/isize/usize` | DONE | `NumericSuffix` | Ruxen extension over Ruby. |
| Bignum (arbitrary precision) | MISSING | i64-capped | Ruby auto-promotes `Integer`. Ruxen is fixed-width; no `BigInt`. Likely OUT-OF-SCOPE for v1. |
| Float `3.14` | DONE | case `03` | |
| Float scientific `1.5e3` | DONE | probe PASS; case `39` | |
| Float suffix `f32/f64` | DONE | `NumericSuffix` | Ruxen extension. |
| Rational `1r`, `3/4r` | MISSING | probe: parse error | No `Rational` type. |
| Complex `1i`, `2+3i` | MISSING | probe: parse error | No `Complex` type. |
| String double-quote `"..."` | DONE | `lexer/strings.rs` | |
| String single-quote `'...'` | PARTIAL | `lexer/strings.rs`; case `36` | Ruxen single-quote is **fully raw** (no escapes at all); Ruby allows `\\` and `\'`. Semantic diff. |
| Escapes `\n \t \u{...}` | DONE | `lex_escape_sequence` | |
| Interpolation `"#{expr}"` | DONE | cases `05/114` | |
| Interpolation format spec `"#{x:>5.2}"` | DONE | `FormatSpec`; cases `070/071` | Ruxen extension (Rust-style), not Ruby. |
| Heredoc squiggly `<<~ID` | MISSING | probe: `E0006` | |
| Heredoc dash `<<-ID` | MISSING | probe: lexes `<<` as `Shr` | |
| Heredoc plain `<<ID` | MISSING | — | |
| Triple-quote multiline `"""..."""` | DONE | `lexer/strings.rs`; case `37` | Ruxen's heredoc substitute (common-leading-whitespace stripped). Not Ruby syntax. |
| Char literal `?a` | PARTIAL | case `34/37` | Parses, but is a `Char`; Ruby `?a` is a 1-char `String`. |
| Symbol `:sym` (value) | MISSING | probe: `expected expression, found Colon` | No `Symbol` type. Foundational gap (blocks hash shorthand, `case` dispatch, many APIs). |
| Dynamic symbol `:"#{x}"` | MISSING | — | |
| `%s(sym)` symbol literal | MISSING | probe: `found Percent` | |
| Regexp `/re/imsxg` | DONE | cases `900-907` | Flags `i m s g x`; `~=` match op. |
| Regexp `%r{...}` | MISSING | probe: `found Percent` | |
| Array literal `[1,2,3]` | DONE | case `26` | |
| Hash rocket `{ "a" => 1 }` | DONE | `MapLiteral`; probe PASS | |
| Hash symbol shorthand `{ a: 1 }` | MISSING | probe: `found Colon` | Needs symbols first. High-value. |
| Hash `**` double-splat | MISSING | — | |
| `%w[a b c]` word array | MISSING | probe: `found Percent` | |
| `%i[a b c]` symbol array | MISSING | probe: `found Percent` | |
| `%q(...)` / `%Q(...)` string | MISSING | probe: `found Percent` | |
| Range `..` inclusive | DONE | `expr/operators.rs`; case `122` | Ruby semantics (`..` incl, `...` excl). `..=` is a hard `E0009`. |
| Range `...` exclusive | DONE | case `58`/`upto` probe | |
| Endless range `(1..)` | DONE | probe PASS | |
| Beginless range `(..5)` | MISSING | probe: `found DotDot` | |
| `true` / `false` | DONE | case `04` | |
| `nil` | DONE | `lexer`; replaces `None`/`null` | `nil` is Option::None and raw-null; `None` is a hard `E0008`. |
| `__FILE__` / `__LINE__` | MISSING | probe: undefined variable | |
| `__method__` | MISSING | probe: undefined variable | |
| `__dir__` | MISSING | — | |

---

## 2. Variables & assignment

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| Bare local `x = 1` | PARTIAL | requires `let`/`var` | Ruxen has no implicit binding; first assignment must be `let x =` / `var x =`. Fundamental surface diff. |
| Local read/reassign | DONE | `var` for mutable | |
| Instance var `@x` (general read/write) | PARTIAL | probe: `@x = 0` → `found At` | `@x` is accepted **only** as the `def init(@x: T)` auto-assign param (case `810`). Field access elsewhere is `self.x`. General `@ivar` MISSING. |
| Class var `@@x` | MISSING | probe: `found At` in class body | Use a `const` or class method. |
| Global `$x` | MISSING | probe: `E0006` on `$` | |
| Constant `CONST = ...` | DONE | `ConstDef`; case `44` | `const NAME: T = val`. |
| Multiple assignment `a, b = 1, 2` | DONE | `ExprKind::MultiAssign`; case `957` | Targets and values equal length; values snapshotted before any store. Owned Class/Struct/Enum values read from a place (variable/field/index) are rejected (would alias); Copy, String, and fresh-alloc values are allowed. |
| Parallel swap `a, b = b, a` | PARTIAL | case `957` | Copy and String swap work (simultaneous). A swap of owned Class/Struct/Enum objects is a clean compile error for now (needs move/clone lowering). |
| Destructuring `let (a, b) = t` | DONE | probe PASS; case `123` | Ruxen's substitute for LHS destructuring. |
| Splat assign `a, *b = [1,2,3]` | PARTIAL | `MultiAssign::splat_target`; case `963` | Trailing splat supports one Array RHS or a comma-list and is formatter-preserving. `b` must be a pre-declared `Array[T]` variable; `T` is Copy-only. Leading/middle splats, short-input nil-fill, and owned-element clone lowering remain. |
| Nested destructure `(a,(b,c)) = ...` | PARTIAL | tuple patterns only | Works for tuples; no array-splat form. |
| `||=` | DONE | `PipePipeEq` token; case `954` | Desugars to `a \|\| (a = b)`. Bool operands (Ruxen is strict-Bool); nilable memoization awaits a `??` operator. |
| `&&=` | DONE | `AmpAmpEq` token; case `954` | Desugars to `a && (a = b)`. |
| `+= -= *= /= %=` | DONE | `CompoundAssign`; probe PASS | |
| `<<= >>= &= |= ^= **=` | MISSING | lexer has only `PlusEq..PercentEq` | No tokens for bitwise/shift compound-assign. |

---

## 3. Operators

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| Arithmetic `+ - * / %` | DONE | `BinOp`; case `02` | |
| Power `**` | DONE | `StarStar` token; `BinOp::Pow`; case `952` | Right-associative, binds tighter than unary minus (`-2 ** 2 == -4`). `Int ** Int → Int` (`ruxen_int_pow`), any float operand → `Float` (`ruxen_float_pow`). Fixed the former silent misparse (`2 ** 8` was `16`). |
| Comparison `< > <= >=` | DONE | cases `126/209` | Auto-`Ord` on structs/enums. |
| Equality `== !=` | DONE | auto-`PartialEq`; case `206` | |
| Spaceship `<=>` | DONE | `Spaceship` token; `BinOp::Cmp`; case `959` | Returns `Int` (-1 / 0 / 1) for Int, Float, and String operands (`ruxen_int_cmp` / `ruxen_float_cmp` / `ruxen_str_spaceship`). Routing `Comparable` through it is a follow-up. |
| Logical `&& \|\| !` | DONE | `BinOp`/`UnaryOp`; case `955` | Now **short-circuit** (branch-CFG lowering). Was eagerly evaluating both operands (a correctness bug: `ptr != nil && ptr.field` touched `ptr.field` when nil); fixed 2026-07-23. |
| Keyword `and` / `or` / `not` | DONE | `KeywordAnd`/`KeywordOr`/`KeywordNot`; case `964` | Ruby precedence: `and`/`or` share one left-associative tier below assignment; `not` consumes assignment and symbolic operators, then stops before keyword `and`/`or`. Strict-Bool, short-circuit, and formatter-preserving. |
| Bitwise `& | ^ << >>` | DONE | `BinOp`; case `812` | |
| Bitwise complement `~` | MISSING | probe: `E0006` on `~` | No token (only `~=`). |
| Ternary `c ? a : b` | DONE | parser desugar to `if`/`else`; case `958` | Right-associative. Disambiguated from the `?` try operator by whitespace: an attached `x?` is try, a spaced `c ? a : b` is ternary. |
| Safe navigation `&.` | DONE | `SafeNav`; probe PASS | `&.` not `?.`. |
| Range `.. ...` | DONE | §1 | |
| Operator method def `+ - * / % & | ^ << >> [] []= -@ +@ !` | DONE | case `631/632` | Explicit `a.+(b)` and sugared `a + b` both dispatch. |
| Operator method def `== <=> < > <= >=` | MISSING | probe: `def ==` → `found EqEq` | Comparison/equality family is auto-derived only, not user-definable. |
| String concat `+` | DONE | case `317` | |
| String append `<<` | DONE | probe PASS | |
| `defined?(x)` | MISSING | probe: undefined function | |

---

## 4. Methods & definitions

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `def name ... end` | DONE | case `12` | |
| Implicit return (last expr) | DONE | case `12` | |
| Explicit `return` | DONE | case `60` | |
| Default args `def f(x = 1)` | DONE | probe PASS; case `802/804` | `def f(x: T = default)`. |
| Named-argument call `f(w: 3, h: 4)` | DONE | probe PASS | Any param is callable by name. |
| Required keyword args `def f(a:)` | PARTIAL | probe: named params also callable positionally | Ruxen params are position-or-name with no *required-keyword* distinction; `f(a:)`/`**kwargs` semantics absent. |
| Positional splat `*args` | MISSING | probe: `found Star` | Only FFI `is_variadic`. High-value. |
| Keyword splat `**kwargs` | MISSING | probe: `found Star` | |
| Block param `&blk` + `yield` | DONE | cases `908-912`, `93` | `&block:` param, `yield`, `block_defined?`. |
| `block_given?` | DONE | case `909`/probe (`block_given?`) | |
| Endless def `def f(x) = expr` | MISSING | probe: `found Eq` | |
| Visibility `private/protected/public` | DONE | section markers; `parser/methods.rs` | `private :a, :b` name-lists too. |
| `module_function` | PARTIAL | cases `806` | Module methods via `def self.x`; `module_function` keyword surface not confirmed. |
| Singleton method `def obj.x` | MISSING | probe: `found Dot` | |
| `def self.x` (class method) | DONE | case `17/807` | |
| Operator methods | DONE | §3 | |
| `alias new old` | DONE | `AliasDef`; cases `913-918` | Keyword form. |
| `alias_method` (runtime) | COMPILE-TIME | — | Use the `alias` keyword. |
| `super(args)` delegation | DONE | case `16` | Forwards to parent (esp. in `init`). |
| Bare `super` / super-as-value | PARTIAL | probe: tail `super()` types as `()` | Using super's return value in a typed tail position fails typeck (`expected String, found ()`). Delegation-only today. |

---

## 5. Blocks, procs, lambdas

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| Brace block `{ |x| ... }` | DONE | case `28/88` | |
| `do ... end` block | DONE | cases `88/93`, probe PASS | |
| `do..end` = immediate, `{}` = stored | DONE | quiver DSL relies on it | Documented Ruxen block semantics. |
| `&:sym` to-proc | DONE | case `944`; `BlockPassKind::ToProc` | |
| Numbered param `_1` | DONE | case `943` | |
| `it` param | DONE | probe PASS; case `943` | |
| `&block` forwarding | DONE | case `946`; `BlockPassKind::Forward` | |
| `yield` with args | DONE | cases `929/93` | |
| Closure bound to var + `.call` | DONE | probe PASS | `let f = { |x: Int| x + 1 }; f.call(2)`. |
| Stabby lambda `->(x) { x }` | MISSING | probe: `found Arrow` | Use `{ |x| ... }`. |
| `Proc.new { }` | MISSING | probe: undefined `Proc` | |
| `lambda { }` | MISSING | — | |
| proc-vs-lambda arity/return semantics | PARTIAL | closures are typed; no distinct `Proc`/`Lambda` object model | Non-local `return` from a stored block is not modeled; `next` gives block value (case `945`). |
| `curry` | MISSING | — | |

---

## 6. Control flow

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `if/elsif/else` | DONE | case `07` | |
| `if` modifier `X if c` | DONE | `parse_statement`; case `953` | Parser desugar to a block `if`. |
| `unless` statement | DONE | `parse_unless_expr`; case `953` | Desugars to `if !cond` (no `elsif`, per Ruby). |
| `unless` modifier | DONE | `parse_statement`; case `953` | `return … unless c` supported. |
| `while` | DONE | case `08` | |
| `while` modifier `X while c` | DONE | `parse_statement`; case `953` | Condition-first (not do-while). |
| `until` statement | DONE | `Until` token + `parse_until_expr`; case `953` | Desugars to `while !cond`. |
| `until` modifier | DONE | `parse_statement`; case `953` | |
| `begin..end while` (do-while) | MISSING | probe: `found While` | |
| `for x in ...` | DONE | cases `09/639`; probe PASS | |
| `loop` | DONE | case `53/54` | `loop` + `end` (no `do`; `loop do` is a parse error). |
| `break` (+ value) | DONE | cases `53/54` | |
| `next` (+ value, in block) | DONE | case `945` | |
| `continue` (in loop) | DONE | case `55` | Ruxen keyword; Ruby uses `next` for both. |
| `redo` | MISSING | no token | |
| `retry` (loop) | MISSING | no token | |
| `throw` / `catch` | MISSING | probe: `found Colon` | |
| Ternary `? :` | DONE | §3; case `958` | See §3 row. |
| `case/when` (`===` dispatch) | DONE | `parse_case_when`; case `960` | Desugars to `if`/`elsif`: `when v` → `s == v`, `when a, b` → `s == a \|\| s == b`, `when lo..hi` → range membership. Subject re-evaluated per branch (v1). |
| `case/when then` inline | DONE | case `960` | `when x then body` (and newline form) both parse. |
| `case/in` (pattern) | DONE | `parse_case_in`; case `960` | Alias of `match`: `case s / in pat then body / … / end`. `else` becomes a wildcard arm. |
| `.times`, `.upto`, `.each` iterators | PARTIAL | `.each` DONE (case `120`); `.times`/`.upto` MISSING | probe: `no method times on Int`. Use ranges. |

---

## 7. Pattern matching (`case/in`)

Ruxen's `match subj ... end` is the pattern-matching workhorse (arms use
`pattern -> body`). The Ruby `case ... in` keyword surface is now **also**
accepted and parses to the same `Match` AST: `case s / in pat then body / … /
end` (pin `960`). Ruby's `case ... when` (`===` dispatch) is a separate
construct that desugars to `if`/`elsif` (§6).

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `case x in PAT` keyword surface | MISSING | probe: `found In` | Spelled `match x / PAT -> ... / end`. |
| Literal pattern | DONE | case `10` | |
| Variable binding pattern | DONE | case `76` | |
| Wildcard `_` | DONE | case `10` | |
| Rest `*` / `..` in pattern | DONE | `Pattern::Rest`; case `75` | |
| Or-pattern `a | b` | DONE | case `58` | |
| Guard `if` | DONE | case `11` | |
| Tuple / array pattern `[a, b]` | PARTIAL | `Pattern::Tuple`; case `62` | Tuple destructure works; Ruby array-pattern with splat middle unverified. |
| Enum/variant pattern | DONE | cases `18/94` | |
| Struct pattern `{ field: }` | PARTIAL | `Pattern::Struct` | Struct field patterns exist; hash-value `deconstruct_keys` semantics absent. |
| Hash pattern `in { a:, b: }` | MISSING | needs symbols | |
| Find pattern `in [*, x, *]` | MISSING | — | |
| Pin operator `^var` | MISSING | — | |
| `deconstruct` / `deconstruct_keys` protocol | MISSING | — | |
| One-line `x => pat` / `x in pat` | MISSING | — | |

---

## 8. Exceptions

**Deliberate design substitution, not a gap.** Ruby's exception model is
re-expressed with `Result[T, E]` + the `?`/try operator + an `Error` mixin +
ownership `Drop` + `panic!`. Nothing here needs a runtime metaobject; a
`begin/rescue/ensure` *keyword surface* could later be offered as **compile-time
sugar** that lowers to the machinery below (rescue → `match` on the `Err` arm,
`ensure` → a `Drop`/`defer` scope guard, `raise` → early-return `Err`). Today
the substitute is idiomatic but the sugar is not built.

| Ruby feature | Status | Compile-time substitute (design) | Note |
|---|---|---|---|
| `begin/rescue/else/ensure` | COMPILE-TIME | `Result` + `match`; sugar = lower `rescue` to the `Err` arm, `ensure` to a `Drop` scope guard | probe: `rescue` not parsed today. Sugar is a parser + desugar task. |
| `raise "msg"` | COMPILE-TIME | early-return `Err(...)`; `panic!` for fatal | Sugar `raise X` → `return Err(X)` inside a `Result` fn. |
| `raise SomeError.new` | COMPILE-TIME | `Err(MyErr.Variant(...))` | Typed error enum, not a runtime class. |
| `rescue => e` modifier | DONE | `?` operator (case `25`) | The propagate case is already sugar. |
| Custom exception class | DONE | `enum E; include Error; def message` | cases `94/95`. |
| `retry` in rescue | COMPILE-TIME | bounded loop over `Result`; a `retry(n)` combinator | cases `444-449` show the retry combinators. |
| `ensure` cleanup | DONE | ownership/`Drop` (cases `134/138`) | Deterministic destruction is strictly stronger than `ensure`. |
| Exception hierarchy / `StandardError` | COMPILE-TIME | `Error` mixin + enum variants; `?`-conversion (case `95`) | Nominal error taxonomy resolved at compile time. |
| `panic!` (unrecoverable) | DONE | case `96` | `panic!("msg")`. |

---

## 9. Classes & objects

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `class Name ... end` | DONE | case `14` | |
| Fields | DONE | `name: T` decls; case `72` | No `attr_*`; visibility via sections. |
| `attr_accessor/reader/writer` | COMPILE-TIME | macro/`derive` that emits reader+writer methods for the listed fields | probe: keyword not parsed. Cheap codegen sugar over existing fields; a clear build item. |
| Inheritance `<` | DONE | case `16` | |
| `super(args)` | DONE / PARTIAL | §4 | Delegation DONE; super-as-value PARTIAL. |
| Open class / reopening — **add methods** | COMPILE-TIME | `extension Type ... end` adds methods to a type from outside (compile-time "monkey-patch") | Partly DONE via `extension`. Reopening the *same* `class` name is `E0727`; the sanctioned surface is `extension`. |
| Open class / reopening — **repatch already-compiled 3rd-party class at runtime** | OUT-OF-SCOPE | vendor the type + `extension`, or a fork | The only irreducible slice: mutating code that is already compiled. |
| `class << self` | COMPILE-TIME | `def self.x` today; a `class << self ... end` block that rewrites to `self.`-methods | probe: `found Class`. Parser sugar over the existing class-method path. |
| Singleton method `def obj.x` | COMPILE-TIME | for a *type* → `extension`; per-instance singleton → not modeled (would need a per-object vtable) | Per-instance case is the rare, near-irreducible slice; the type-level case is `extension`. |
| Class method `def self.x` | DONE | case `17` | |
| Constants in class | DONE | case `44` | |
| `attr`-style equality/hash/inspect | DONE (auto) | cases `133/207/139` | Auto `PartialEq`/`Hash`/`Debug`. |
| `to_s` universal | DONE | case `810` | User `to_s` overrides derived. |
| `<=>` + `Comparable` (hand-written) | PARTIAL | auto-`Ord` DONE (case `209`); manual `<=>` needs a spaceship token + `def <=>` in the operator wave | Direct-parity build: add `<=>` and route `Comparable` through it. |
| `Struct.new(:a, :b)` | COMPILE-TIME | a `Struct.new(...)` macro that expands to a `struct` with accessors + auto-derives | `struct` is the mechanism (case `63`); the `Struct.new` *surface* is a macro over it. |
| `Data.define(:a, :b)` | COMPILE-TIME | macro → immutable `struct` (no writers) + value semantics + auto-derives | Same design as `Struct.new`, writers omitted. |
| `Comparable` protocol (auto) | DONE | case `126/209` | Via derived `Ord`. |
| `Enumerable` protocol | DONE | cases `612/613/936` | `include Enumerable`. |

---

## 10. Modules & mixins

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `module M ... end` (namespace) | DONE | case (module_method probe) | Namespacing + `M.fn`. |
| `module` as mixin via `include` | MISSING | probe: `include <module>` → `no field hello` | Method-bearing includables must be `mixin`, not `module`. |
| `mixin M` + `include M` | DONE | cases `21/22/86` | Ruby-`module`-include equivalent. |
| `include` (instance methods) | DONE | case `21` | |
| `prepend` | COMPILE-TIME | a `prepend` mixin whose methods win over the type's own, `super` chaining to the original | Method-resolution order is decided at compile time; `prepend` = insert the mixin ahead of the type in that fixed order. |
| `extend` (add as class/singleton methods) | COMPILE-TIME | `extension` for a type; a `module`-as-class-methods `extend` that lifts the module's `def`s to `self.` methods | probe: include-only today. Compile-time lift, no runtime singleton. |
| Default methods in mixin | DONE | cases `22/86/87` | |
| Mixin static/assoc-type/generic | DONE | cases `80/81/655-659` | Beyond Ruby (typed mixins). |
| MRO / ancestor linearization | COMPILE-TIME | static resolution | No runtime `ancestors`; resolution is compile-time. |
| `refinements` / `using` | COMPILE-TIME | lexically-scoped `extension` activated per-module/per-import | Ruby refinements are already lexically scoped; a scoped `extension` covers the real use case at compile time. Runtime activation toggling is the only irreducible edge. |
| `Comparable`/`Enumerable` includes | DONE / PARTIAL | §9 | `Enumerable` DONE; `Comparable` needs `<=>`. |
| `require` / `require_relative` | DONE | `module_loader.rs` (2026-07-14) | Ruby-style AOT module loading. |
| `autoload :Const, "path"` | DONE | `module_loader::expand_autoloads` | Unresolved-name fixpoint. |

---

## 11. Metaprogramming

**The doctrine: do at compile time what Ruby does at runtime.** Ruby generates
and dispatches methods from a live metaobject; Ruxen is a pure compiler, so each
feature below gets a **compile-time design that produces the identical result**
without the program mutating itself at runtime. The recurring moves are: (a) a
**literal symbol/name folds to a direct reference** at compile time; (b) a
**bounded, compile-time-known method/field set** becomes generated real methods
or a `match`; (c) a **schema/attribute** drives codegen (`derive` / a macro /
attribute-driven expansion). Only string-`eval`, `binding`, `TracePoint`,
`ObjectSpace`, and repatching already-compiled code are irreducible.

*Caution (measured):* several reflection calls **appear to compile inside string
interpolation** because the interpolation re-lexer is permissive, then parse-fail
standalone and emit empty output at runtime. They are not functional today; the
`COMPILE-TIME` status marks the design to build, not a working feature.

| Ruby feature | Status | Compile-time design that delivers the same result | Note / irreducible edge |
|---|---|---|---|
| `define_method(:m) { ... }` | COMPILE-TIME | attribute/macro that expands a declarative spec into concrete `def`s at compile time (the "dynamic finders / generated accessors" use case); `derive` for the standard families | Body known at compile time → real methods. probe: `found LParen`. |
| `method_missing` | COMPILE-TIME | for delegation/proxy/dynamic-attr (the real uses), a macro that enumerates the fixed, schema-derived name set and emits forwarding methods; a `Hash`-backed dynamic object for open key access | Genuinely-unbounded `obj.anything` with no schema is the only irreducible edge. probe: needs `Symbol`. |
| `respond_to_missing?` | COMPILE-TIME | pairs with the above: the generated method set is known, so membership is a compile-time constant | |
| `send(:m, ...)` / `public_send` | COMPILE-TIME | literal symbol → direct call `obj.m(...)`; runtime symbol from a bounded set → generated `match sym { :m -> obj.m(...) , ... }` | Unbounded runtime symbol → the `match` still covers any declared method. |
| `respond_to?(:m)` | COMPILE-TIME | literal symbol → constant-fold `true`/`false` from the static type; dynamic → `match` over the known method set | Method set is a compile-time fact. |
| `instance_variable_get/set(:@x)` | COMPILE-TIME | literal name → field access `obj.x`; dynamic name → `match` over the fixed field set, or a `Hash`-backed object for open attributes | Field set is fixed and known. probe: empty at runtime today. |
| `.class` / `.class.name` | COMPILE-TIME | emit the static type name as a compile-time `&str` const; a `TypeId`-style value for comparisons | No reflective mutable class object; the *name/identity* use case folds to a const. |
| `is_a?` / `kind_of?` / `instance_of?` | COMPILE-TIME | literal type → constant-fold from the static type; sum-type dispatch → `match` over enum variants | Static types make the answer a compile-time fact. probe: `no method is_a?`. |
| `const_get` / `const_set` | COMPILE-TIME | literal name → direct constant reference; dynamic over a known module → generated `match`; `const_set` of a *new* name → not expressible (use a macro to declare it) | Declaring a brand-new constant from runtime data is the irreducible edge. |
| `define_singleton_method` | COMPILE-TIME | type-level → `extension`/macro emitting the method; per-instance → not modeled | Per-instance singleton is the rare irreducible edge (needs per-object vtable). |
| `class_eval`/`instance_eval` **(block form)** | COMPILE-TIME | macro that splices the compile-time-known block into the class/instance body | The block is code, so it is compile-time-known. |
| `eval("string")` | OUT-OF-SCOPE | none for arbitrary strings; templated codegen covers "generate code from a schema" | Irreducible: code unknown until runtime. |
| `binding` | OUT-OF-SCOPE | an explicit struct capturing the locals you actually need | Irreducible: needs a live environment + eval. |
| `ObjectSpace` (heap walk) | OUT-OF-SCOPE | an explicit registry the program maintains (compile-time-inserted registration on construct) | Irreducible: no GC/heap-walk runtime. |
| `TracePoint` | OUT-OF-SCOPE | compile-time instrumentation: attribute-driven codegen inserting enter/exit hooks (covers profiling/tracing) | Irreducible only for *arbitrary runtime* hook installation. |
| `Method` / `UnboundMethod` objects | COMPILE-TIME | `obj.method(:m)` with a literal → a typed closure `{ obj.m }`; `UnboundMethod` → a fn value taking the receiver | First-class typed function values replace method objects. |
| `tap` | MISSING | — | Not metaprogramming; a `Kernel` Direct-parity method. probe: `no method tap`. Cheap to add. |
| `then` / `yield_self` | MISSING | — | Not metaprogramming; a `Kernel` Direct-parity method. probe: `no method then`. Cheap to add. |
| `delegate` / `Forwardable#def_delegators` | COMPILE-TIME | a `delegate :a, :b, to: :field` macro emitting concrete forwarding methods | Fixed method list → real methods. |

---

## 12. Misc / special

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `puts` / `print` accept any value | DONE | typeck `to_s` conversion; case `956` | A non-String arg is converted through its universal `to_s` (HIR only, so the AST and `ruxen fmt` are unchanged). A value with no `to_s` (Array, Option, tuple, a struct without a Debug/Display derive) gives a clean compile error, not a link error. |
| `puts a, b` (multi-arg) | MISSING | probe: `found Comma` | Single arg only; multi-arg is a follow-up. |
| String `%` formatting `"%05d" % x` | DONE | probe PASS | |
| `format` / `sprintf` | MISSING | probe: undefined function | `%` operator works; the functions do not. |
| `p` / `pp` (inspect) | MISSING | — | |
| Line comment `#` | DONE | `lexer` | |
| Block comment `#= ... =#` | DONE | case `110` | Ruxen form. |
| `=begin ... =end` block comment | MISSING | probe: `found Eq` | Use `#= =#`. |
| Doc comment `##` | DONE | cases `111/130` | |
| `frozen_string_literal: true` magic | DONE (inert) | probe PASS | Accepted as a comment; strings are owned by default, so it is a no-op. |
| `BEGIN {}` / `END {}` | MISSING | — | |
| `defined?` | MISSING | §3 | |
| `__method__` / `__FILE__` / `__LINE__` | MISSING | §1 | |
| `<=>` spaceship | DONE | §3; case `959` | See §3 row. Int/Float/String. |
| `and`/`or`/`not` | DONE | §3; case `964` | |
| `nil?` universal predicate | MISSING | probe: `no field nil? on Int` | Non-nullable types make it moot; `Int?` (Option) uses `match`/`?`. |

---

## Priority backlog

Two backlogs below. **Backlog A** is Direct-parity `MISSING`/`PARTIAL` (syntax a
Ruby dev types); **Backlog B** is the `COMPILE-TIME` design work (metaprogramming
re-expressed as codegen so the program never mutates itself at runtime). Only the
truly irreducible slice is excluded from both (listed last).

### Backlog A — Direct parity (syntax), by value

#### P0 — cheap, ubiquitous, block the "reads like Ruby" promise

1. ~~**Statement modifiers** `X if c` / `X unless c` / `X while c` / `X until c`.~~ **DONE** (parser desugar; `return … unless c` handled). Pin `953`. (§6)
2. ~~**`unless` and `until`** (statement + modifier).~~ **DONE** (`Until` token added; block + modifier forms). Pin `953`. (§6)
3. ~~**Multiple assignment** `a, b = 1, 2` and Copy-value swap `a, b = b, a`.~~ **DONE** (dedicated `MultiAssign` node; simultaneous stores). Trailing Copy-element splat `a, *b = xs` is also built and formatter-preserving; leading/middle splats, short-input nil-fill, owned-object swap, and owned splat elements remain. Pins `957`, `963`. (§2)
4. ~~**`||=` and `&&=`.**~~ **DONE** (Bool conditional-assign; desugars to `a || (a = b)` / `a && (a = b)`). This also uncovered and fixed a **short-circuit bug**: `&&`/`||` were evaluating both operands eagerly. Pins `954`, `955`. (§2)
5. ~~**Ternary `c ? a : b`.**~~ **DONE** (parser desugar to `if`/`else`; whitespace-disambiguated from the `?` try operator; right-associative). Pin `958`. (§3)
6. ~~**`**` power operator.**~~ **DONE** (`StarStar` token + `BinOp::Pow`, right-associative, routed to `ruxen_int_pow`/`ruxen_float_pow`). Fixed the silent miscompile (`2 ** 8` was `16`, now `256`). Pin `952`. (§3)
7. ~~**`puts`/`print` accept any `to_s`-able value**~~ **DONE** for the single-argument form (typeck converts a non-String arg via its universal `to_s`; `puts 42` works, AST and fmt unchanged). Multi-arg `puts a, b` is a follow-up. Pin `956`. (§12)

#### P1 — high-frequency, need one new primitive each

8. **Symbols `:sym` as a first-class value** (+ `Symbol` type). Unlocks hash symbol-shorthand `{ a: 1 }`, symbol arrays, and reflective-style APIs. (§1)
9. **Hash symbol-key shorthand `{ a: 1 }`** (rides on symbols). (§1)
10. ~~**`and` / `or` / `not` keyword operators.**~~ **DONE** (Ruby
    low-precedence tiers, strict-Bool short-circuit lowering, and
    surface-preserving formatting). Pin `964`. (§3)
11. **`<=>` spaceship + user-definable `==`/`<=>`** so hand-written `Comparable` works, not only auto-`Ord`. The `<=>` OPERATOR is **DONE** (Int/Float/String, returns -1/0/1, pin `959`); routing `Comparable` through it and user-definable `def <=>` remain. (§3, §9)
12. **`*args` / `**kwargs` splat params** and required-keyword args `def f(a:)`. (§4)
13. **General `@ivar` read/write** beyond the `init` auto-assign sugar. (§2)
14. ~~**`case/when` keyword surface** (`===` dispatch) as sugar over `match`.~~ **DONE** (`case/when` desugars to `if`/`elsif`; `case/in` is an alias of `match`). Pin `960`. (§6)
15. **`Integer#times` / `#upto` / `#downto`** iterators. (§6)
16. **`tap` and `then`/`yield_self`.** Cheap `Kernel` additions. (§11)

#### P2 — completeness, lower frequency

17. **Heredocs `<<~ID` / `<<-ID` / `<<ID`** (triple-quote exists as a partial substitute). (§1)
18. **Endless method def `def f(x) = expr`.** (§4)
19. **`%w` / `%i` / `%q` / `%Q` / `%r` percent-literals.** (§1)
20. **Beginless range `(..5)`.** (§1)
21. **`~` bitwise complement**; bitwise/shift compound-assigns `<<= &= |= ^=`. (§2, §3)
22. **`defined?`.** (§3)
23. **`redo`; `begin..end while` do-while.** (§6)
24. **Full `case/in` pattern surface**: hash patterns, find patterns, pin `^`, `deconstruct`/`deconstruct_keys`, one-line `=>`/`in`. (§7)
25. **Stabby lambda `->(x){}`, `Proc.new`, `lambda`, `curry`.** (§5)
26. **`format`/`sprintf`, `p`/`pp`, `__method__`/`__FILE__`/`__LINE__`, `BEGIN`/`END`.** (§12)
27. **Ruby method-name parity in core** (e.g. `upcase`/`downcase` vs Ruxen `to_upper`/`to_lower`). Tracked in `core.md`; listed here because it affects "reads like Ruby".

### Backlog B — Compile-time designs (metaprogramming as codegen)

These deliver Ruby's runtime-metaprogramming *results* at compile time. Each is a
design + build task, not a dead end. Sorted by how much of the gem ecosystem they
unblock.

B1. **Attribute-driven codegen / a `define`-family macro** — the substrate for `define_method`, `attr_accessor/reader/writer`, `delegate`/`def_delegators`, `Struct.new`, `Data.define`. One macro engine that expands a declarative field/method spec into concrete `def`s. Unblocks ActiveRecord-style ORMs, serializers, delegators. (§9, §11)
B2. **Literal-symbol folding** for `send(:m)`, `respond_to?(:m)`, `instance_variable_get(:@x)`, `const_get(:C)`, `.method(:m)`, `is_a?(T)`, `.class`. Each folds to a direct call/field/const/closure/constant. Needs first-class symbols (Backlog A #8) plus a fold pass. (§11)
B3. **Bounded-set dispatch `match`** for the dynamic-symbol forms of send / respond_to? / instance_variable_get over a type's fixed method/field set. (§11)
B4. **`extension` maturation** as the compile-time monkey-patch: covers open-class "add methods", `extend`, `prepend` (MRO insertion), and scoped `refinements`. (§9, §10)
B5. **`method_missing` via schema** — a proxy/delegation macro that emits the fixed, schema-derived method set; `Hash`-backed dynamic object for open-key access. (§11)
B6. **`class_eval`/`instance_eval` block form**: splice a compile-time-known block into a class/instance body. (§11)
B7. **`begin/rescue/ensure` sugar** over `Result`/`Drop` (optional; the substitute already works without it). (§8)
B8. **Compile-time instrumentation** (attribute-driven enter/exit hooks) as the `TracePoint` substitute for profiling/tracing. (§11)

### Irreducible — `OUT-OF-SCOPE` (needs a runtime Ruxen will not grow)

- `eval("string")` of code unknown until runtime; `binding` (live local environment). (§11)
- `ObjectSpace` heap-walking; `TracePoint` arbitrary runtime hook installation (profiling covered by B8). (§11)
- Repatching an **already-compiled** third-party class at runtime; per-instance singleton methods. (§9)
- `Rational` / `Complex` / arbitrary-precision `Bignum` (fixed-width numeric model). (§1)

Each names its closest compile-time cover above; none blocks a real Ruby app pattern.

---

*Audited against `ruxen 0.1.0` on 2026-07-23. Probe corpus:
`$CLAUDE_JOB_DIR/tmp/probes`. Update a row only with a case pin or a fresh
probe; never let `PARTIAL`/`MISSING` become `DONE` without a
`tests/release-e2e/` case (README process rule 4).*
