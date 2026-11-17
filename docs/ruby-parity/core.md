# Ruby core-class parity, the exhaustive audit

Authoritative, no-skip backlog for **core-class method parity** between Ruby
(CRuby / MRI 3.x) and Ruxen. One table per Ruby core class; a row per real Ruby
method. See [`README.md`](README.md) for the tier framework.

Status vocabulary:

| Status | Meaning |
|--------|---------|
| **DONE**, tagged [implemented] | Present in Ruxen, verified to compile **and run correctly**. |
| **PARTIAL**, tagged [implemented]; the note names the gap | Declared / compiles but has a real gap: wrong at runtime, receiver-shadowed, element-type-restricted, or a narrower signature than Ruby. |
| **MISSING**, tagged [planned](../../TODO.md#t-parity-core) (or the more specific tracker item the row links) | No usable Ruxen equivalent (typeck rejects, or links to a non-existent symbol). |
| **COMPILE-TIME**, tagged [planned](../../TODO.md#t-parity-compile-time) unless the row says it already works | Re-expressed as a Ruxen language construct / mixin, not a runtime method (tier 2). |
| **OUT-OF-SCOPE**, tagged [retired]: Ruxen will not build it | Needs a runtime metaobject / interpreter Ruxen deliberately lacks (tier 3). |

## Methodology (how each status was established)

Ruxen's core surface lives in `library/std/{string,array,map,set,scalar,
option_result,core,regex,time}/src/*.rx` (FFI aliases + `Enumerable[T]` mixin
bodies), backed by C in each package's `runtime/`. That `.rx` surface was read
directly, then **every** status below was probed against the installed
toolchain (`ruxen 0.1.0`, `~/.ruxen/bin/ruxen compile`).

Three probe traps were discovered and controlled for, they matter for anyone
extending this audit:

1. **No-arg unknown calls** error cleanly at typeck (`no field X` / `no method
   X`). Reliable.
2. **With-arg / block-form calls on generic builtins** (`Array`/`Hash`/`Set`/
   `Option`/`Result`) *pass* typeck via a permissive monomorphization fallback
   and only fail later, at Ruxen codegen (`no runtime symbol`) or the C linker
   (`Undefined symbols`). So "compiles" ≠ "exists"; every generic-collection
   method was driven through a **full compile + link**.
3. **Compiles ≠ works.** Several operators link to *a* symbol but produce
   garbage or crash at runtime. So a representative sample (and every operator)
   was **compiled, run, and checked against an expected value**.

## ⚠️ Operator reality (verified by running, not just compiling)

This is the highest-value finding and colors every table below. Ruxen's arithmetic
baseline works; much of the rest **compiles but is broken**:

| Operator(s) | Status | Evidence |
|-------------|--------|----------|
| Int/Float `+ - * / %`, unary `-` | [implemented] **DONE** | `2+3`, `7/2`, `7%2`, `-5` all correct. |
| Int/Float `< <= > >= == !=` | [implemented] **DONE** | all correct. |
| `&& \|\| !`, Bool `& \| ^` | [implemented] **DONE** | correct. |
| Int `<< >> & \| ^` (bitwise) | [implemented] **DONE** | `1<<4`=16, `6&3`=2 correct. |
| String `+`, String `==` | [implemented] **DONE** | `"ab"+"cd"`="abcd". |
| `Array[i]`, `Array[i]=`, `Hash[k]`, `Hash[k]=` | [implemented] **DONE** | `a[1]`=20, `a[0]=9`, `h["a"]` all correct. `Hash[k]=` was a SILENT NO-OP until B72 (pins `1710`/`1711`): the row claimed DONE while the store was discarded with no diagnostic. Both `Hash` construction spellings (a typed binding `h: Hash[K, V] = Hash.new`, which today needs the [deprecated] `let`/`var` inside a `def` until typed locals land [planned](../../TODO.md#t-typed-local), and `Hash[K, V].new`) are now pinned, `1728`/`1729`. |
| `Array[range]` (e.g. `a[0..1]`) | [implemented] DONE | compiles+runs. |
| **`**` (power)** | [implemented] **DONE** | `2**8`⇒256. `StarStar` token + `BinOp::Pow` → `ruxen_int_pow`/`ruxen_float_pow` (pin 952). |
| **`<=>` (spaceship)** | [implemented] **DONE** | Primitive Int/Float/String pin `959`; user method and Comparable dispatch pin `965`. |
| **String `*`** | [implemented] **DONE** | `"ab"*3`⇒"ababab". `.rx` `def *`→`repeat`, builtin-head operator route (pin 984). |
| **String `<<`** | [planned](../../TODO.md#t-string-shovel) **MISSING (BUG)** | no-op (receiver unchanged). Use `.push_str`. (`<<` mutation semantics pending.) |
| **String `[i]` (index)** | [planned](../../TODO.md#t-parity-core) **MISSING (BUG)** | `"abc"[1]` ⇒ empty/null Char. No `[]` in `string.rx`. |
| **Array `+`** | [implemented] **DONE** | `[1,2]+[3,4]`⇒`[1,2,3,4]`. `.rx` `def +`→`chain` (pin 984). **`- * &  \|`** still MISSING (need element-eq set ops). |
| **Array `<<`** | [planned](../../TODO.md#t-parity-core) **MISSING (BUG)** | no-op. Use `.push(x)`. Source check 2026-11-03: `library/std` declares no `def <<` for `Array` (only `push`, `library/std/array/src/lib.rx:87`), so the operator has no method to reach; not re-probed. |
| **Set `\| & -`** | [implemented] **DONE** | `.rx` `def \|`/`&`/`-`→`union`/`intersection`/`difference` (pin 984). **`^`** (symmetric diff) MISSING. |

**Also broken:** `Range#map` returns empty (though `for x in a..b` iteration
works). (`Array[Int]#join` is fixed — see `join` in the Array table.)

These runtime-broken operators are recorded as MISSING (they are not usable),
with the working method spelling noted. Fixing `**` and the collection
operators is high-value backlog.

---

## String  (`library/std/string/src/string.rx`)

Present surface is thin and uses Ruxen (not always Ruby) spellings. Verified
working: `size`, `empty?`, `include?`, `starts_with`, `ends_with`, `find`,
`trim*`, `to_lower`/`to_upper`, `repeat`, `replace`, `split`, `splitn`, `chars`,
`lines`, `bytes`, `parse_int`/`parse_float`, `push`/`push_str`, `clone`.

| Ruby method | Status | Ruxen symbol / gap | Note |
|---|---|---|---|
| `length` | [implemented] PARTIAL | `ruxen_string_len` | Ruby `length` aliased to the same C symbol as `size`. Pin `972`. **Counts BYTES, not characters**, so it diverges from CRuby for non-ASCII text (`"héllo".length` is 6, Ruby says 5). `chars.size` / `codepoints.size` are the character counts. Changing it is a cross-cutting call (HTTP framing in `library/std/http` relies on the byte semantics), so it is recorded here rather than changed inside a String batch. |
| `size` | [implemented] PARTIAL | `ruxen_string_len` | Same byte-vs-character divergence as `length`. |
| `bytesize` | [implemented] DONE | `.rx` over `bytes` (pin 1071) | byte length, distinct from the character count. |
| `+` (concat) | [implemented] DONE | operator | `"a"+"b"`. |
| `*` (repeat) | [implemented] DONE | `def *`→`repeat` (pin 984). |
| `<<` / `concat` | [planned](../../TODO.md#t-string-shovel) MISSING (BUG) | use `push_str` | `<<` is a no-op. |
| `%` (format) | [planned](../../TODO.md#t-parity-core) MISSING | use `"#{}"` interp | no `String#%`. |
| `[]` / `slice` (index) | [implemented] PARTIAL | `.rx` `slice(start, len)` (pin 1092) | the two-argument `slice` is DONE, returning `String?` with Ruby's exact out-of-range/negative-start edges. The `[]` operator, the one-argument index form and the Range form are still MISSING (`"abc"[1]` yields a null Char). |
| `at` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `byteslice` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `sub` / `sub!` | [implemented] PARTIAL | `.rx` `sub(pattern, replacement)` (pin 1090), regex form pin 1101 | string and `Regex` first-match forms both present. `sub!` MISSING (no in-place variant is expressible; see `strip!`). The block form needs a block-taking overload. |
| `gsub` / `gsub!` | [implemented] PARTIAL | `.rx` `gsub(pattern, replacement)` (pin 1090), regex form pin 1101 | string and `Regex` replace-all forms both present. Implemented in `.rx` rather than delegating to the FFI `replace`, which leaves an EMPTY pattern alone where Ruby matches it at every position. `gsub!` MISSING (no in-place variant is expressible). The block form needs a block-taking overload. |
| `tr` / `tr_s` | [implemented] DONE | `.rx` (pin 1082) | tr-style sets with `a-z` ranges, `^` negation and `\\` escapes; a short `to` is padded with its last character and an empty `to` deletes. |
| `replace` (whole-string) | [planned](../../TODO.md#t-parity-core) MISSING | — | the NAME is taken: Ruxen's `replace(from, to)` is a replace-all, i.e. its `gsub`. Ruby's one-argument `replace(other)` overwrites the receiver in place, which is not expressible (see `strip!`). Use `gsub` for the two-argument meaning. |
| `split` | [implemented] PARTIAL | `ruxen_string_split` + `.rx` `split(sep, limit)` (pin 1092) and `split(re)` (pin 1101) | string, limit and `Regex` forms present. NOTE the one-argument `split(sep)` keeps trailing empty fields, so it matches Ruby's `split(sep, -1)`, not Ruby's default; pass an explicit `0` for Ruby's default. No awk-style whitespace-splitting default. |
| `splitn(n, delim)` | [implemented] DONE | `ruxen_string_splitn` | Ruxen-specific limited split. |
| `strip` | [implemented] DONE | `ruxen_string_trim` | Ruby `strip` aliased to the same C symbol as `trim`. Pin `975`. |
| `lstrip` / `rstrip` | [implemented] DONE | `trim_start` / `trim_end` | Ruby spellings aliased to the same C symbols. Pin `975`. |
| `strip!`/`lstrip!`/`rstrip!` | [planned](../../TODO.md#t-parity-core) MISSING | — | no in-place variants, and none are expressible: inside `class String` the receiver is the FFI shell (a buffer pointer held BY VALUE), so a `.rx` body cannot rewrite the caller's string, and growing it needs a realloc whose new pointer has nowhere to go. Needs a `&var self` receiver. |
| `chomp` / `chop` | [implemented] DONE | `.rx` (pin 1080) | `chomp` has both arities (no-arg drops one `\r\n` / `\n` / `\r`; `chomp("")` drops every trailing terminator); `chop` counts a trailing `\r\n` as one character. |
| `upcase` | [implemented] DONE | `to_upper` | Ruby `upcase` aliased to the same C symbol; `to_upper` also kept. Pin `972`. |
| `downcase` | [implemented] DONE | `to_lower` | Ruby `downcase` aliased to the same C symbol; `to_lower` also kept. Pin `972`. |
| `capitalize` / `swapcase` | [implemented] DONE | `.rx` over `chars` (pins 987, 1080) | `swapcase` is ASCII-only, matching `upcase` / `downcase`, which are per-byte C `toupper` / `tolower`. |
| `upcase!`/`downcase!`/`capitalize!`/`swapcase!`/`squeeze!`/`sub!`/`gsub!`/… | [planned](../../TODO.md#t-parity-core) MISSING | — | no `!` variant of ANY String method, and none are expressible for the same reason as `strip!` above. Every method here returns a new String. |
| `start_with?` | [implemented] PARTIAL | `ruxen_string_starts_with` + `.rx` array overload (pins 975, 1072) | single `&String` arg, plus an `&Array[String]` overload standing in for Ruby's splat. No regex form. An untyped `[]` literal cannot pick between the two overloads: use a typed local. |
| `end_with?` | [implemented] PARTIAL | `ruxen_string_ends_with` + `.rx` array overload (pins 975, 1072) | as `start_with?`. |
| `include?` | [implemented] DONE | `ruxen_string_contains` | |
| `index` | [implemented] DONE | `.rx` `index(sub, offset = 0)` (pin 1070) | CHARACTER index, `Int?`, negative offset counts from the end. The pre-existing `find` stays as the BYTE-offset spelling. Regex form: see `match`. |
| `rindex` | [implemented] DONE | `.rx` `rindex(sub)` / `rindex(sub, offset)` (pin 1070) | two arities rather than a default argument: a NEGATIVE default argument mis-lowers on a builtin head. |
| `count` | [implemented] PARTIAL | `.rx` `count(set)` (pin 1071) | Ruby tr-style sets including `a-z` ranges, `^` negation and `\\` escapes. Ruby's multi-argument intersection form needs a splat Ruxen does not have. |
| `delete` / `squeeze` | [implemented] DONE | `.rx` (pin 1082) | tr-style sets, shared with `count` / `tr`. `squeeze` has both arities (no-arg collapses every run). |
| `center` / `ljust` / `rjust` | [implemented] DONE | `.rx` (pins 988, 989, 1081) | `pad` defaults to a space and is CYCLED one character at a time, so the result is exactly `max(width, length)` characters. A multi-character pad previously overshot the width. |
| `chars` | [implemented] DONE | `ruxen_string_chars` → `Array[Char]` | |
| `bytes` | [implemented] DONE | `ruxen_string_bytes` → `Array[UInt8]` | |
| `codepoints` | [implemented] DONE | `.rx` over `chars` (pin 1071) | |
| `lines` | [implemented] DONE | `ruxen_string_lines` | |
| `each_char` / `each_line` / `each_byte` | [implemented] PARTIAL | `.rx` block forms (pin 1073) | the block is required (no Enumerator return) and the return is `nil`, not `self`. `each_line` strips the line terminator, where CRuby keeps it. |
| `each_grapheme_cluster` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `scan` | [implemented] PARTIAL | `.rx` `scan(re)` (pin 1101) | returns `Array[String]` of whole matches. Ruby's array-per-match form for a pattern WITH groups is absent: use `Regex#scan` and read `Match#group`. |
| `match` / `match?` / `=~` | [implemented] PARTIAL | `.rx` `match(re)` / `match?(re)` / `index(re)` (pin 1101) | `match` returns `Match?`, `match?` a Bool. `=~` CANNOT be defined: Ruxen's lexer has no `=~` token (its match operator is `~=`, a Bool-returning dedicated route). Ruby's `=~` answer, the character index of the match, is available under Ruby's other name for it, `index(regexp)`. |
| `to_i` / `to_f` | [implemented] DONE | `.rx` `to_i(base = 10)` / `to_f` (pin 1100) | Ruby's lenient semantics: leading whitespace and sign, `_` between digits, a `0x` / `0b` / `0o` / `0d` prefix when it agrees with `base`, scanning stops at the first bad character, and 0 when nothing parses. The strict `Result`-returning `parse_int` / `parse_float` remain. CAVEAT: the accumulator is an `Int`, so a value past 64 bits WRAPS where CRuby promotes to a Bignum. |
| `parse_int`/`parse_float`/`parse_uint` | [implemented] DONE | `ruxen_string_parse_*` | Ruxen-native, `Result`-returning. |
| `to_sym` / `intern` | [planned](../../TODO.md#t-symbols) MISSING | — | blocked on the Symbol type (a language-design decision), not a metaobject need. |
| `to_r` / `to_c` | [planned](../../TODO.md#t-parity-core) MISSING | — | no Rational/Complex. |
| `to_s` / `to_str` | [implemented] DONE | `ruxen_string_to_string`, `.rx` `to_str` (pin 1102) | |
| `ord` / `chr` | [implemented] DONE | `.rx` (pin 1102) | `ord` on an EMPTY receiver aborts via `panic!`: Ruby raises ArgumentError and Ruxen has no exception surface, so aborting beats inventing a 0. |
| `hex` / `oct` | [implemented] DONE | `.rx` over `to_i(base)` (pin 1100) | `oct` reads the base from the receiver's own prefix (`0x`→16, `0b`→2, `0d`→10, otherwise 8), as in Ruby. |
| `succ` / `next` | [implemented] DONE | `.rx` (pin 1083) | full carry semantics: rightmost alphanumeric, carrying leftwards across alphanumerics and skipping non-alphanumerics, seeding `"1"` / `"a"` / `"A"` on overflow. The no-alphanumeric path carries at the CHARACTER level where CRuby carries at the BYTE level; the two agree for every ASCII string. |
| `reverse` | [implemented] DONE | `.rx` over `chars`+`Array#reverse` (pin 987). |
| `insert` | [implemented] PARTIAL | `insert(i, codepoint)` / `insert_str(i, s)` | Ruxen splits into codepoint- vs string-insert; arg is an Int codepoint, not a String, for `insert`. |
| `prepend` | [implemented] PARTIAL | `.rx` (pin 1084) | returns a NEW String; Ruby mutates the receiver and returns self. |
| `delete_prefix` / `delete_suffix` | [implemented] DONE | `.rx` (pin 1084) | |
| `partition` / `rpartition` | [implemented] DONE | `.rx` (pin 1091) | three-element `Array[String]`; string separators only. |
| `freeze` / `frozen?` | [planned](../../TODO.md#t-parity-core) MISSING | — | no freeze protocol on values. |
| `dup` / `clone` | [implemented] DONE | `clone`, `.rx` `dup` (pin 1102) | both are an owned copy. Ruby's `clone` also copies frozen state and singleton methods, neither of which Ruxen has. |
| `hash` | [planned](../../TODO.md#t-parity-core) MISSING | — | (`Hashable` mixin exists for user types; no `String#hash` surface.) |
| `eql?` / `casecmp` / `casecmp?` | [implemented] DONE | `.rx` (pin 1072) | `casecmp` returns -1/0/1, ASCII folding only (CRuby folds full Unicode). Ruby's nil-for-a-non-String return has no analogue: the parameter is statically `&String`. `<=>` on String is still absent. |
| `encode`/`force_encoding`/`encoding`/`valid_encoding?`/`b`/`scrub` | [planned](../../TODO.md#t-parity-core) MISSING | — | no encoding API (strings are UTF-8 bytes). |
| `unpack` / `unpack1` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `getbyte` / `setbyte` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `sum` / `crypt` / `unicode_normalize` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `upto` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `format` / `sprintf` (as method) | [planned](../../TODO.md#t-d9) MISSING | use `"#{}"` interp | |
| `clear` / `truncate(n)` | [implemented] DONE | `ruxen_string_clear`/`_truncate` | mutating (`var`). |
| `remove(i)` | [implemented] DONE (Ruxen-native) | `ruxen_string_remove` → Char | removes+returns codepoint at `i`. |
| `new` / `with_capacity` / `from_bytes` | [implemented] DONE | class methods | builders. |

---

## Symbol

| Ruby feature | Status | Note |
|---|---|---|
| `:sym` literal | [planned](../../TODO.md#t-symbols) MISSING | `:foo` is a **parse error** (`found Colon`); no `SymbolLiteral` token in the lexer. A Symbol type is a language-design decision, not a metaobject gap. |
| Symbol type + `to_s`/`to_sym`/`to_proc`/`length`/`[]`/`upcase`/`<=>`/`succ`/`id2name` | [planned](../../TODO.md#t-symbols) MISSING | No Symbol value type exists. |

Ruxen uses **bare string literals** (owned `String`) where Ruby uses symbols
(DSL keys, etc.). A Symbol type is not currently part of the language.

---

## Integer  (`library/std/scalar/src/lib.rx`, `class Int`)

The `Integer` surface landed in batches **B12 / B13 / B14** (pins `1110`,
`1111`, `1120`, `1121`, `1130`, `1131`), all in `.rx` on the `class Int`
method-home. Every expected output in those pins was diffed line for line
against CRuby 3.3.

Two properties apply to the WHOLE table and are not repeated per row:

* **`Int` is i64, not a Bignum.** CRuby promotes on overflow; Ruxen wraps
  (`9223372036854775807 + 1` is `-9223372036854775808`). Any row whose Ruby
  answer can leave the i64 range is PARTIAL for that reason alone.
* **Division by zero traps** in hardware, exactly as the bare `/` operator
  does. CRuby raises `ZeroDivisionError`; Ruxen has no `raise` yet (batch
  B30), so the domain errors Ruby raises are documented per row as a
  total-function fallback instead.

| Ruby method | Status | Note |
|---|---|---|
| `+ - * / %` | [implemented] DONE | operators. `/` and `%` TRUNCATE (C), where Ruby's floor. Use `div` / `modulo` for Ruby's answers. |
| `**` (pow) | [implemented] DONE | `StarStar` token + `BinOp::Pow` → `ruxen_int_pow` (pin 952, see "Operator reality"). The older "lowered to `*`" note predates the fix. |
| `divmod` / `div` / `modulo` | [implemented] DONE | FLOORED, Ruby's convention: `-7.div(2) == -4`, `-7.modulo(2) == 1`. Pin `1120`. |
| `remainder` | [implemented] DONE | TRUNCATED, sign of the dividend (`-7.remainder(2) == -1`). Pin `1120`. |
| `fdiv` | [implemented] DONE | Value matches; a whole Float RENDERS as `2` where CRuby prints `2.0` (pre-existing `ruxen_float_to_string` gap). |
| `gcd` / `lcm` / `gcdlcm` | [implemented] DONE | Non-negative results; `0.gcd(0) == 0`, `0.lcm(5) == 0`. Pin `1121`. |
| `abs` / `magnitude` | [implemented] DONE | `magnitude` is an `alias`. |
| `ceil` / `floor` / `round` / `truncate` | [implemented] DONE / PARTIAL | Precision arg included; `round` is half-away-from-zero like CRuby. PARTIAL only for `ndigits <= -19`, which leaves the i64 range (returns 0). Pin `1111`. |
| `times` | [implemented] DONE | Pre-existing; pinned by `1130`. |
| `upto` / `downto` / `step` | [implemented] DONE / PARTIAL | Block-taking `.rx` methods; both `step` arities. PARTIAL: `step(limit, 0)` yields nothing where CRuby raises `ArgumentError`. Pin `1130`. |
| `pred` / `succ` / `next` | [implemented] DONE | `next` is an `alias` of `succ`. |
| `to_s` | [implemented] DONE | `ruxen_int_to_string`. |
| `to_s(base)` | [implemented] DONE / PARTIAL | Arity-1 `.rx` overload, bases 2..36, correct for i64-min. PARTIAL: an out-of-range base falls back to 10 where CRuby raises `ArgumentError`. Pin `1131`. |
| `to_i` / `to_int` | [implemented] DONE | identity; `to_int` is an `alias`. |
| `to_f` | [implemented] DONE | `ruxen_int_to_f`. |
| `to_r` / `to_c` | [planned](../../TODO.md#t-parity-core) MISSING | no Rational/Complex. |
| `digits` / `digits(base)` | [implemented] DONE / PARTIAL | `1234.digits == [4,3,2,1]`, `0.digits == [0]`. PARTIAL: a negative receiver or `base < 2` returns an EMPTY array where CRuby raises `Math::DomainError` / `ArgumentError`. Pin `1121`. |
| `bit_length` | [implemented] DONE | Complement-counting for negatives (`(-7).bit_length == 3`). Pin `1121`. |
| `pow(n)` | [implemented] DONE / PARTIAL | Binary exponentiation. PARTIAL for a NEGATIVE exponent: CRuby returns a `Rational`, Ruxen returns its integer truncation (`0`, or `±1` for base `±1`). |
| `pow(n, m)` | [implemented] DONE / PARTIAL | Modular exponentiation, Ruby's sign convention (`(-2).pow(3,5) == 2`). PARTIAL: exact only while `m*m` fits in i64, and a negative exponent (modular inverse) returns 0. |
| `Integer.sqrt` | [implemented] DONE | Spelled `Int.sqrt(n)` (Ruxen's type name). Floors; a negative argument returns 0 where CRuby raises `Math::DomainError`. Pin `1121`. |
| `<< >> & \| ^ ~` | [implemented] DONE | bitwise operators; `~` verified by pin `1121`. |
| `even?` / `odd?` / `zero?` | [implemented] DONE | Sign-correct for negative receivers. Pin `1110`. |
| `positive?` / `negative?` | [implemented] DONE | Pin `1110`. |
| `nonzero?` | [planned](../../TODO.md#t-parity-core) MISSING | Needs an `Int?` RETURN from a scalar method-home, which types as the payload at the call site (`no field present? on type Int`), so it cannot be consumed. A `Bool` version would be a different method (`!zero?`), not Ruby's. Blocked on the Option-return bug. |
| `integer?` | [implemented] DONE | always `true`. |
| `chr` / `ord` | [implemented] DONE | `chr` returns a real `Char` (CRuby returns a 1-char String). |
| `coerce` | [planned](../../TODO.md#t-parity-core) MISSING | No numeric coercion protocol in the language: nothing dispatches on the `[other, self]` pair, so the method would have no caller. Revisit with a `Numeric` surface. |
| `<=>` | [implemented] DONE | Primitive operator returns -1/0/1 (case `959`). |
| `clamp` / `between?` | [implemented] DONE | Pin `1110`. `clamp` with `low > high` returns `low` where CRuby raises `ArgumentError`. |
| `[]` (bit ref) | [implemented] PARTIAL | Ships as `bit(i)` with Ruby's infinite sign extension (`(-2).bit(100) == 1`). The `[]` SPELLING does not route: an index expression on a scalar head never reaches an `.rx` `def []` (it types as `<error>`). Pin `1121`. |
| `size` / `numerator` / `denominator` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `hash` | [planned](../../TODO.md#t-parity-core) MISSING | |

---

## Float  (`library/std/scalar/src/lib.rx`, `class Float`)

Declares `to_string` / `to_s` / `to_i` (+ `include Add`). Everything else MISSING.

| Ruby method | Status | Note |
|---|---|---|
| `+ - * / %` | [implemented] DONE | operators. |
| `**` | [implemented] DONE | `ruxen_float_pow` (pin 952, see "Operator reality"). The older "lowered to `*`" note predates the fix. |
| `to_s` | [implemented] DONE; a whole Float prints without `.0` (`3.0` prints `3`) [known issue](../../TODO.md#t-float-to-s) | `ruxen_float_to_string`. |
| `to_i` / `to_int` | [implemented] DONE / PARTIAL | `to_i` = `ruxen_float_to_i` (truncation). |
| `to_f` | [planned](../../TODO.md#t-parity-core) MISSING | identity; not declared. |
| `to_r` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `abs` / `magnitude` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `ceil` / `floor` / `round` / `truncate` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `nan?` / `infinite?` / `finite?` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `divmod` / `modulo` / `quo` / `fdiv` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `zero?` / `positive?` / `negative?` / `nonzero?` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `<=>` | [implemented] DONE | primitive Float `<=>` (pin 959, see "Operator reality"). The older "not lexed" note predates the fix. |
| `clamp` / `between?` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `coerce` / `step` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `hash` | [planned](../../TODO.md#t-parity-core) MISSING | |

**Numeric / Rational / Complex** as classes: **MISSING** entirely (no `Numeric`
supertype surface, no `Rational`, no `Complex`, no `to_r`/`to_c`).

---

## Char  (`library/std/scalar/src/lib.rx`, `class Char`)

Ruby has **no `Char` class** — a one-character string is a `String` — so this
table maps each Ruxen `Char` method onto the `String` method a Ruby developer
would call on `str[0]`. It matters because every `.rx` `String` body in this
stdlib works char-by-char over `self.chars` (`Array[Char]`).

Landed in batch **B18**, pin `1170`; the expected output was diffed against
CRuby 3.3 using `"a"`-style receivers and the POSIX character classes
(`/[[:digit:]]/` and friends).

Classification is **ASCII-only**: a non-ASCII codepoint answers `false` to
every predicate and is returned unchanged by `upcase` / `downcase`.

**Authoring quirk:** a char literal used directly as a receiver (`?7.digit?`)
mis-parses and silently produces nothing. Bind it first (`let c = ?7`).
Also, `? ` and `?!` do not lex at all; use `32.chr` / `33.chr`.

| Ruby (`String`) method | Ruxen `Char` | Status | Note |
|---|---|---|---|
| `ord` | `ord` | [implemented] DONE | the codepoint. |
| `to_i` | `to_i` | [implemented] DONE | the DIGIT value, matching `"7".to_i == 7`; a non-digit is `0`. Use `ord` for the codepoint. |
| `chr` | `chr` | [implemented] DONE | the receiver itself (round-tripped through the codepoint: a bare `self` body is the FFI-shell type and fails `-> Char`). |
| `upcase` / `downcase` | same | [implemented] DONE | ASCII range only. |
| `/[[:digit:]]/` | `digit?` | [implemented] DONE | |
| `/[[:alpha:]]/` | `alpha?` | [implemented] DONE | |
| `/[[:alnum:]]/` | `alnum?` | [implemented] DONE | |
| `/[[:space:]]/` | `space?` / `whitespace?` | [implemented] DONE | codepoints 32 and 9..13. `whitespace?` is an `alias`. |
| `/[[:punct:]]/` | `punct?` | [implemented] DONE | ASCII runs 33..47, 58..64, 91..96, 123..126. |
| `<=>` | `<=>` | [implemented] PARTIAL | The METHOD spelling works (`a.<=>(b)`), returning -1/0/1 by codepoint. The OPERATOR spelling does NOT: `<=>` keeps its dedicated comparison route, which rejects `Char` operands with E0707. Needs the comparison route to consult `.rx` methods. |
| `succ` / `next` | `succ` / `next` | [implemented] PARTIAL | The next CODEPOINT (`?z.succ == ?{`). CRuby's `String#succ` CARRIES (`"z".succ == "aa"`), which a single `Char` cannot represent. |
| `to_s` | `to_s` | [implemented] DONE | `ruxen_char_to_string` (pre-existing). |
| `casecmp` / `casecmp?` / `between?` | — | [planned](../../TODO.md#t-parity-core) MISSING | |

---

## Array  (`library/std/array/src/lib.rx`)

Strongest core class. Verified working: builders, `size`/`count`/`capacity`/
`empty?`, `get`/`get_mut`/`first`/`last`/`include?`, `push`/`pop`/`insert`/
`remove`/`swap`/`clear`/`truncate`/`extend`/`dedup`, `reverse`/`sort`/`clone`/
`to_a`/`to_set`/`take`/`drop`/`chain`/`sum`, `[]`/`[]=`/`[range]`, and the
`Enumerable` combinators + `sort_by`/`zip`.

| Ruby method | Status | Ruxen symbol / gap | Note |
|---|---|---|---|
| `push` / `append` | [implemented] DONE | `ruxen_vec_push` | `<<` operator is broken; use `push`. |
| `pop` | [implemented] DONE | `ruxen_vec_pop` → `T?` | |
| `shift` / `unshift` / `prepend` | [planned](../../TODO.md#t-parity-core) MISSING | — | no front ops. |
| `<<` | [planned](../../TODO.md#t-parity-core) MISSING (BUG) | use `push` | no-op operator. |
| `+` | [implemented] DONE (`def +`→`chain`, pin 984); `concat` still MISSING. |
| `-` | [planned](../../TODO.md#t-parity-core) MISSING (BUG) | — | crashes. |
| `*` (repeat/join) | [planned](../../TODO.md#t-parity-core) MISSING (BUG) | — | garbage. |
| `&` / `\|` (set ops) | [planned](../../TODO.md#t-parity-core) MISSING (BUG) | use `.to_set` ops | garbage. |
| `[]` (index) | [implemented] DONE | subscript | `a[i]`, negative? untested. |
| `[]=` | [implemented] DONE | subscript assign | `a[i]=x`. |
| `[range]` / `slice` | [implemented] PARTIAL | `a[0..1]` works | `slice(i,n)` method MISSING; range-subscript works. |
| `at` | [planned](../../TODO.md#t-parity-core) MISSING | use `get(i)` / `a[i]` | `get` → `Option`. |
| `values_at` / `dig` / `assoc` / `rassoc` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `first` / `last` | [implemented] DONE | `ruxen_vec_first`/`_last` → `T?` | no `first(n)`/`last(n)`. |
| `first(n)` / `last(n)` | [planned](../../TODO.md#t-parity-core) MISSING | use `take(n)` | arity overloading does not reach a builtin head: with both `first` and `first(n)` defined, `a.first(2)` binds the ZERO-arg body and dies in the Cranelift verifier (`got 2, expected 1`). `take(n)` is what Ruby's `first(n)` returns. |
| `index` / `find_index` | [implemented] PARTIAL | `index {block}` / `find_index {block}` (Enumerable, pin 1061) | predicate form only; the value-arg `index(x)` needs `T` equality (B20). |
| `rindex` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `include?` / `member?` | [implemented] PARTIAL | `include?` DONE; `member?` MISSING | |
| `each` | [implemented] DONE | `.rx` body (literal blocks MIR-inlined) | iteration primitive. |
| `each_with_index` | [implemented] DONE | Enumerable body | |
| `each_index` / `each_with_object` | [implemented] PARTIAL | `each_with_object(memo)` `.rx` Enumerable (pin 1062) | `each_index` MISSING. `each_with_object` threads the memo THROUGH the block (`{ \|x, memo\| ... }` must return it) because Ruxen closures take arguments by value; the Ruby argument ORDER is preserved. |
| `reverse_each` / `cycle` | [implemented] DONE | `.rx` Enumerable defaults (pin 1062) | `cycle(n)` counted form only: Ruby's arg-less infinite form and its enumerator form are not expressible. |
| `map` / `collect` | [implemented] PARTIAL | `map` DONE (Enumerable); `collect` MISSING | the name `collect` is claimed by the iterator `collect[Target]` interception (`typeck/infer/expr.rs`, `mir/lower/expr/method_call.rs`), which fires for ANY receiver before method resolution. Ruby's `collect` needs that interception narrowed first. |
| `flat_map` / `collect_concat` | [implemented] DONE | `.rx` Enumerable defaults (pin 1061) | one level deep, as in Ruby. Required dropping `flat_map` from the unimplemented-Vec blocklist in `codegen/lang_intrinsics.rs`. |
| `select` / `filter` | [implemented] PARTIAL | `select` DONE; `filter` MISSING | no `filter` alias. |
| `reject` | [implemented] DONE | Enumerable | |
| `filter_map` | [implemented] DONE | `.rx` Enumerable default (pin 1061) | the block returns `U?`; `nil` is the skip signal, which is the typed form of Ruby's truthiness test. |
| `find` / `detect` | [implemented] PARTIAL | `find` DONE; `detect` MISSING | |
| `find_all` | [implemented] DONE | `.rx` Enumerable delegating to `select` (pin 985). |
| `find_index` | [implemented] PARTIAL | `.rx` Enumerable delegating to `index` (pin 1061) | predicate form only; the value form needs `T` equality (B20). |
| `all?` / `any?` | [implemented] DONE | Enumerable | |
| `none?` / `one?` | [implemented] DONE | `.rx` Enumerable defaults (pin 985). |
| `count` | [implemented] PARTIAL | `ruxen_vec_count` (no-arg); `.rx` Enumerable `count` for other heads (pin 1060) | `count {block}` / `count(obj)` still MISSING. |
| `reduce` / `inject` | [implemented] PARTIAL | `reduce(seed){}` DONE; `inject` MISSING | no symbol form `reduce(:+)`, no no-seed form. |
| `sum` | [implemented] PARTIAL | `ruxen_vec_sum` (`where T: Add`) | no `sum(init)` / `sum{block}`. |
| `min` / `max` | [implemented] DONE | `.rx` body, per-`T` monomorphized (pin 991) | Ruby semantics: `nil` on empty. Orders by the element type's own `<` / `>` (numeric for Int, lexicographic for String). |
| `minmax` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `min_by` / `max_by` / `minmax_by` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `sort` | [implemented] DONE | `ruxen_vec_sort` | natural order. |
| `sort!` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `sort_by` | [implemented] PARTIAL | `.rx` body | in-place comparator-closure `sort_by(cmp)`, signature differs from Ruby's key-extractor `sort_by{}` (returns nil, mutates). |
| `uniq` | [implemented] DONE | `.rx` body, per-`T` monomorphized (pin 991) | keeps the FIRST occurrence of each distinct element, in order, by the element type's own `==` (content equality for `String`). No block form (`uniq {}`). |
| `uniq!` | [planned](../../TODO.md#t-parity-core) MISSING | `dedup` (adjacent only) | `dedup` ≠ Ruby `uniq` (only removes consecutive dups). |
| `flatten` / `flatten!` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `compact` / `compact!` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `zip` | [implemented] DONE | `.rx` body → `Array[(T,U)]` | |
| `take` / `drop` | [implemented] DONE | `ruxen_vec_take`/`_skip` | |
| `take_while` / `drop_while` | [implemented] DONE | `.rx` Enumerable defaults (pin 1060) | latch semantics match Ruby: `drop_while` keeps the element that first failed the predicate and everything after it. |
| `partition` | [implemented] DONE | Enumerable → `(Array,Array)` | |
| `group_by` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `chunk` / `chunk_while` / `slice_when` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `each_slice` / `each_cons` | [implemented] DONE | `.rx` Enumerable defaults (pin 1062, edges pin 1063) | short trailing run still yielded; a window wider than the collection yields nothing; `n <= 0` yields nothing instead of looping. |
| `tally` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `rotate` / `rotate!` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `sample` / `shuffle` / `shuffle!` | [planned](../../TODO.md#t-parity-core) MISSING | — | (no PRNG surface, see Kernel.) |
| `combination` / `permutation` / `product` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `pack` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `join` | [implemented] DONE | `.rx` body, per-`T` monomorphized (pin 991) | `[].join == ""`, separator defaults to `", "`. Formats every element type through its own display (`ruxen_vec_join` retired). Paren-less `a.join` still needs `a.join()`. Element type `Float` prints truncated — a pre-existing float-word ABI bug, not a `join` bug. |
| `fill` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `delete` / `delete_at` / `delete_if` / `keep_if` / `reject!` / `select!` | [implemented] PARTIAL | `select!` has a resolver arm | most delete/keep ops MISSING; `select!` present via `collections.rs`. |
| `insert(i,x)` | [implemented] DONE | `ruxen_vec_insert` | single-element. |
| `remove(i)` / `delete_at` | [implemented] PARTIAL | `remove(i)` DONE (Ruxen name) | returns element by value. |
| `clear` / `truncate` | [implemented] DONE | `ruxen_vec_clear`/`_truncate` | |
| `swap(i,j)` | [implemented] DONE (Ruxen-native) | `ruxen_vec_swap` | |
| `extend(other)` | [implemented] DONE (Ruxen-native) | `ruxen_vec_extend` | in-place concat. |
| `chain(other)` | [implemented] DONE (Ruxen-native) | `ruxen_vec_chain` | returns new array (Ruby `+`). |
| `dedup` | [implemented] DONE (Ruxen-native) | `ruxen_vec_dedup` | adjacent-dup removal. |
| `size` / `length` | [implemented] PARTIAL | `size` DONE; `length` MISSING | |
| `capacity` | [implemented] DONE (Ruxen-native) | `ruxen_vec_capacity` | |
| `to_a` | [implemented] DONE | `alias to_a clone` | shallow copy. |
| `to_h` | [implemented] PARTIAL | `ruxen_hash_from_iter` | from `Array[(K,V)]`; resolver-arm typed. |
| `to_set` | [implemented] DONE | `ruxen_set_from_iter` | |
| `dup` / `clone` | [implemented] PARTIAL | `clone` DONE; `dup` MISSING | shallow (heap elems aliased). |
| `freeze` / `frozen?` / `hash` / `eql?` / `<=>` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `bsearch` / `bsearch_index` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `lazy` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `new` / `with_capacity` | [implemented] DONE | builders | |

---

## Hash  (`library/std/map/src/lib.rx`, surface class `Hash[K,V]`)

Verified working: builders, `size`/`empty?`, `get`/`key?`, `keys`/`values`/
`to_a`, `insert`/`remove`/`clear`, `[]`/`[]=`, and the `Enumerable[(K,V)]`
combinators.

| Ruby method | Status | Ruxen symbol / gap | Note |
|---|---|---|---|
| `[]` (fetch) | [implemented] DONE | subscript | `h[k]`. |
| `[]=` / `store` | [implemented] PARTIAL | `[]=` DONE; `store` MISSING | also `insert(k,v)` (Ruxen name) DONE. |
| `fetch` | [planned](../../TODO.md#t-parity-core) MISSING | use `get` / `h[k]` | `get` → `&V?`; no default/block `fetch`. |
| `dig` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `get(k)` | [implemented] DONE (Ruxen name) | `ruxen_hash_get` → `&V?` | |
| `key?` / `has_key?` / `include?` / `member?` | [implemented] DONE | `key?` + `alias has_key?`/`include?`/`member?` (pins 986, 990). | one `Hash_key?` body serves all four; the delegating bodies went away with Q58. |
| `value?` / `has_value?` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `keys` / `values` | [implemented] DONE | `ruxen_hash_keys`/`_values` | |
| `values_at` / `fetch_values` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `each` / `each_pair` | [implemented] DONE | `each` (yields `(K,V)`) + `alias each_pair each` (pin 990). |
| `each_key` / `each_value` | [implemented] DONE | `.rx` over `each` + tuple access (pin 986). |
| `map` / `select` / `filter` / `reject` | [implemented] PARTIAL | `map`/`select`/`reject` DONE (Enumerable) | closure gets a `(K,V)` tuple; `filter` alias MISSING. |
| `filter_map` / `flat_map` | [implemented] DONE | `.rx` Enumerable defaults (pin 1064) | the block sees the `(K, V)` pair. |
| `find` / `detect` | [implemented] PARTIAL | `find` DONE; `detect` MISSING | |
| `all?` / `any?` / `none?` / `one?` | [implemented] DONE | all four `.rx` Enumerable defaults (pin 985 for none?/one?). |
| `reduce` / `inject` | [implemented] PARTIAL | `reduce(seed){}` DONE | |
| `partition` | [implemented] DONE | Enumerable | |
| `each_with_index` | [implemented] DONE | Enumerable | |
| `each_with_object` | [implemented] DONE | `.rx` Enumerable default (pin 1064) | memo is threaded through the block, which must return it. |
| `count` | [implemented] DONE | `.rx` Enumerable default (pin 1064) | O(n) walk over `each`, no `Hash` length primitive behind it. |
| `sum` / `min_by` / `max_by` / `sort_by` / `group_by` / `tally` | [planned](../../TODO.md#t-parity-core) MISSING | — | Enumerable subset not covered. |
| `merge` / `merge!` / `update` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `transform_keys` / `transform_values` (+ `!`) | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `invert` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `to_a` | [implemented] DONE | `ruxen_hash_entries` → `Array[(K,V)]` | |
| `to_h` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `slice` / `except` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `delete` / `delete_if` / `reject!` / `select!` / `keep_if` | [implemented] PARTIAL | `remove(k)` DONE (Ruxen name) → `V?` | Ruby `delete`+block/predicate MISSING. |
| `default` / `default=` / `default_proc` | [planned](../../TODO.md#t-parity-core) MISSING | — | no default-value protocol. |
| `key(v)` / `assoc` / `rassoc` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `length` / `size` | [implemented] PARTIAL | `size` DONE; `length` MISSING | |
| `empty?` | [implemented] DONE | `ruxen_hash_is_empty` | |
| `clear` | [implemented] DONE | `ruxen_hash_clear` | |
| `replace` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `compare_by_identity` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `dup` / `clone` / `freeze` / `frozen?` / `hash` / `eql?` / `==` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `flatten` / `first` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `new` / `with_capacity` | [implemented] DONE | builders | no default-value ctor. |

---

## Range

`for x in a..b` iteration works (verified sum-loop = 6). As a first-class object
the method surface is nearly empty and partly broken.

| Ruby method | Status | Note |
|---|---|---|
| `a..b` / `a...b` literals | [implemented] DONE (COMPILE-TIME) | inclusive + exclusive; usable in `for`. |
| `each` | [implemented] PARTIAL: iteration in `for` works; `(0..3).each` fails to compile [known issue](../../TODO.md#t-range-methods) | resolves; `for` iteration works. |
| `map` | [implemented] PARTIAL (BUG); `Range` methods fail to compile today [known issue](../../TODO.md#t-range-methods) | resolves but returns empty at runtime. |
| `size` | [implemented] PARTIAL; `Range` methods fail to compile today [known issue](../../TODO.md#t-range-methods) | resolves (correctness unverified). |
| `to_a` / `to_set` / `entries` | [planned](../../TODO.md#t-range-methods) MISSING | |
| `include?` / `member?` / `cover?` / `===` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `step` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `min` / `max` / `minmax` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `sum` / `count` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `first` / `last` / `begin` / `end` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `select` / `reject` / `reduce` / `find` | [planned](../../TODO.md#t-parity-core) MISSING | (Enumerable not mixed into Range.) |
| `exclude_end?` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `lazy` | [planned](../../TODO.md#t-parity-core) MISSING | |

Range is currently an iteration construct, **not** an Enumerable object.

---

## Set  (`library/std/set/src/lib.rx`)

Verified working: builders, `size`/`empty?`/`include?`, `to_a`, `union`/
`intersection`/`difference`, `insert`/`remove`/`clear`, Enumerable combinators.

| Ruby method | Status | Ruxen symbol / gap | Note |
|---|---|---|---|
| `add` / `<<` | [planned](../../TODO.md#t-parity-core) MISSING | use `insert(x)` | Ruby names absent. |
| `add?` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `insert(x)` | [implemented] DONE (Ruxen name) | `ruxen_set_insert` | |
| `delete` | [implemented] PARTIAL | `remove(x)` → `Bool` | Ruxen name. |
| `include?` / `member?` / `===` | [implemented] PARTIAL | `include?` DONE | `member?` alias MISSING. |
| `size` / `length` / `count` | [implemented] PARTIAL | `size` DONE; `count` `.rx` Enumerable (pin 1064) | `length` MISSING. `count` is an O(n) walk over `each`, not the `size` primitive. |
| `empty?` | [implemented] DONE | `ruxen_set_is_empty` | |
| `to_a` | [implemented] DONE | `ruxen_set_iter` → `Array[&T]` | |
| `union` / `\|` / `+` | [implemented] PARTIAL | `union(o)` DONE; operators broken | `s1 \| s2` ⇒ 0. |
| `intersection` / `&` | [implemented] PARTIAL | `intersection(o)` DONE; `&` broken | |
| `difference` / `-` | [implemented] PARTIAL | `difference(o)` DONE; `-` broken | |
| `^` (symmetric diff) | [planned](../../TODO.md#t-parity-core) MISSING (BUG) | — | operator garbage; no method. |
| `subset?` / `superset?` / `proper_*` / `<=` / `>=` / `<` / `>` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `disjoint?` / `intersect?` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `merge` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `map` / `select` / `reject` / `find` / `reduce` / `all?` / `any?` / `partition` / `each` / `each_with_index` / `index` | [implemented] DONE | Enumerable | |
| `first` / `entries` / `take` / `drop` / `take_while` / `drop_while` / `flat_map` / `collect_concat` / `filter_map` / `find_index` / `each_slice` / `each_cons` / `each_with_object` / `cycle` / `reverse_each` | [implemented] DONE | `.rx` Enumerable defaults (pin 1064) | `Set` iterates in hash-bucket order, NOT insertion order, so the order-sensitive members of this group differ from Ruby's insertion-ordered `Set`. |
| `clear` | [implemented] DONE | `ruxen_set_clear` | |
| `to_set` / `dup` / `clone` / `freeze` / `==` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `new` / `with_capacity` | [implemented] DONE | builders | |

---

## Enumerable (the mixin)  (`library/std/core/src/lib.rx`)

`mixin Enumerable[T]` is real and reused across Array / Hash / Set via a single
required `each`. It covers a **subset** of Ruby's ~60 Enumerable methods.

| Ruby Enumerable method | Status | Note |
|---|---|---|
| `each` (required) | [implemented] DONE | the iteration primitive. |
| `map` / `collect` | [implemented] PARTIAL | `map` DONE; no `collect` alias. |
| `select` / `filter` / `find_all` | [implemented] DONE | `select`+`filter`+`find_all` all `.rx` Enumerable. |
| `reject` | [implemented] DONE | |
| `find` / `detect` | [implemented] PARTIAL | `find` DONE; no `detect`. |
| `find_index` | [implemented] PARTIAL | predicate form delegates to `index` (pin 1061); the value form needs `T` equality (B20). |
| `index` | [implemented] DONE | predicate form. |
| `all?` / `any?` | [implemented] DONE | closure form only (no arg/pattern form). |
| `none?` / `one?` | [implemented] DONE | `.rx` defaults (pin 985). |
| `partition` | [implemented] DONE | |
| `each_with_index` | [implemented] DONE | |
| `reduce` / `inject` | [implemented] PARTIAL | `reduce(seed){}` DONE; no `inject`, no symbol/no-seed forms. |
| `flat_map` / `collect_concat` | [implemented] DONE | `.rx` defaults (pin 1061); one level deep. |
| `filter_map` | [implemented] DONE | `.rx` default (pin 1061); block returns `U?`, `nil` skips. |
| `min` / `max` / `minmax` / `min_by` / `max_by` / `minmax_by` | [planned](../../TODO.md#t-parity-core) MISSING | |
| `sort` / `sort_by` | [planned](../../TODO.md#t-parity-core) MISSING (Enumerable) | (Array has its own.) |
| `sum` / `count` / `tally` | [implemented] PARTIAL | `count` (no-arg) is an `.rx` default (pin 1060). `sum` / `tally` need `T` ordering / equality (B20). |
| `group_by` / `chunk` / `chunk_while` / `slice_when` | [planned](../../TODO.md#t-parity-core) MISSING | `group_by` / `tally` need `T` equality to key the result hash, so they are B20, not B07. |
| `each_slice` / `each_cons` / `each_with_object` | [implemented] DONE | `.rx` defaults (pin 1062, edges pin 1063). `each_with_object` threads the memo through the block. |
| `take` / `drop` / `take_while` / `drop_while` | [implemented] DONE | all four `.rx` defaults (pin 1060), so `Set` / `Hash` get them too. |
| `first` / `first(n)` | [implemented] PARTIAL | `first` DONE (pin 1060). `first(n)` MISSING: arity overloading does not reach a builtin head (verifier: `got 2, expected 1`); `take(n)` carries the behavior. |
| `zip` | [planned](../../TODO.md#t-parity-core) MISSING (Enumerable) | (Array has its own.) |
| `to_a` / `entries` / `to_h` / `to_set` | [implemented] PARTIAL | `entries` DONE (pin 1060). `to_a` stays per-head (`Array` / `Set` / `Hash` each home their own with a head-specific element type); `to_h` / `to_set` MISSING. |
| `include?` / `member?` | [planned](../../TODO.md#t-parity-core) MISSING (Enumerable) | |
| `reverse_each` / `cycle` | [implemented] DONE | `.rx` defaults (pin 1062); `cycle(n)` counted form only. |
| `uniq` / `flatten` / `lazy` / `chunk` / `grep` / `grep_v` | [planned](../../TODO.md#t-parity-core) MISSING | `uniq` needs `T` equality (B20); `lazy` needs an enumerator protocol. |

Note: all combinators take a `do |x| … end` block (Ruby-style). `{ }` blocks
are *stored* closures, `do…end` are immediately-invoked (see workspace ledger).

---

## Comparable (the mixin)  (`library/std/core/src/lib.rx`)

| Ruby feature | Status | Note |
|---|---|---|
| `mixin Comparable` | [implemented] DONE (ordering) | Requires `<=>`; shared `.rx` defaults provide `==`, `<`, `<=`, `>`, `>=`, and `between?`. |
| `<` `<=` `==` `>` `>=` | [implemented] DONE | Dispatches through a hand-written `<=>` when `Comparable` is included (case `965`). |
| `<=>` | [implemented] DONE | Primitive and user-defined forms are available (cases `959/965`). |
| `between?(min,max)` | [implemented] DONE | Shared Comparable default (case `965`). |
| `clamp(min,max)` / `clamp(range)` | [planned](../../TODO.md#t-parity-core) MISSING | Needs an ownership-safe return contract for `self` vs. either bound. |

`include Comparable` plus `def <=>(other: &Self) -> Int` is the hand-written
ordering protocol. `clamp` is the remaining core method gap.

---

## Kernel / Object

| Ruby method | Status | Ruxen form / gap | Note |
|---|---|---|---|
| `puts` | [implemented] DONE | builtin | String/interp only. |
| `print` | [implemented] DONE | builtin | |
| `p` | [implemented] PARTIAL; since this audit `p` takes any value that includes `Inspect` (ruby-naming spec, section 12.1) | `.rx` free fn, one overload per type (pin 1180) | prints the INSPECT form (a `String` gets quotes) and RETURNS its argument, as in Ruby. Defined for `Int` / `Bool` / `String` only: Ruxen has no universal `to_s`, so `p` cannot be universal the way Ruby's is. No escaping of embedded quotes or newlines. |
| `pp` | [implemented] PARTIAL | `.rx` free fn (pin 1180) | identical output to `p`; the values reachable here have no nested structure to lay out. Same per-type limit as `p`. |
| `print` to stderr / `warn` | [implemented] DONE | `warn(msg)` `.rx` free fn over `eputs` (pin 1181) | writes to stderr with a newline, leaving stdout untouched. Ruby's `uplevel:` / `category:` keywords need kwargs and are not modelled. |
| `puts`-family extras | [implemented] DONE | `println`, `eputs`, `eprintln` | Ruxen-native. |
| `gets` / `readline` | [planned](../../TODO.md#t-parity-core) MISSING | — | `undefined variable gets`. |
| `format` / `sprintf` | [planned](../../TODO.md#t-d9) MISSING | use `"#{}"` interpolation | `undefined function`. |
| `printf` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `rand` / `srand` | [implemented] PARTIAL | `rand(n)` `.rx` free fn over `Rand.random_u64` (pin 1181) | `rand(n)` returns `[0, n)`; the signed 64-bit draw is folded non-negative (twice, so `Int::MIN` cannot leak through) before the modulo. Ruby's arg-less `rand` returns a `Float` in `[0.0, 1.0)` and needs the `Float` ABI (B01), so `rand(0)` yields `0` here. `srand` MISSING and NOT shippable: `std/rand` exposes only OS entropy, with no seedable generator to make reproducible. |
| `loop do…end` | [planned](../../TODO.md#t-loop-do) COMPILE-TIME; today the `loop ... end` keyword form, which is [deprecated] | keyword | language construct (see `language.md`); runtime behavior of the observed probe was inconclusive. |
| `raise` / `fail` | [planned](../../TODO.md#t-exceptions) MISSING | use `panic!(msg)` / `Result` + `?` | `raise` undefined; error model (exceptions vs `Result`) is a language-design call, see `language.md`, not a metaobject gap. |
| `panic!(msg)` | [deprecated] DONE (Ruxen-native), replace with `raise "msg"` [planned](../../TODO.md#t-exceptions) or `abort "msg"` [implemented] | MIR intrinsic | aborts. |
| `exit` / `exit!` | [implemented] PARTIAL | `exit(code)` DONE, a pre-existing bare FFI decl in `process/src/lib.rx` (pin 1183) | `exit!` MISSING and deliberately not shipped: a free function whose name ends in `!` compiles clean and then its CALL SITE does nothing (verified with `def shout!(x)…end; shout!(5)`), so `exit!` would have been a user-callable silent no-op. Ruxen has no at-exit registry, so `exit` and `exit!` would have had identical behavior anyway. |
| `abort` | [implemented] DONE | `.rx` free fn (pin 1182) | writes `msg` to stderr, then terminates with status 1. Returns `Never`. |
| `sleep` | [implemented] PARTIAL | `sleep(secs)` `.rx` free fn over `Thread.sleep` (pin 1181) | INTEGER seconds only. Ruby's fractional form needs a `Float` argument, which does not survive the current ABI (B01) and would silently sleep zero. Sub-second waits use `Thread.sleep(Duration.from_millis(n))`. |
| `Integer()` / `Float()` | [planned](../../TODO.md#t-parity-core) MISSING | use `"42".to_i` / `"1.5".to_f` | NOT shippable as written: a capitalized identifier lexes as a `TypeIdentifier`, so `def Integer(s)` is a parse error (`expected identifier, found TypeIdentifier("Integer")`). `Float()` additionally waits on the `Float` ABI (B01). |
| `String()` | [planned](../../TODO.md#t-parity-core) MISSING | use `.to_s` | same capitalized-name parse block as `Integer()`, plus it needs a universal `to_s`. |
| `Array()` / `Hash()` | [planned](../../TODO.md#t-parity-core) MISSING | — | same capitalized-name parse block as `Integer()`. |
| `inspect` | [planned](../../TODO.md#t-d9) MISSING | `p(x)` / `"#{x}"` via `include Debug` | no `.inspect` METHOD: a `mixin Kernel` default would have to call `self.to_s`, which is not universal, and `mixin` defaults are emitted for every including head, so it would break every type without one. The inspect FORM is reachable through `p` / `pp` for the types those cover. |
| `to_s` | [implemented] DONE | per-type + `include Debug`/`Displayable` | |
| `class` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | see Reflection cluster below; `x.class` also needs a surface spelling (`class` keyword parse-clash today). |
| `is_a?` / `kind_of?` / `instance_of?` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | static type + known `include` set → constant `Bool`. See Reflection cluster. |
| `respond_to?` (literal method) | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | fixed method set → constant-fold. See Reflection cluster. |
| `send` / `public_send` / `__send__` (literal name) | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | constant name → static dispatch. See Reflection cluster. |
| `define_method` / `method` / `methods` / `instance_variable_*` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | — | mixin codegen + fn references. See Reflection cluster. |
| `tap` | [implemented] DONE | `.rx` `mixin Kernel` (universal include), pin 973/983. |
| `then` / `yield_self` | [implemented] DONE | `.rx` `mixin Kernel` (universal include); `yield_self` a real method, pin 973. |
| `freeze` / `frozen?` | [planned](../../TODO.md#t-parity-core) MISSING | — | no value-freeze protocol. |
| `dup` / `clone` | [implemented] PARTIAL | `.clone` on String/Array/collections | universal `dup` MISSING; `clone` for user types via `include Clone`. |
| `object_id` / `__id__` | [planned](../../TODO.md#t-parity-core) MISSING | — | runtime identity; a pointer-identity intrinsic could supply it (not a metaobject need). |
| `==` / `eql?` / `equal?` | [implemented] PARTIAL | `==` for scalars/String | structural `==` works for a **`struct`** via `include PartialEq`, and is BROKEN for a **`class`**: `C.new(1) == C.new(1)` is `false` with or without an explicit include (**B74**). Pin `133` only covers a struct, which is why it went unnoticed. `eql?`/`equal?` MISSING. |
| `hash` | [implemented] PARTIAL | `Hashable` mixin for user types | no universal `.hash` on values. A **`struct`** carrying `Hashable` + `PartialEq` now works as a `Hash`/`Set` KEY (B73, pin `1722`), routed through the synthesized `hash_code` and `eq`. A **`class`** key still compares by identity, blocked on B74. |
| `nil?` | [planned](../../TODO.md#t-parity-core) MISSING | use `match`/`Option` | `x.nil?` unresolved even on `Option` (drop-Some coercion). |
| `!` / `!=` | [implemented] DONE | operators | |
| `__method__` / `__callee__` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | compiler knows the enclosing method statically | fold to the method-name string literal at the call site. |
| `caller` / `binding` | [retired] OUT-OF-SCOPE | — | needs a runtime call stack / live binding. |
| `catch` / `throw` | [planned](../../TODO.md#t-parity-core) MISSING | — | non-local control flow; lowerable as a language construct (see `language.md`), not a metaobject. |
| `eval` / `instance_eval` / `class_eval` | [retired] OUT-OF-SCOPE | — | arbitrary code from a runtime string, irreducible without an interpreter. |

**Reflection cluster, COMPILE-TIME, not out-of-scope.** Ruxen is a pure
compiler: every type, field, and method is fully known at compile time, so the
*results* Ruby computes at runtime are computable at compile time. These are
tier-2 (compile-time) parity, delivered by a `match` over the fixed type/method
set, mixin codegen, or a macro, not a runtime metaobject:

| Ruby reflective method | Status | Compile-time design that delivers it |
|---|---|---|
| `class` / `.class.name` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | receiver's static type is known; lower `x.class` to its type token / name string. (Today `x.class` also parse-errors on the `class` keyword, needs a surface spelling.) |
| `is_a?` / `kind_of?` / `instance_of?` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | static type + the known mixin/`include` set → a compile-time constant `Bool` (fold to `true`/`false`); for enums, a `match` on the variant. |
| `respond_to?(:m)` (literal `m`) | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | the method set of every type is fixed at compile time → constant-fold to `true`/`false`. |
| `send(:m, …)` / `public_send` (literal `m`) | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | a constant method name folds to direct static dispatch (`x.send("foo")` ≡ `x.foo`). |
| `send(name, …)` (name unknown until runtime) | [retired] OUT-OF-SCOPE | closest substitute: a `match` over the finite set of valid names dispatching statically. |
| `define_method` / `attr_accessor`-style | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | codegen: a mixin or macro synthesizes the method bodies (the tier-2 mechanism the README describes for ORMs/DSLs). |
| `method(:m)` / `instance_method(:m)` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | a first-class function/closure reference to the method (Ruxen already has fn references + `.()`); no `UnboundMethod` object. |
| `methods` / `instance_methods` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | the method set is a compile-time-known list; emit it as a literal array. |
| `instance_variables` / `_get` / `_set` (literal name) | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | fields are statically known; enumerate at compile time; `instance_variable_get("@x")` with a constant folds to field access. Runtime-unknown name → OUT-OF-SCOPE (substitute: `match` over the known field set). |

Genuinely irreducible (tier-3, need an interpreter/live metaobject Ruxen omits):
`eval`/`binding`/`instance_eval`, `ObjectSpace`, `TracePoint`, `caller`, and
`send`/`instance_variable_get` with a name not known until runtime, each with
its closest compile-time substitute noted above.

---

## NilClass / TrueClass / FalseClass

| Ruby feature | Status | Note |
|---|---|---|
| `nil` literal | [implemented] DONE | also the empty `Option` (`nil` in `match` arms). |
| `Int?` optional types | [implemented] DONE (COMPILE-TIME) | `Option` is only the compiler's internal representation; users write `Int?`. |
| `T? ?? T` fallback | [deprecated] DONE, replace with `\|\|` [planned](../../TODO.md#t-truthiness) | case `966`; lazy and right-associative. |
| `true` / `false` literals | [implemented] DONE | |
| `TrueClass#to_s` / `FalseClass#to_s` | [implemented] DONE | `class Bool` → `ruxen_bool_to_string`. |
| `Bool#to_string` | [deprecated] DONE, replace with `to_s` [implemented] | |
| `true & false`, `\| `, `^` | [implemented] DONE | operators, non-short-circuiting exactly as in Ruby (`&&` / `\|\|` are the short-circuiting pair in both). Nothing was added in `.rx`; batch B18 pinned them (pin `1171`) so they cannot regress. |
| `!` (TrueClass/FalseClass) | [implemented] DONE | machine unary operator; pin `1171`. |
| `Bool#to_i` | [implemented] DONE | Batch B18. Ruby has NO `true.to_i`; the 1/0 mapping is what a Ruby developer writes as `flag ? 1 : 0`. Pin `1171`. |
| `NilClass#to_s` / `nil.to_s`→`""` | [planned](../../TODO.md#t-parity-core) MISSING | no method surface on the nil value. |
| `nil?` / `to_a` / `to_h` on nil | [planned](../../TODO.md#t-parity-core) MISSING | use `match`. |
| `TrueClass`/`FalseClass`/`NilClass` as reified classes | [implemented] COMPILE-TIME: nothing to build, the types are static | these types are known statically; no runtime class object is needed, `match` / static dispatch handles the branches Ruby resolves via the class. |

---

## Proc / Method / blocks

| Ruby feature | Status | Note |
|---|---|---|
| Blocks `do \|x\| … end` / `{ \|x\| … }` | [implemented] DONE (COMPILE-TIME) | `do…end` = immediately-invoked, `{ }` = stored closure. |
| `&block` params, `yield`, `block_defined?` | [implemented] DONE (COMPILE-TIME) | Ruby-style block protocol. |
| Closure types `any Fn[Fn(T)->U]`, call via `f.(args)` | [implemented] DONE (COMPILE-TIME); the paren type form `Fn(T) -> U` is [deprecated], replace with `Fn[(T) -> U]` | first-class closures. |
| `Proc.new` / `proc` / `lambda` / `->(){}` | [implemented] DONE | all four build a closure value that owns its captures (ruby-naming spec, section 5). Strict arity and a proc's `return` are [planned](../../TODO.md#t-lambda-call). |
| `Proc#call` / `#()` / `#[]` / `#===` | [implemented] PARTIAL: `f.(…)` and `f.call(…)` work; `f[x]` [planned](../../TODO.md#t-lambda-call); `===` MISSING | `f.(…)` DONE; `[]`/`===` MISSING. |
| `Proc#arity` | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | closure signature is statically known → constant-fold the arity. |
| `Proc#curry` / `#lambda?` | [planned](../../TODO.md#t-lambda-call) MISSING (`lambda?`); `curry` [planned](../../TODO.md#t-parity-core) | curry is a compile-time transform (closure over partial args); `lambda?` folds to a constant. |
| `method(:x)` (literal `x`) | [planned](../../TODO.md#t-parity-compile-time) COMPILE-TIME | a first-class function reference to the named method (Ruxen has fn references + `.()`); no `Method` object. |
| `Method` / `UnboundMethod` reification | [retired] OUT-OF-SCOPE | no runtime metaobject; the fn-reference substitute above covers call-site use. |
| `Symbol#to_proc` (`&:upcase`) | [implemented] DONE | `names.map(&:upcase)` works without a Symbol type (ruby-naming spec, section 5). |

Detailed block/closure semantics belong to `language.md`; summarized here for
core-class completeness.

---

## Regexp / MatchData  (`library/std/regex/src/lib.rx`)

Well-developed, PCRE2-backed. `Regex`, `Match`, `RegexError`, `/pat/flags`
literals, and the `~=` operator.

| Ruby feature | Status | Ruxen symbol | Note |
|---|---|---|---|
| `/pat/flags` literals | [implemented] DONE (COMPILE-TIME) | `ruxen_regex_compile_const` | compiled once at module init; flags `[imsgx]`. |
| `=~` | [implemented] PARTIAL: spelled `~=` today; `=~` [planned](../../TODO.md#t-match-op) | `~=` operator → `is_match` | Ruxen spelling `~=`; returns `Bool` (not index). |
| `Regexp.new` / `.compile` | [implemented] DONE | `Regex.new(pat,flags)` returns a `Result`, which is [deprecated]: Ruby raises `RegexpError`, [planned](../../TODO.md#t-exceptions) | |
| `match` | [implemented] PARTIAL; the Ruby name `match` [planned](../../TODO.md#t-free-keywords) | `Regex#find` → `Match?` | renamed (`match` is a keyword). |
| `match?` | [implemented] DONE | `Regex#is_match` → `Bool` | |
| `scan` (on Regexp) | [implemented] DONE | `Regex#scan` → `Array[Match]` | |
| `String#scan` / `#gsub(/re/)` / `#sub(/re/)` / `#split(/re/)` | [implemented] DONE | `.rx` overloads on `String` delegating to `Regex` (pin 1101) | all four now spell as in Ruby, selected by argument TYPE against their string-argument siblings; `String#match` / `#match?` / `#index(regexp)` too. The `Regex` methods remain the direct route. |
| `Regexp.escape` / `.quote` / `.union` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `Regexp#source` / `#options` / `#names` / `#casefold?` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `MatchData#[]` / `#to_a` / `#captures` | [implemented] PARTIAL | `Match#group(n)` / `#groups` | `group(0)`=whole; `groups`→`Array[String?]`. |
| `MatchData#named_captures` / `#[name]` | [implemented] PARTIAL | `Match#named(name)` / `#named_groups` | `named_groups`→`Hash[String,String]` (omits non-participating). |
| `MatchData#pre_match` / `#post_match` / `#names` / `#values_at` | [planned](../../TODO.md#t-parity-core) MISSING | — | |
| `MatchData#begin` / `#end` / `#offset` | [implemented] PARTIAL | `Match#start_pos` / `#end_pos` | whole-match byte offsets only (not per-group). |
| `MatchData#[0]` (matched) | [implemented] DONE | `Match#matched` → `String` | |
| `RegexError#message` / `#offset` | [implemented] DONE | | compile error + byte offset. |
| global `$~` `$1` `$&` etc. | [planned](../../TODO.md#t-parity-core) MISSING | — | no implicit match globals; the explicit `Match` object (`find` → `Match?`) is the substitute, not a metaobject gap. |

---

## Time / Duration / Instant  (`library/std/time/src/lib.rx`)

Ruxen ships **monotonic + wall-clock primitives**, not Ruby's calendar `Time`.

| Ruby Time feature | Status | Ruxen equivalent | Note |
|---|---|---|---|
| `Time.now` | [implemented] PARTIAL: `Time.unix_ns` today; a `Time` object from `Time.now` [planned](../../TODO.md#t-stdlib-shapes) | `Time.unix_ns` → Int (ns since epoch) | raw nanoseconds, not a Time object. |
| `Instant.now` / `#elapsed` / `#duration_since` / `-` | [deprecated] DONE (Ruxen-native), replace with `Time.now` [planned](../../TODO.md#t-stdlib-shapes) | `class Instant` | monotonic clock (Ruxen-native). |
| `Duration` (from/as secs/millis/micros/nanos, `+`/`-`) | [deprecated] DONE (Ruxen-native); the Ruby time shapes are [planned](../../TODO.md#t-stdlib-shapes) | `class Duration` | Ruxen-native value type. |
| `Time.at` / `Time.new(y,m,d,…)` / `Time.parse` | [planned](../../TODO.md#t-parity-time) MISSING | — | no calendar construction. |
| `#year`/`#month`/`#day`/`#hour`/`#min`/`#sec`/`#usec`/`#nsec` | [planned](../../TODO.md#t-parity-time) MISSING | — | no broken-down time. |
| `#wday`/`#yday`/`#monday?`… | [planned](../../TODO.md#t-parity-time) MISSING | — | |
| `#strftime` / `#iso8601` / `#to_s` | [planned](../../TODO.md#t-parity-time) MISSING | — | no formatting. |
| `#to_i` / `#to_f` / `#to_r` | [implemented] PARTIAL | `Time.unix_ns` | ns int only. |
| `#+`/`#-`(seconds) / comparison | [implemented] PARTIAL | on `Duration`/`Instant` only | not on wall-clock time. |
| `#utc` / `#localtime` / `#zone` / `#getlocal` | [planned](../../TODO.md#t-parity-time) MISSING | — | no timezone model. |
| `Date` / `DateTime` classes | [planned](../../TODO.md#t-parity-time) MISSING | — | (stdlib `date`, see `stdlib.md`). |

---

## Struct

| Ruby feature | Status | Note |
|---|---|---|
| `Struct.new(:a,:b)` factory | [planned](../../TODO.md#t-struct-new) MISSING (COMPILE-TIME substitute) | Ruxen has a native `struct Name … end` declaration instead. |
| `struct`/`class` with fields | [implemented] DONE for `class`; `struct` is [deprecated], replace with `Data.define` [planned](../../TODO.md#t-data-define) or `Struct.new` [planned](../../TODO.md#t-struct-new) | native aggregate types with typed fields + accessors. |
| `#members` / `#to_a` / `#to_h` / `#[]` / `#each` / `#==` | [implemented] PARTIAL | structural `==` via `include PartialEq`; `to_h`/`members`/`[]` MISSING. |
| keyword_init / `Data.define` | [planned](../../TODO.md#t-data-define) MISSING | — | |

---

## Priority backlog (highest-value MISSING, batched)

Ordered by leverage. Each batch is implementable as a unit (a class's method
family, or a cross-cutting compiler fix), each with a `tests/release-e2e/` pin.

### P0, correctness bugs in *existing* surface (fix before adding breadth)
These silently miscompile or crash today; they erode trust in the whole surface.
1. **`**` power operator**, currently lowered to `*` (`2**10`⇒20). Lex a
   distinct token + real pow lowering (Int + Float). *(Compiler.)*
2. ~~**`<=>` spaceship**~~ **DONE** for primitive and user-defined comparison
   plus Comparable defaults (cases `959/965`).
3. ~~**`Array[Int]#join`**~~ **DONE** — `join` is now a `.rx` body specialized
   per concrete element type (pin `991`); `ruxen_vec_join` is retired. What
   remains from this entry is a *different*, deeper bug: **`Array[Float]`
   elements truncate to `Int`** (`[1.5][0]` ⇒ `1`, `each` yields `1`), because
   a `Float` crossing into an erased uniform word is `fcvt`-converted rather
   than bit-reinterpreted (`codegen/cranelift/emit.rs::coerce_value`). Fixing
   it needs a float-in-word ABI decision (bitcast on push AND on read), not a
   `join` fix; it also explains the "Float closure-argument passes as 0"
   entry below.
4. **Collection/String operators that crash or no-op**, `Array +/-/*/&/|/<<`,
   `String */<<`, `String[i]`, `Set |/&/-/^`, `Range#map`. Either implement or
   make them a compile error (never a silent crash). Prefer wiring the operators
   to the working methods (`chain`, `push`, `union`, …).

### P1, Integer / Float methods (whole family; `class Int`/`class Float`)
**Integer half: DONE** (batches B12/B13/B14, pins `1110`/`1111`/`1120`/`1121`/
`1130`/`1131`) — `abs`, `times`, `upto`, `downto`, `step`, `pow`, `gcd`, `lcm`,
`divmod`, `fdiv`, `div`, `modulo`, `remainder`, `ceil`, `floor`, `round`,
`truncate`, `even?`, `odd?`, `zero?`, `positive?`, `negative?`, `pred`, `succ`,
`digits`, `bit_length`, `bit`, `chr`, `ord`, `to_i`, `clamp`, `between?`,
`to_s(base)`, `Int.sqrt`. No C symbol was needed: every one is an `.rx` `def`
on the method-home. Still open on the Integer side: `nonzero?` (needs an
`Option` return that survives the call site), `coerce` (needs a numeric
coercion protocol), the `[]` bit-reference SPELLING, `**` (the P0 bug above),
and `to_r`/`to_c`/`hash`.

**Float half: still MISSING**, and blocked ahead of the methods themselves by
the `class Float` receiver-ABI bug (batch B01): `self` arrives truncated to an
integer inside a `Float` `.rx` body.

### P2, String method family (`string.rx` + `runtime/string.c`)
Batch A (query): `length`(alias), `sub`, `gsub`, `scan`, `index`(alias `find`),
`rindex`, `count`, `[]`/`slice`(+range/2-arg), `start_with?`/`end_with?`(`?`
aliases). Batch B (transform): `upcase`/`downcase`/`capitalize`/`swapcase`
(aliases), `strip`(alias), `chomp`, `chop`, `reverse`, `tr`, `delete`, `squeeze`,
`center`/`ljust`/`rjust`, `delete_prefix`/`delete_suffix`, `prepend`. Batch C
(convert): `to_i`/`to_f` (lenient, alias `parse_*`), `ord`/`chr`, `hex`/`oct`,
`succ`, `format`/`%`.

### P3, Array method family (`array/src/lib.rx`)
Batch A (access/query): `shift`/`unshift`, `min`/`max`/`minmax`, `min_by`/
`max_by`, `uniq`, `flatten`, `compact`, `first(n)`/`last(n)`, `values_at`, `dig`,
`rindex`, `member?`. Batch B (transform): `collect`/`filter`/`detect`/`inject`
(aliases), `flat_map`, `filter_map`, `group_by`, `tally`, `each_slice`/
`each_cons`, `take_while`/`drop_while`, `rotate`, `each_with_object`, `none?`/
`one?`. Batch C (mutate): `<<`(op→push), `delete`/`delete_at`, `fill`,
`sort!`/`reverse!`/`uniq!`, `concat`(op→chain).

### P4, Hash method family (`map/src/lib.rx`)
`fetch`, `dig`, `store`(alias), `has_key?`/`include?`/`member?`(aliases),
`value?`/`has_value?`, `merge`/`merge!`, `transform_keys`/`transform_values`,
`invert`, `to_h`, `slice`/`except`, `each_key`/`each_value`/`each_pair`(aliases),
`values_at`, `key`, `default`/`default=`, `count`, `min_by`/`max_by`/`sort_by`/
`sum`/`group_by`, `each_with_object`, `replace`.

### P5, Range as an Enumerable object
`include Enumerable[T]` on Range + `to_a`/`include?`/`cover?`/`===`/`step`/
`min`/`max`/`sum`/`first`/`last`/`begin`/`end`/`exclude_end?`. Currently only
`for`-iteration works.

### P6, Enumerable breadth (`core/lib.rx` mixin, reused everywhere)
Add to the shared mixin: `collect`/`filter`/`detect`/`find_all`(aliases),
`none?`/`one?`, `min`/`max`/`min_by`/`max_by`, `sum`, `count`, `flat_map`,
`filter_map`, `group_by`, `tally`, `each_with_object`, `take`/`drop`/
`take_while`/`drop_while`, `first`, `to_a`/`to_h`, `sort_by`. One implementation
lands on Array + Hash + Set + Range simultaneously.

### P7, Comparable wiring
DONE for `< <= == > >= between?` over required `<=>` (case `965`). `clamp`
remains staged until its ownership-safe return contract is chosen.

### P8, Kernel conveniences
`p`/`pp` (debug-print, over `include Debug`), `format`/`sprintf` (or bless
priority given interpolation), `rand`/`srand` (expose `std/rand`), `Integer()`/
`Float()`/`String()` conversion fns, `tap`/`then`.

### P9, Compile-time reflection (tier-2 parity, not out-of-scope)
Deliver Ruby's reflective results at compile time, since the pure compiler knows
every type/field/method statically. Batch: `is_a?`/`kind_of?`/`instance_of?` and
`respond_to?(:m)` as constant-folded predicates over the known type + `include`
set; `class`/`.class.name` lowered to the receiver's static type token (needs a
non-keyword surface spelling); `send(:m,…)`/`public_send` with a literal name →
static dispatch; `method(:m)` → a first-class fn reference; `methods`/
`instance_variables` → compile-time-known literal lists; `define_method`-style
generation via mixin/macro. Only a name unknown until runtime falls
back to a `match` over the finite valid-name set.

### Deferred / language-identity decisions (tier 3, tracked not scheduled)
- **Symbol type** (`:sym`, `to_sym`, `&:method`, symbol hash keys). Currently
  bare strings substitute. A real Symbol type is a language-design call.
- **Irreducibly-runtime reflection**, `eval`/`instance_eval`/`binding`,
  `ObjectSpace`, `TracePoint`, `caller`, and `send`/`instance_variable_get` with
  a name not known until compile time. These need the interpreter/live metaobject
  Ruxen deliberately omits; closest compile-time substitute noted per row above.
  (Everything else in Ruby's reflection surface is **P9**, not deferred.)
- **Exceptions** (`raise`/`rescue`/`ensure`/`retry`, exception hierarchy) vs the
  current `panic!` + `Result`/`?` model, see `language.md`.
- **Rational / Complex** numeric towers.
- **Calendar `Time`/`Date`/`DateTime`** (vs the monotonic `Instant`/`Duration`
  primitives), overlaps `stdlib.md` (`date`).

---

### Cross-references
- Operators, blocks, `<=>`/operator-overloading, exceptions, pattern matching →
  [`language.md`](language.md).
- `date`/`time` stdlib, `json`, `securerandom`, etc. → [`stdlib.md`](stdlib.md).
- Source of truth for the present surface: `library/std/*/src/lib.rx` +
  each package's `CLAUDE.md`.
</content>
</invoke>
