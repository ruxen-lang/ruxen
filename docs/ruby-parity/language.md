# `language.md` — Ruby syntax + semantics parity

Authoritative, no-skip audit of Ruby's **language surface** (syntax + core
semantics) against Ruxen. Sibling of `core.md` (classes/methods) and the gem
matrices. Read `README.md` first for the three-tier framework.

**Status legend**

| Status | Meaning |
|--------|---------|
| `DONE`, tagged [implemented] | Ruby syntax/semantics work as-is in Ruxen. |
| `PARTIAL`, tagged [implemented]; the note names the gap | Some of the feature works; a real sub-case is missing or spelled differently. |
| `MISSING`, tagged [planned](../../TODO.md#t-parity-language) (or the more specific tracker item the row links) | Direct-parity feature that is expressible in Ruxen's model but not yet built. A burn-down item. |
| `COMPILE-TIME`, tagged [planned](../../TODO.md#t-parity-compile-time) unless the row says it already works | The right tier is Ruxen compile-time machinery (mixins, `extension`, a macro, attribute-driven codegen, a `match` over a fixed set). Ergonomics match Ruby; mechanism is codegen, never runtime self-mutation. Some rows are already built (mixins, `extension`); others name the design still to build (see Backlog B). Never "out of scope". |
| `OUT-OF-SCOPE`, tagged [retired]: Ruxen will not build it | Irreducibly runtime (needs an interpreter/metaobject runtime). Tracked with WHY + substitute. |

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
- **Ruby's load-time metaprogramming is re-expressed at compile time** (mixins / `extension` / a new macro / attribute-driven codegen / a `match` over a fixed method set). `define_method`, `method_missing`, `attr_accessor`, `send`, `respond_to?`, `Struct`, delegation, and reopening-to-add-methods all get a **compile-time design that produces the same result** so the compiled program never mutates itself at runtime. These are `COMPILE-TIME` (a design + build task), not out-of-scope.
- **Only the irreducibly-runtime slice is `OUT-OF-SCOPE`**: `eval` of a string unknown until runtime, `binding`, `TracePoint`, `ObjectSpace` heap-walking, and live monkey-patching of *already-compiled* third-party code. Each such row still names the closest compile-time design that covers the real use case.
- **Ruby's exceptions are the target**: `raise`, `begin/rescue/ensure` and `retry`, checked statically and lowered onto the existing `Result` and `Drop` machinery, are [planned](../../TODO.md#t-exceptions). The `Result` + `?` + `panic!` spellings that work today are [deprecated].

---

## 1. Literals

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| Integer decimal `42` | [implemented] DONE | `lexer` IntLiteral; case `02` | |
| Integer underscores `1_000_000` | [implemented] DONE | probe PASS | |
| Integer hex `0xFF` | [implemented] DONE | probe PASS; case `38` | |
| Integer binary `0b1010` | [implemented] DONE | probe PASS | |
| Integer octal `0o17` | [implemented] DONE | probe PASS | |
| Integer octal bare `017` | [implemented] PARTIAL | probe parses | Leading-`0` accepted but octal *value* semantics unverified; Ruby treats `017`=15. Prefer `0o`. |
| Sized int suffixes `i8..i64/u8..u64/isize/usize` | [implemented] DONE | `NumericSuffix` | Ruxen extension over Ruby. |
| Bignum (arbitrary precision) | [planned](../../TODO.md#t-parity-language) MISSING | i64-capped | Ruby auto-promotes `Integer`. Ruxen is fixed-width; no `BigInt`. Likely OUT-OF-SCOPE for v1. |
| Float `3.14` | [implemented] DONE | case `03` | |
| Float scientific `1.5e3` | [implemented] DONE | probe PASS; case `39` | |
| Float suffix `f32/f64` | [implemented] DONE | `NumericSuffix` | Ruxen extension. |
| Rational `1r`, `3/4r` | [planned](../../TODO.md#t-parity-language) MISSING | probe: parse error | No `Rational` type. |
| Complex `1i`, `2+3i` | [planned](../../TODO.md#t-parity-language) MISSING | probe: parse error | No `Complex` type. |
| String double-quote `"..."` | [implemented] DONE | `lexer/strings.rs` | |
| String single-quote `'...'` | [implemented] PARTIAL | `lexer/strings.rs`; case `36` | Ruxen single-quote is **fully raw** (no escapes at all); Ruby allows `\\` and `\'`. Semantic diff. |
| Escapes `\n \t \u{...}` | [implemented] DONE | `lex_escape_sequence` | |
| Interpolation `"#{expr}"` | [implemented] DONE | cases `05/114` | |
| Interpolation format spec `"#{x:>5.2}"` | [deprecated] DONE, replace with `x.inspect`, `n.to_s.rjust(5)` or `format` [planned](../../TODO.md#t-d9) | `FormatSpec`; cases `070/071` | Ruxen extension (Rust-style), not Ruby. |
| Heredoc squiggly `<<~ID` | [planned](../../TODO.md#t-heredoc) MISSING | probe: `E0006` | |
| Heredoc dash `<<-ID` | [planned](../../TODO.md#t-heredoc) MISSING | probe: lexes `<<` as `Shr` | |
| Heredoc plain `<<ID` | [planned](../../TODO.md#t-heredoc) MISSING | — | |
| Triple-quote multiline `"""..."""` | [implemented] DONE | `lexer/strings.rs`; case `37` | Ruxen's heredoc substitute (common-leading-whitespace stripped). Not Ruby syntax. |
| Char literal `?a` | [implemented] PARTIAL | case `34/37` | Parses, but is a `Char`; Ruby `?a` is a 1-char `String`. |
| Symbol `:sym` (value) | [planned](../../TODO.md#t-symbols) MISSING | probe: `expected expression, found Colon` | No `Symbol` type. Foundational gap (blocks hash shorthand, `case` dispatch, many APIs). |
| Dynamic symbol `:"#{x}"` | [planned](../../TODO.md#t-symbols) MISSING | — | |
| `%s(sym)` symbol literal | [planned](../../TODO.md#t-symbols) MISSING | probe: `found Percent` | |
| Regexp `/re/imsxg` | [implemented] DONE | cases `900-907` | Flags `i m s g x`; `~=` match op. |
| Regexp `%r{...}` | [planned](../../TODO.md#t-parity-language) MISSING | probe: `found Percent` | |
| Array literal `[1,2,3]` | [implemented] DONE | case `26` | |
| Hash rocket `{ "a" => 1 }` | [implemented] DONE | `MapLiteral`; probe PASS | |
| Hash symbol shorthand `{ a: 1 }` | [planned](../../TODO.md#t-symbols) MISSING | probe: `found Colon` | Needs symbols first. High-value. |
| Hash `**` double-splat | [planned](../../TODO.md#t-parity-language) MISSING | — | |
| `%w[a b c]` word array | [implemented] DONE | pin `974` | `[]`, `()`, `{}` and `<>` delimiters, in expression context only (so `a % w` stays modulo). |
| `%i[a b c]` symbol array | [planned](../../TODO.md#t-symbols) MISSING | probe: `found Percent` | |
| `%q(...)` / `%Q(...)` string | [planned](../../TODO.md#t-parity-language) MISSING | probe: `found Percent` | |
| Range `..` inclusive | [implemented] DONE | `expr/operators.rs`; case `122` | Ruby semantics (`..` incl, `...` excl). `..=` is a hard `E0009`. |
| Range `...` exclusive | [implemented] DONE | case `58`/`upto` probe | |
| Endless range `(1..)` | [implemented] DONE | probe PASS | |
| Beginless range `(..5)` | [planned](../../TODO.md#t-parity-language) MISSING | probe: `found DotDot` | |
| `true` / `false` | [implemented] DONE | case `04` | |
| `nil` | [implemented] DONE | `lexer`; replaces `None`/`null` | `nil` is the absent value of `T?` and raw-null; `None` is a hard `E0008`. |
| `__FILE__` / `__LINE__` | [planned](../../TODO.md#t-parity-language) MISSING | probe: undefined variable | |
| `__method__` | [planned](../../TODO.md#t-parity-language) MISSING | probe: undefined variable | |
| `__dir__` | [planned](../../TODO.md#t-parity-language) MISSING | — | |

---

## 2. Variables & assignment

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| Bare local `x = 1` | [implemented] DONE | `resolve/control_flow.rs` (`resolve_bare_assignment_binding`) | An assignment to an undeclared name declares a mutable local. A typed local `x: Int = 0` is [planned](../../TODO.md#t-typed-local); `let` / `var` are [deprecated], replace with `x = 1`. |
| Local read/reassign | [implemented] DONE | reassignment | Ruby locals are always reassignable; `var` is [deprecated]. |
| Instance var `@x` (general read/write) | [implemented] DONE | pin `970` | `@name` reads and writes the declared field (also compound assignment). `self.name` field access is [deprecated], replace with `@name`. |
| Class var `@@x` | [planned](../../TODO.md#t-parity-language) MISSING | probe: `found At` in class body | Use a constant or a class method. |
| Global `$x` | [planned](../../TODO.md#t-parity-language) MISSING | probe: `E0006` on `$` | |
| Constant `CONST = ...` | [planned](../../TODO.md#t-constants) MISSING; today `const NAME: T = val` [deprecated] | `ConstDef`; case `44` | `const NAME: T = val`. |
| Multiple assignment `a, b = 1, 2` | [implemented] PARTIAL: inside a `def` when every target is already declared; new names and top level [planned](../../TODO.md#t-parallel-assign) | `ExprKind::MultiAssign`; case `957` | Targets and values equal length; values snapshotted before any store. Owned Class/Struct/Enum values read from a place (variable/field/index) are rejected (would alias); Copy, String, and fresh-alloc values are allowed. |
| Parallel swap `a, b = b, a` | [implemented] PARTIAL (same limits as the row above) | case `957` | Copy and String swap work (simultaneous). A swap of owned Class/Struct/Enum objects is a clean compile error for now (needs move/clone lowering). |
| Destructuring `let (a, b) = t` | [deprecated] DONE, replace with `a, b = pair` [planned](../../TODO.md#t-parallel-assign) | probe PASS; case `123` | Ruxen's substitute for LHS destructuring. |
| Splat assign `a, *b = [1,2,3]` | [implemented] PARTIAL | `MultiAssign::splat_target`; case `963` | Trailing splat supports one Array RHS or a comma-list and is formatter-preserving. `b` must be a pre-declared `Array[T]` variable; `T` is Copy-only. Leading/middle splats, short-input nil-fill, and owned-element clone lowering remain. |
| Nested destructure `(a,(b,c)) = ...` | [implemented] PARTIAL | tuple patterns only | Works for tuples; no array-splat form. |
| `\|\|=` | [implemented] PARTIAL | `PipePipeEq` token; case `954` | Desugars to `a \|\| (a = b)`. Works on a `Bool`; on a `T?` or an `Int` it is [planned](../../TODO.md#t-truthiness) (E0731 today). |
| `&&=` | [implemented] DONE | `AmpAmpEq` token; case `954` | Desugars to `a && (a = b)`. |
| `+= -= *= /= %=` | [implemented] DONE | `CompoundAssign`; probe PASS | |
| `<<= >>= &= \|= ^= **=` | [implemented] PARTIAL | pin `968` | `<<= >>= &= \|= ^=` desugar to `a = a OP b`; `**=` was not checked. |

---

## 3. Operators

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| Arithmetic `+ - * / %` | [implemented] DONE | `BinOp`; case `02` | |
| Power `**` | [implemented] DONE | `StarStar` token; `BinOp::Pow`; case `952` | Right-associative, binds tighter than unary minus (`-2 ** 2 == -4`). `Int ** Int → Int` (`ruxen_int_pow`), any float operand → `Float` (`ruxen_float_pow`). Fixed the former silent misparse (`2 ** 8` was `16`). |
| Comparison `< > <= >=` | [implemented] DONE | cases `126/209/965` | Auto-`Ord` on structs/enums; user methods and `Comparable` defaults dispatch when available. |
| Equality `== !=` | [implemented] DONE (scalars, `struct`) | auto-`PartialEq`; cases `206/965` | User `def ==` dispatches; `!=` inverts it. Auto-`PartialEq` holds for a **`struct`** (case `206`); a **`class`** compares by identity (**B74**). |
| Spaceship `<=>` | [implemented] DONE | `Spaceship` token; `BinOp::Cmp`; cases `959/965` | Returns `Int` (-1 / 0 / 1) for Int, Float, String, and user-defined `def <=>`. |
| Nil coalescing `??` | [deprecated] DONE, replace with `a \|\| b` [planned](../../TODO.md#t-truthiness) | `QuestionQuestion`; `BinOp::NilCoalesce`; case `966` | Ruxen-specific `Option[T] ?? T -> T`. Right-associative, below `\|\|` and above assignment; fallback is lazy. |
| Logical `&& \|\| !` | [implemented] DONE | `BinOp`/`UnaryOp`; case `955` | Now **short-circuit** (branch-CFG lowering). Was eagerly evaluating both operands (a correctness bug: `ptr != nil && ptr.field` touched `ptr.field` when nil); fixed 2026-07-23. |
| Keyword `and` / `or` / `not` | [implemented] DONE | `KeywordAnd`/`KeywordOr`/`KeywordNot`; case `964` | Ruby precedence: `and`/`or` share one left-associative tier below assignment; `not` consumes assignment and symbolic operators, then stops before keyword `and`/`or`. Strict-Bool, short-circuit, and formatter-preserving. |
| Bitwise `& \| ^ << >>` | [implemented] DONE | `BinOp`; case `812` | |
| Bitwise complement `~` | [implemented] DONE | `UnaryOp::BitNot`; pin `968` | Lowers to `x ^ -1`; a non-integer operand is E0733. |
| Ternary `c ? a : b` | [implemented] DONE | parser desugar to `if`/`else`; case `958` | Right-associative. Disambiguated from the `?` try operator by whitespace: an attached `x?` is try, a spaced `c ? a : b` is ternary. |
| Safe navigation `&.` | [implemented] DONE | `SafeNav`; probe PASS | `&.` not `?.`. |
| Range `.. ...` | [implemented] DONE | §1 | |
| Operator method def `+ - * / % & \| ^ << >> [] []= -@ +@ !` | [implemented] DONE | case `631/632` | Explicit `a.+(b)` and sugared `a + b` both dispatch. |
| Operator method def `== <=> < > <= >=` | [implemented] DONE | case `965` | Explicit and sugared comparison calls dispatch; primitive and auto-derived paths remain fallbacks. |
| String concat `+` | [implemented] DONE | case `317` | |
| String append `<<` | [planned](../../TODO.md#t-string-shovel) MISSING (BUG): `<<` on a `String` compiles but does not append today [known issue](../../TODO.md#t-string-shovel); `push_str` works | the 2026-07 probe only compiled it; the 2026-10-25 docs probe found the no-op | |
| `defined?(x)` | [planned](../../TODO.md#t-parity-language) MISSING | probe: undefined function | |

---

## 4. Methods & definitions

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `def name ... end` | [implemented] DONE | case `12` | |
| Implicit return (last expr) | [implemented] DONE | case `12` | |
| Explicit `return` | [implemented] DONE | case `60` | |
| Default args `def f(x = 1)` | [implemented] DONE | probe PASS; case `802/804` | `def f(x: T = default)`. |
| Named-argument call `f(w: 3, h: 4)` | [planned](../../TODO.md#t-kwargs) MISSING: call-site keywords by parameter name, Crystal's rule (OQ-4 decided 2026-11-03; a bare `*` marks keyword-only parameters). Today `f(w: 3, h: 4)` is a parse error ("expected RParen, found Colon") | 2026-11-03 probe | Wait for t-kwargs. |
| Required keyword args `def f(a:)` | [planned](../../TODO.md#t-kwargs) PARTIAL | probe: named params also callable positionally | Ruxen params are position-or-name with no *required-keyword* distinction; `f(a:)`/`**kwargs` semantics absent. |
| Positional splat `*args` | [planned](../../TODO.md#t-rest-args) MISSING | probe: `found Star` | Only FFI `is_variadic`. High-value. |
| Keyword splat `**kwargs` | [planned](../../TODO.md#t-rest-args) MISSING | probe: `found Star` | |
| Block param `&blk` + `yield` | [implemented] DONE | cases `908-912`, `93` | `&block:` param, `yield`, `block_defined?`. |
| `block_given?` | [implemented] DONE | case `909`/probe (`block_given?`) | |
| Endless def `def f(x) = expr` | [implemented] DONE | pin `969` | Top-level functions and class methods; in a `mixin` body [planned](../../TODO.md#t-implicit-self). |
| Visibility `private/protected/public` | [implemented] DONE; Ruby `protected` semantics [planned](../../TODO.md#t-protected) | section markers; `parser/methods.rs` | `private :a, :b` name-lists too. |
| `module_function` | [implemented] PARTIAL | cases `806` | Module methods via `def self.x`; `module_function` keyword surface not confirmed. |
| Singleton method `def obj.x` | [planned](../../TODO.md#t-parity-language) MISSING | probe: `found Dot` | |
| `def self.x` (class method) | [implemented] DONE | case `17/807` | |
| Operator methods | [implemented] DONE | §3 | |
| `alias new old` | [implemented] DONE | `AliasDef`; cases `913-918` | Keyword form. |
| `alias_method` (runtime) | [planned](../../TODO.md#t-alias-method) COMPILE-TIME; today use the `alias` keyword [implemented] | — | Use the `alias` keyword. |
| `super(args)` delegation | [implemented] DONE inside `init`; in any other method [planned](../../TODO.md#t-super) (E1127 today) | case `16` | Forwards to parent (esp. in `init`). |
| Bare `super` / super-as-value | [implemented] PARTIAL; `super` outside `init` [planned](../../TODO.md#t-super) | probe: tail `super()` types as `()` | Using super's return value in a typed tail position fails typeck (`expected String, found ()`). Delegation-only today. |

---

## 5. Blocks, procs, lambdas

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| Brace block `{ \|x\| ... }` | [implemented] DONE | case `28/88` | |
| `do ... end` block | [implemented] DONE | cases `88/93`, probe PASS | |
| `do..end` = immediate, `{}` = stored | [implemented] DONE | quiver DSL relies on it | Documented Ruxen block semantics. |
| `&:sym` to-proc | [implemented] DONE | case `944`; `BlockPassKind::ToProc` | |
| Numbered param `_1` | [implemented] DONE | case `943` | |
| `it` param | [implemented] DONE | probe PASS; case `943` | |
| `&block` forwarding | [implemented] DONE | case `946`; `BlockPassKind::Forward` | |
| `yield` with args | [implemented] DONE | cases `929/93` | |
| Closure bound to var + `.call` | [implemented] DONE | probe PASS | `f = ->(x: Int) { x + 1 }; f.call(2)`. |
| Stabby lambda `->(x) { x }` | [implemented] DONE | ruby-naming spec, section 5 | Owns its captures. |
| `Proc.new { }` | [implemented] DONE | ruby-naming spec, section 5 | Owns its captures. |
| `lambda { }` | [implemented] DONE | ruby-naming spec, section 5 | Owns its captures; `proc { }` too. |
| proc-vs-lambda arity/return semantics | [implemented] PARTIAL; strict arity and a proc's `return` [planned](../../TODO.md#t-lambda-call) | closures are typed; no distinct `Proc`/`Lambda` object model | Non-local `return` from a stored block is not modeled; `next` gives block value (case `945`). |
| `curry` | [planned](../../TODO.md#t-parity-language) MISSING | — | |

---

## 6. Control flow

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `if/elsif/else` | [implemented] DONE | case `07` | |
| `if` modifier `X if c` | [implemented] DONE in a method body; at top level [planned](../../TODO.md#t-toplevel-stmts) | `parse_statement`; case `953` | Parser desugar to a block `if`. |
| `unless` statement | [implemented] DONE | `parse_unless_expr`; case `953` | Desugars to `if !cond` (no `elsif`, per Ruby). |
| `unless` modifier | [implemented] DONE in a method body; at top level [planned](../../TODO.md#t-toplevel-stmts) | `parse_statement`; case `953` | `return … unless c` supported. |
| `while` | [implemented] DONE | case `08` | |
| `while` modifier `X while c` | [implemented] DONE in a method body; at top level [planned](../../TODO.md#t-toplevel-stmts) | `parse_statement`; case `953` | Condition-first (not do-while). |
| `until` statement | [implemented] DONE | `Until` token + `parse_until_expr`; case `953` | Desugars to `while !cond`. |
| `until` modifier | [implemented] DONE in a method body; at top level [planned](../../TODO.md#t-toplevel-stmts) | `parse_statement`; case `953` | |
| `begin..end while` (do-while) | [planned](../../TODO.md#t-parity-language) MISSING | probe: `found While` | |
| `for x in ...` | [implemented] DONE | cases `09/639`; probe PASS | |
| `loop` | [deprecated] DONE (`loop ... end`), replace with `loop do ... end` [planned](../../TODO.md#t-loop-do) | case `53/54` | `loop` + `end` (no `do`; `loop do` is a parse error). |
| `break` (+ value) | [implemented] DONE | cases `53/54` | |
| `next` (+ value, in block) | [implemented] DONE in blocks; a bare `next if c` and `next` inside `each` fail [known issue](../../TODO.md#t-next-block) | case `945` | |
| `continue` (in loop) | [deprecated] DONE, replace with `next` in every loop [planned](../../TODO.md#t-next-loops) | case `55` | Ruxen keyword; Ruby uses `next` for both. |
| `redo` | [planned](../../TODO.md#t-parity-language) MISSING | no token | |
| `retry` (loop) | [planned](../../TODO.md#t-exceptions) MISSING | no token | |
| `throw` / `catch` | [planned](../../TODO.md#t-parity-language) MISSING | probe: `found Colon` | |
| Ternary `? :` | [implemented] DONE | §3; case `958` | See §3 row. |
| `case/when` (`===` dispatch) | [implemented] DONE | `parse_case_when`; case `960` | Desugars to `if`/`elsif`: `when v` → `s == v`, `when a, b` → `s == a \|\| s == b`, `when lo..hi` → range membership. Subject re-evaluated per branch (v1). |
| `case/when then` inline | [implemented] DONE | case `960` | `when x then body` (and newline form) both parse. |
| `case/in` (pattern) | [implemented] DONE | `parse_case_in`; case `960` | Alias of `match`: `case s / in pat then body / … / end`. `else` becomes a wildcard arm. |
| `.times`, `.upto`, `.each` iterators | [implemented] PARTIAL | `.each` DONE (case `120`); `.times` DONE (pin `971`) | `upto` / `downto` are a follow-up (core.md lists their state). |

---

## 7. Pattern matching (`case/in`)

Ruby's `case ... in` is the pattern-matching form [implemented]: `case s / in
pat then body / … / end` (pin `960`). The older `match subj ... end` with
`pattern -> body` arms parses to the same AST and is [deprecated]. Ruby's `case ... when` (`===` dispatch) is a separate
construct that desugars to `if`/`elsif` (§6).

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `case x in PAT` keyword surface | [implemented] DONE | `parse_case_in`; pin `960` | `then` or a newline separates the pattern from the body. The older `match x / PAT -> ... / end` is [deprecated]. |
| Literal pattern | [implemented] DONE | case `10` | |
| Variable binding pattern | [implemented] DONE | case `76` | |
| Wildcard `_` | [implemented] DONE | case `10` | |
| Rest `*` / `..` in pattern | [implemented] DONE in tuple and variant patterns; the array pattern `in [first, *rest]` [planned](../../TODO.md#t-patterns) | `Pattern::Rest`; case `75` | |
| Or-pattern `a \| b` | [implemented] DONE | case `58` | |
| Guard `if` | [implemented] DONE | case `11` | |
| Tuple / array pattern `[a, b]` | [implemented] PARTIAL; array patterns [planned](../../TODO.md#t-patterns) | `Pattern::Tuple`; case `62` | Tuple destructure works; Ruby array-pattern with splat middle unverified. |
| Enum/variant pattern | [implemented] DONE | cases `18/94` | |
| Struct pattern `{ field: }` | [implemented] PARTIAL: `TypeName(field: pat)` parses; `in Point(x:, y:)` [planned](../../TODO.md#t-patterns) | `Pattern::Struct` | Struct field patterns exist; hash-value `deconstruct_keys` semantics absent. |
| Hash pattern `in { a:, b: }` | [planned](../../TODO.md#t-patterns) MISSING | needs symbols | |
| Find pattern `in [*, x, *]` | [planned](../../TODO.md#t-patterns) MISSING | — | |
| Pin operator `^var` | [planned](../../TODO.md#t-patterns) MISSING | — | |
| `deconstruct` / `deconstruct_keys` protocol | [planned](../../TODO.md#t-patterns) MISSING | — | |
| One-line `x => pat` / `x in pat` | [planned](../../TODO.md#t-parity-language) MISSING | — | |

---

## 8. Exceptions

Ruby exceptions are [planned](../../TODO.md#t-exceptions): `raise`, `begin/rescue/else/ensure`
and `retry`, checked statically. The compiler lowers them onto the machinery
that exists today (rescue → `match` on the `Err` arm, `ensure` → a `Drop` scope
guard, `raise` → early-return `Err`), so nothing needs a runtime metaobject.
Until they land, `Result[T, E]`, the postfix `?` and `panic!` are the working
spellings, and all three are [deprecated].

| Ruby feature | Status | Compile-time substitute (design) | Note |
|---|---|---|---|
| `begin/rescue/else/ensure` | [planned](../../TODO.md#t-exceptions) COMPILE-TIME | `Result` + `match`; sugar = lower `rescue` to the `Err` arm, `ensure` to a `Drop` scope guard | probe: `rescue` not parsed today. Sugar is a parser + desugar task. |
| `raise "msg"` | [planned](../../TODO.md#t-exceptions) COMPILE-TIME | early-return `Err(...)`; `panic!` for fatal | Sugar `raise X` → `return Err(X)` inside a `Result` fn. |
| `raise SomeError.new` | [planned](../../TODO.md#t-exceptions) COMPILE-TIME | `Err(MyErr.Variant(...))` | Typed error enum, not a runtime class. |
| `rescue => e` modifier | [deprecated] DONE as the postfix `?` operator, replace with nothing: exceptions propagate on their own [planned](../../TODO.md#t-exceptions) | `?` operator (case `25`) | The propagate case is already sugar. |
| Custom exception class | [deprecated] DONE (`enum` with `include Error`), replace with `class NotFound < StandardError` [planned](../../TODO.md#t-exceptions) | `enum E; include Error; def message` | cases `94/95`. |
| `retry` in rescue | [planned](../../TODO.md#t-exceptions) COMPILE-TIME | bounded loop over `Result`; a `retry(n)` combinator | cases `444-449` show the retry combinators. |
| `ensure` cleanup | [implemented] DONE through `Drop`; the `ensure` keyword [planned](../../TODO.md#t-exceptions) | ownership/`Drop` (cases `134/138`) | Deterministic destruction is strictly stronger than `ensure`. |
| Exception hierarchy / `StandardError` | [planned](../../TODO.md#t-exceptions) COMPILE-TIME | `Error` mixin + enum variants; `?`-conversion (case `95`) | Nominal error taxonomy resolved at compile time. |
| `panic!` (unrecoverable) | [deprecated] DONE, replace with `raise "msg"` [planned](../../TODO.md#t-exceptions) or `abort "msg"` [implemented] | case `96` | `panic!("msg")`. |

---

## 9. Classes & objects

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `class Name ... end` | [implemented] DONE | case `14` | |
| Fields | [implemented] DONE | `name: T` decls; case `72` | No `attr_*`; visibility via sections. |
| `attr_accessor/reader/writer` | [planned](../../TODO.md#t-attr-accessors) COMPILE-TIME | compile-time codegen that emits reader+writer methods for the listed fields | probe: keyword not parsed. Cheap codegen sugar over existing fields; a clear build item. |
| Inheritance `<` | [implemented] DONE | case `16` | |
| `super(args)` | [implemented] DONE inside `init`; elsewhere [planned](../../TODO.md#t-super) | §4 | Delegation DONE; super-as-value PARTIAL. |
| Open class / reopening — **add methods** | [planned](../../TODO.md#t-reopen-class) COMPILE-TIME; today `extension Type ... end`, which is [deprecated] | `extension Type ... end` adds methods to a type from outside (compile-time "monkey-patch") | Partly DONE via `extension`. Reopening the *same* `class` name is `E0727`; the sanctioned surface is `extension`. |
| Open class / reopening — **repatch already-compiled 3rd-party class at runtime** | [retired] OUT-OF-SCOPE | vendor the type + `extension`, or a fork | The only irreducible slice: mutating code that is already compiled. |
| `class << self` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | `def self.x` today; a `class << self ... end` block that rewrites to `self.`-methods | probe: `found Class`. Parser sugar over the existing class-method path. |
| Singleton method `def obj.x` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | for a *type* → `extension`; per-instance singleton → not modeled (would need a per-object vtable) | Per-instance case is the rare, near-irreducible slice; the type-level case is `extension`. |
| Class method `def self.x` | [implemented] DONE | case `17` | |
| Constants in class | [planned](../../TODO.md#t-nested-class) MISSING (the ruby-naming spec, section 9, supersedes the older DONE) | case `44` | |
| `attr`-style equality/hash/inspect | [implemented] PARTIAL (`struct` only) | cases `133/207/139` | Auto `PartialEq`/`Hash`/`Debug`. All three cases pin a **`struct`**. For a **`class`**, structural `==` returns `false` with or without an explicit `include PartialEq` (**B74**), so a `class` also cannot serve as a `Hash`/`Set` key. |
| `to_s` universal | [implemented] DONE; interpolation ignores a user `to_s` [known issue](../../TODO.md#t-interp-to-s) | case `810` | User `to_s` overrides derived. |
| `<=>` + `Comparable` (hand-written) | [implemented] DONE (operators) | case `965` | Required `<=>` supplies default `==`, ordering operators, and `between?`; `clamp` remains an ownership-design follow-up. |
| `Struct.new(:a, :b)` | [planned](../../TODO.md#t-struct-new) COMPILE-TIME | a `Struct.new(...)` macro that expands to a `struct` with accessors + auto-included mixins | `struct` is the mechanism (case `63`); the `Struct.new` *surface* is a macro over it. |
| `Data.define(:a, :b)` | [planned](../../TODO.md#t-data-define) COMPILE-TIME | macro → immutable `struct` (no writers) + value semantics + auto-included mixins | Same design as `Struct.new`, writers omitted. |
| `Comparable` protocol (auto) | [implemented] DONE | case `126/209` | Via derived `Ord`. |
| `Enumerable` protocol | [implemented] DONE as `include Enumerable[T]`; plain `include Enumerable` [planned](../../TODO.md#t-enumerable-infer) | cases `612/613/936` | `include Enumerable`. |

---

## 10. Modules & mixins

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `module M ... end` (namespace) | [implemented] DONE | case (module_method probe) | Namespacing + `M.fn`. |
| `module` as mixin via `include` | [planned](../../TODO.md#t-module-mixins) MISSING | probe: `include <module>` → `no field hello` | Method-bearing includables must be `mixin`, not `module`. |
| `mixin M` + `include M` | [deprecated] DONE, replace with `module` and `include` [planned](../../TODO.md#t-module-mixins) | cases `21/22/86` | Ruby-`module`-include equivalent. |
| `include` (instance methods) | [implemented] DONE | case `21` | |
| `prepend` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | a `prepend` mixin whose methods win over the type's own, `super` chaining to the original | Method-resolution order is decided at compile time; `prepend` = insert the mixin ahead of the type in that fixed order. |
| `extend` (add as class/singleton methods) | [planned](../../TODO.md#t-extend) COMPILE-TIME | `extension` for a type; a `module`-as-class-methods `extend` that lifts the module's `def`s to `self.` methods | probe: include-only today. Compile-time lift, no runtime singleton. |
| Default methods in mixin | [implemented] DONE | cases `22/86/87` | |
| Mixin static/assoc-type/generic | [implemented] DONE | cases `80/81/655-659` | Beyond Ruby (typed mixins). |
| MRO / ancestor linearization | [implemented] COMPILE-TIME (static resolution works); Ruby's "last included module wins" rule [planned](../../TODO.md#t-oq) | static resolution | No runtime `ancestors`; resolution is compile-time. |
| `refinements` / `using` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | lexically-scoped `extension` activated per-module/per-import | Ruby refinements are already lexically scoped; a scoped `extension` covers the real use case at compile time. Runtime activation toggling is the only irreducible edge. |
| `Comparable`/`Enumerable` includes | [implemented] DONE | §9 | Both use shared `.rx` default bodies over one required primitive. |
| `require` / `require_relative` | [implemented] DONE | `module_loader.rs` (2026-07-14) | Ruby-style AOT module loading. |
| `autoload :Const, "path"` | [implemented] DONE | `module_loader::expand_autoloads` | Unresolved-name fixpoint. |

---

## 11. Metaprogramming

**The doctrine: do at compile time what Ruby does at runtime.** Ruby generates
and dispatches methods from a live metaobject; Ruxen is a pure compiler, so each
feature below gets a **compile-time design that produces the identical result**
without the program mutating itself at runtime. The recurring moves are: (a) a
**literal symbol/name folds to a direct reference** at compile time; (b) a
**bounded, compile-time-known method/field set** becomes generated real methods
or a `match`; (c) a **schema/attribute** drives codegen (mixins / a macro /
attribute-driven expansion). Only string-`eval`, `binding`, `TracePoint`,
`ObjectSpace`, and repatching already-compiled code are irreducible.

*Caution (measured):* several reflection calls **appear to compile inside string
interpolation** because the interpolation re-lexer is permissive, then parse-fail
standalone and emit empty output at runtime. They are not functional today; the
`COMPILE-TIME` status marks the design to build, not a working feature.

| Ruby feature | Status | Compile-time design that delivers the same result | Note / irreducible edge |
|---|---|---|---|
| `define_method(:m) { ... }` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | attribute/macro that expands a declarative spec into concrete `def`s at compile time (the "dynamic finders / generated accessors" use case); mixins for the standard families | Body known at compile time → real methods. probe: `found LParen`. |
| `method_missing` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | for delegation/proxy/dynamic-attr (the real uses), a macro that enumerates the fixed, schema-derived name set and emits forwarding methods; a `Hash`-backed dynamic object for open key access | Genuinely-unbounded `obj.anything` with no schema is the only irreducible edge. probe: needs `Symbol`. |
| `respond_to_missing?` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | pairs with the above: the generated method set is known, so membership is a compile-time constant | |
| `send(:m, ...)` / `public_send` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | literal symbol → direct call `obj.m(...)`; runtime symbol from a bounded set → generated `match sym { :m -> obj.m(...) , ... }` | Unbounded runtime symbol → the `match` still covers any declared method. |
| `respond_to?(:m)` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | literal symbol → constant-fold `true`/`false` from the static type; dynamic → `match` over the known method set | Method set is a compile-time fact. |
| `instance_variable_get/set(:@x)` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | literal name → field access `obj.x`; dynamic name → `match` over the fixed field set, or a `Hash`-backed object for open attributes | Field set is fixed and known. probe: empty at runtime today. |
| `.class` / `.class.name` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | emit the static type name as a compile-time `String` constant; a `TypeId`-style value for comparisons | No reflective mutable class object; the *name/identity* use case folds to a const. |
| `is_a?` / `kind_of?` / `instance_of?` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | literal type → constant-fold from the static type; sum-type dispatch → `match` over enum variants | Static types make the answer a compile-time fact. probe: `no method is_a?`. |
| `const_get` / `const_set` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | literal name → direct constant reference; dynamic over a known module → generated `match`; `const_set` of a *new* name → not expressible (use a macro to declare it) | Declaring a brand-new constant from runtime data is the irreducible edge. |
| `define_singleton_method` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | type-level → `extension`/macro emitting the method; per-instance → not modeled | Per-instance singleton is the rare irreducible edge (needs per-object vtable). |
| `class_eval`/`instance_eval` **(block form)** | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | macro that splices the compile-time-known block into the class/instance body | The block is code, so it is compile-time-known. |
| `eval("string")` | [retired] OUT-OF-SCOPE | none for arbitrary strings; templated codegen covers "generate code from a schema" | Irreducible: code unknown until runtime. |
| `binding` | [retired] OUT-OF-SCOPE | an explicit struct capturing the locals you actually need | Irreducible: needs a live environment + eval. |
| `ObjectSpace` (heap walk) | [retired] OUT-OF-SCOPE | an explicit registry the program maintains (compile-time-inserted registration on construct) | Irreducible: no GC/heap-walk runtime. |
| `TracePoint` | [retired] OUT-OF-SCOPE | compile-time instrumentation: attribute-driven codegen inserting enter/exit hooks (covers profiling/tracing) | Irreducible only for *arbitrary runtime* hook installation. |
| `Method` / `UnboundMethod` objects | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | `obj.method(:m)` with a literal → a typed closure `{ obj.m }`; `UnboundMethod` → a fn value taking the receiver | First-class typed function values replace method objects. |
| `tap` | [implemented] DONE | `.rx` `mixin Kernel`; pin `973` | A universal block method that returns the receiver. |
| `then` / `yield_self` | [implemented] DONE | `.rx` `mixin Kernel`; pin `973` | Returns the block result. |
| `delegate` / `Forwardable#def_delegators` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | a `delegate :a, :b, to: :field` macro emitting concrete forwarding methods | Fixed method list → real methods. |

---

## 12. Misc / special

| Ruby feature | Status | Ruxen location or gap | Note |
|---|---|---|---|
| `puts` / `print` accept any value | [implemented] DONE | typeck `to_s` conversion; case `956` | A non-String arg is converted through its universal `to_s` (HIR only, so the AST and `ruxen fmt` are unchanged). A value with no `to_s` (Array, Option, tuple, a struct without a `Debug`/`Display` include) gives a clean compile error, not a link error. |
| `puts a, b` (multi-arg) | [planned](../../TODO.md#t-parity-language) MISSING | probe: `found Comma` | Single arg only; multi-arg is a follow-up. |
| String `%` formatting `"%05d" % x` | [implemented] DONE | probe PASS | |
| `format` / `sprintf` | [planned](../../TODO.md#t-d9) MISSING | probe: undefined function | `%` operator works; the functions do not. |
| `p` / `pp` (inspect) | [implemented] PARTIAL: `p` takes any value that includes `Inspect` (ruby-naming spec, section 12.1); `pp` per core.md | — | |
| Line comment `#` | [implemented] DONE | `lexer` | |
| Block comment `#= ... =#` | [implemented] DONE; kept until `=begin` lands (OQ-12) | case `110` | Ruxen form. |
| `=begin ... =end` block comment | [planned](../../TODO.md#t-begin-comment) MISSING | probe: `found Eq` | Use `#= =#`. |
| Doc comment `##` | [implemented] DONE | cases `111/130` | |
| `frozen_string_literal: true` magic | [implemented] DONE (inert) | probe PASS | Accepted as a comment; strings are owned by default, so it is a no-op. |
| `BEGIN {}` / `END {}` | [planned](../../TODO.md#t-parity-language) MISSING | — | |
| `defined?` | [planned](../../TODO.md#t-parity-language) MISSING | §3 | |
| `__method__` / `__FILE__` / `__LINE__` | [planned](../../TODO.md#t-parity-language) MISSING | §1 | |
| `<=>` spaceship | [implemented] DONE | §3; case `959` | See §3 row. Int/Float/String. |
| `and`/`or`/`not` | [implemented] DONE | §3; case `964` | |
| `nil?` universal predicate | [planned](../../TODO.md#t-parity-language) MISSING | probe: `no field nil? on Int` | Non-nullable types make it moot; `Int?` (Option) uses `match`/`?`. |

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
11. ~~**`<=>` spaceship + user-definable `==`/`<=>`.**~~ **DONE** for
    comparison operators and `between?`: primitive `<=>` pin `959`,
    hand-written `Comparable` and user comparison methods pin `965`.
    `Comparable#clamp` remains an ownership-design follow-up. (§3, §9)
12. **`*args` / `**kwargs` splat params** and required-keyword args `def f(a:)`. (§4)
13. ~~**General `@ivar` read/write** beyond the `init` auto-assign sugar.~~ **DONE** (`@name` in expression position desugars to `self.name`; read, write, and compound-assign; fmt renders `self.name`, surface-preserving `@name` is a follow-up). Pin `970`. (§2)
14. ~~**`case/when` keyword surface** (`===` dispatch) as sugar over `match`.~~ **DONE** (`case/when` desugars to `if`/`elsif`; `case/in` is an alias of `match`). Pin `960`. (§6)
15. **`Integer#times` / `#upto` / `#downto`** iterators. **PARTIAL** — `times` DONE (pin `971`); `upto`/`downto` are a follow-up (they need `self` in arithmetic, but a scalar method-home `self` is nominal `Class[Int]` and does not yet flow into the primitive `+`/`-` floor). (§6)
16. ~~**`tap` and `then`/`yield_self`.**~~ **DONE** (universal block methods intercepted in typeck + MIR; `tap` returns the receiver, `then`/`yield_self` return the block result; `.then` parses as a contextual-keyword method name). Pin `973`. (§11)

#### P2 — completeness, lower frequency

17. **Heredocs `<<~ID` / `<<-ID` / `<<ID`** (triple-quote exists as a partial substitute). (§1)
18. ~~**Endless method def `def f(x) = expr`.**~~ **DONE** (a `def` signature followed by `=` takes a single-expression body, no `end`; top-level fns and class methods; fmt normalizes to `{ expr }`, surface-preserving `= expr` is a follow-up). Pin `969`. (§4)
19. **`%w` / `%i` / `%q` / `%Q` / `%r` percent-literals.** **PARTIAL** — `%w[...]` word arrays DONE (`[]`/`()`/`{}`/`<>` delimiters, expression-context only so `a % w` modulo is safe; pin `974`). `%i` needs a `Symbol` type; `%q`/`%Q`/`%r` (alternate string/regex quotes) are a follow-up. (§1)
20. **Beginless range `(..5)`.** (§1)
21. ~~**`~` bitwise complement**; bitwise/shift compound-assigns `<<= &= |= ^=`.~~ **DONE** (`~` is `UnaryOp::BitNot` lowering to `x ^ -1`, non-integer → E0733; `<<= >>= &= |= ^=` desugar to `a = a OP b`). Pin `968`. (§2, §3)
22. **`defined?`.** (§3)
23. **`redo`; `begin..end while` do-while.** (§6)
24. **Full `case/in` pattern surface**: hash patterns, find patterns, pin `^`, `deconstruct`/`deconstruct_keys`, one-line `=>`/`in`. (§7)
25. ~~**Stabby lambda `->(x){}`, `Proc.new`, `lambda`.**~~ **DONE** (closures that own their captures). `curry` remains. (§5)
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
B7. **`begin/rescue/ensure`, `raise` and `retry`** over `Result`/`Drop`, [planned](../../TODO.md#t-exceptions). (§8)
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
