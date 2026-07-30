# Ruby core-class parity, the exhaustive audit

Authoritative, no-skip backlog for **core-class method parity** between Ruby
(CRuby / MRI 3.x) and Ruxen. One table per Ruby core class; a row per real Ruby
method. See [`README.md`](README.md) for the tier framework.

Status vocabulary:

| Status | Meaning |
|--------|---------|
| **DONE** | Present in Ruxen, verified to compile **and run correctly**. |
| **PARTIAL** | Declared / compiles but has a real gap: wrong at runtime, receiver-shadowed, element-type-restricted, or a narrower signature than Ruby. |
| **MISSING** | No usable Ruxen equivalent (typeck rejects, or links to a non-existent symbol). |
| **COMPILE-TIME** | Re-expressed as a Ruxen language construct / mixin, not a runtime method (tier 2). |
| **OUT-OF-SCOPE** | Needs a runtime metaobject / interpreter Ruxen deliberately lacks (tier 3). |

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
| Int/Float `+ - * / %`, unary `-` | **DONE** | `2+3`, `7/2`, `7%2`, `-5` all correct. |
| Int/Float `< <= > >= == !=` | **DONE** | all correct. |
| `&& \|\| !`, Bool `& \| ^` | **DONE** | correct. |
| Int `<< >> & \| ^` (bitwise) | **DONE** | `1<<4`=16, `6&3`=2 correct. |
| String `+`, String `==` | **DONE** | `"ab"+"cd"`="abcd". |
| `Array[i]`, `Array[i]=`, `Hash[k]`, `Hash[k]=` | **DONE** | `a[1]`=20, `a[0]=9`, `h["a"]` all correct. `Hash[k]=` was a SILENT NO-OP until B72 (pins `1710`/`1711`): the row claimed DONE while the store was discarded with no diagnostic. Both `Hash` construction spellings (`var h: Hash[K, V] = Hash.new` and `Hash[K, V].new`) are now pinned, `1728`/`1729`. |
| `Array[range]` (e.g. `a[0..1]`) | DONE | compiles+runs. |
| **`**` (power)** | **DONE** | `2**8`⇒256. `StarStar` token + `BinOp::Pow` → `ruxen_int_pow`/`ruxen_float_pow` (pin 952). |
| **`<=>` (spaceship)** | **DONE** | Primitive Int/Float/String pin `959`; user method and Comparable dispatch pin `965`. |
| **String `*`** | **DONE** | `"ab"*3`⇒"ababab". `.rx` `def *`→`repeat`, builtin-head operator route (pin 984). |
| **String `<<`** | **MISSING (BUG)** | no-op (receiver unchanged). Use `.push_str`. (`<<` mutation semantics pending.) |
| **String `[i]` (index)** | **MISSING (BUG)** | `"abc"[1]` ⇒ empty/null Char. No `[]` in `string.rx`. |
| **Array `+`** | **DONE** | `[1,2]+[3,4]`⇒`[1,2,3,4]`. `.rx` `def +`→`chain` (pin 984). **`- * &  \|`** still MISSING (need element-eq set ops). |
| **Array `<<`** | **MISSING (BUG)** | no-op. Use `.push(x)`. |
| **Set `\| & -`** | **DONE** | `.rx` `def \|`/`&`/`-`→`union`/`intersection`/`difference` (pin 984). **`^`** (symmetric diff) MISSING. |

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
| `length` | PARTIAL | `ruxen_string_len` | Ruby `length` aliased to the same C symbol as `size`. Pin `972`. **Counts BYTES, not characters**, so it diverges from CRuby for non-ASCII text (`"héllo".length` is 6, Ruby says 5). `chars.size` / `codepoints.size` are the character counts. Changing it is a cross-cutting call (HTTP framing in `library/std/http` relies on the byte semantics), so it is recorded here rather than changed inside a String batch. |
| `size` | PARTIAL | `ruxen_string_len` | Same byte-vs-character divergence as `length`. |
| `bytesize` | DONE | `.rx` over `bytes` (pin 1071) | byte length, distinct from the character count. |
| `+` (concat) | DONE | operator | `"a"+"b"`. |
| `*` (repeat) | DONE | `def *`→`repeat` (pin 984). |
| `<<` / `concat` | MISSING (BUG) | use `push_str` | `<<` is a no-op. |
| `%` (format) | MISSING | use `"#{}"` interp | no `String#%`. |
| `[]` / `slice` (index) | PARTIAL | `.rx` `slice(start, len)` (pin 1092) | the two-argument `slice` is DONE, returning `Option[String]` with Ruby's exact out-of-range/negative-start edges. The `[]` operator, the one-argument index form and the Range form are still MISSING (`"abc"[1]` yields a null Char). |
| `at` | MISSING | — | |
| `byteslice` | MISSING | — | |
| `sub` / `sub!` | PARTIAL | `.rx` `sub(pattern, replacement)` (pin 1090), regex form pin 1101 | string and `Regex` first-match forms both present. `sub!` MISSING (no in-place variant is expressible; see `strip!`). The block form needs a block-taking overload. |
| `gsub` / `gsub!` | PARTIAL | `.rx` `gsub(pattern, replacement)` (pin 1090), regex form pin 1101 | string and `Regex` replace-all forms both present. Implemented in `.rx` rather than delegating to the FFI `replace`, which leaves an EMPTY pattern alone where Ruby matches it at every position. `gsub!` MISSING (no in-place variant is expressible). The block form needs a block-taking overload. |
| `tr` / `tr_s` | DONE | `.rx` (pin 1082) | tr-style sets with `a-z` ranges, `^` negation and `\\` escapes; a short `to` is padded with its last character and an empty `to` deletes. |
| `replace` (whole-string) | MISSING | — | the NAME is taken: Ruxen's `replace(from, to)` is a replace-all, i.e. its `gsub`. Ruby's one-argument `replace(other)` overwrites the receiver in place, which is not expressible (see `strip!`). Use `gsub` for the two-argument meaning. |
| `split` | PARTIAL | `ruxen_string_split` + `.rx` `split(sep, limit)` (pin 1092) and `split(re)` (pin 1101) | string, limit and `Regex` forms present. NOTE the one-argument `split(sep)` keeps trailing empty fields, so it matches Ruby's `split(sep, -1)`, not Ruby's default; pass an explicit `0` for Ruby's default. No awk-style whitespace-splitting default. |
| `splitn(n, delim)` | DONE | `ruxen_string_splitn` | Ruxen-specific limited split. |
| `strip` | DONE | `ruxen_string_trim` | Ruby `strip` aliased to the same C symbol as `trim`. Pin `975`. |
| `lstrip` / `rstrip` | DONE | `trim_start` / `trim_end` | Ruby spellings aliased to the same C symbols. Pin `975`. |
| `strip!`/`lstrip!`/`rstrip!` | MISSING | — | no in-place variants, and none are expressible: inside `class String` the receiver is the FFI shell (a buffer pointer held BY VALUE), so a `.rx` body cannot rewrite the caller's string, and growing it needs a realloc whose new pointer has nowhere to go. Needs a `&var self` receiver. |
| `chomp` / `chop` | DONE | `.rx` (pin 1080) | `chomp` has both arities (no-arg drops one `\r\n` / `\n` / `\r`; `chomp("")` drops every trailing terminator); `chop` counts a trailing `\r\n` as one character. |
| `upcase` | DONE | `to_upper` | Ruby `upcase` aliased to the same C symbol; `to_upper` also kept. Pin `972`. |
| `downcase` | DONE | `to_lower` | Ruby `downcase` aliased to the same C symbol; `to_lower` also kept. Pin `972`. |
| `capitalize` / `swapcase` | DONE | `.rx` over `chars` (pins 987, 1080) | `swapcase` is ASCII-only, matching `upcase` / `downcase`, which are per-byte C `toupper` / `tolower`. |
| `upcase!`/`downcase!`/`capitalize!`/`swapcase!`/`squeeze!`/`sub!`/`gsub!`/… | MISSING | — | no `!` variant of ANY String method, and none are expressible for the same reason as `strip!` above. Every method here returns a new String. |
| `start_with?` | PARTIAL | `ruxen_string_starts_with` + `.rx` array overload (pins 975, 1072) | single `&String` arg, plus an `&Array[String]` overload standing in for Ruby's splat. No regex form. An untyped `[]` literal cannot pick between the two overloads: use a typed local. |
| `end_with?` | PARTIAL | `ruxen_string_ends_with` + `.rx` array overload (pins 975, 1072) | as `start_with?`. |
| `include?` | DONE | `ruxen_string_contains` | |
| `index` | DONE | `.rx` `index(sub, offset = 0)` (pin 1070) | CHARACTER index, `Option[Int]`, negative offset counts from the end. The pre-existing `find` stays as the BYTE-offset spelling. Regex form: see `match`. |
| `rindex` | DONE | `.rx` `rindex(sub)` / `rindex(sub, offset)` (pin 1070) | two arities rather than a default argument: a NEGATIVE default argument mis-lowers on a builtin head. |
| `count` | PARTIAL | `.rx` `count(set)` (pin 1071) | Ruby tr-style sets including `a-z` ranges, `^` negation and `\\` escapes. Ruby's multi-argument intersection form needs a splat Ruxen does not have. |
| `delete` / `squeeze` | DONE | `.rx` (pin 1082) | tr-style sets, shared with `count` / `tr`. `squeeze` has both arities (no-arg collapses every run). |
| `center` / `ljust` / `rjust` | DONE | `.rx` (pins 988, 989, 1081) | `pad` defaults to a space and is CYCLED one character at a time, so the result is exactly `max(width, length)` characters. A multi-character pad previously overshot the width. |
| `chars` | DONE | `ruxen_string_chars` → `Array[Char]` | |
| `bytes` | DONE | `ruxen_string_bytes` → `Array[UInt8]` | |
| `codepoints` | DONE | `.rx` over `chars` (pin 1071) | |
| `lines` | DONE | `ruxen_string_lines` | |
| `each_char` / `each_line` / `each_byte` | PARTIAL | `.rx` block forms (pin 1073) | the block is required (no Enumerator return) and the return is `nil`, not `self`. `each_line` strips the line terminator, where CRuby keeps it. |
| `each_grapheme_cluster` | MISSING | — | |
| `scan` | PARTIAL | `.rx` `scan(re)` (pin 1101) | returns `Array[String]` of whole matches. Ruby's array-per-match form for a pattern WITH groups is absent: use `Regex#scan` and read `Match#group`. |
| `match` / `match?` / `=~` | PARTIAL | `.rx` `match(re)` / `match?(re)` / `index(re)` (pin 1101) | `match` returns `Option[Match]`, `match?` a Bool. `=~` CANNOT be defined: Ruxen's lexer has no `=~` token (its match operator is `~=`, a Bool-returning dedicated route). Ruby's `=~` answer, the character index of the match, is available under Ruby's other name for it, `index(regexp)`. |
| `to_i` / `to_f` | DONE | `.rx` `to_i(base = 10)` / `to_f` (pin 1100) | Ruby's lenient semantics: leading whitespace and sign, `_` between digits, a `0x` / `0b` / `0o` / `0d` prefix when it agrees with `base`, scanning stops at the first bad character, and 0 when nothing parses. The strict `Result`-returning `parse_int` / `parse_float` remain. CAVEAT: the accumulator is an `Int`, so a value past 64 bits WRAPS where CRuby promotes to a Bignum. |
| `parse_int`/`parse_float`/`parse_uint` | DONE | `ruxen_string_parse_*` | Ruxen-native, `Result`-returning. |
| `to_sym` / `intern` | MISSING | — | blocked on the Symbol type (a language-design decision), not a metaobject need. |
| `to_r` / `to_c` | MISSING | — | no Rational/Complex. |
| `to_s` / `to_str` | DONE | `ruxen_string_to_string`, `.rx` `to_str` (pin 1102) | |
| `ord` / `chr` | DONE | `.rx` (pin 1102) | `ord` on an EMPTY receiver aborts via `panic!`: Ruby raises ArgumentError and Ruxen has no exception surface, so aborting beats inventing a 0. |
| `hex` / `oct` | DONE | `.rx` over `to_i(base)` (pin 1100) | `oct` reads the base from the receiver's own prefix (`0x`→16, `0b`→2, `0d`→10, otherwise 8), as in Ruby. |
| `succ` / `next` | DONE | `.rx` (pin 1083) | full carry semantics: rightmost alphanumeric, carrying leftwards across alphanumerics and skipping non-alphanumerics, seeding `"1"` / `"a"` / `"A"` on overflow. The no-alphanumeric path carries at the CHARACTER level where CRuby carries at the BYTE level; the two agree for every ASCII string. |
| `reverse` | DONE | `.rx` over `chars`+`Array#reverse` (pin 987). |
| `insert` | PARTIAL | `insert(i, codepoint)` / `insert_str(i, s)` | Ruxen splits into codepoint- vs string-insert; arg is an Int codepoint, not a String, for `insert`. |
| `prepend` | PARTIAL | `.rx` (pin 1084) | returns a NEW String; Ruby mutates the receiver and returns self. |
| `delete_prefix` / `delete_suffix` | DONE | `.rx` (pin 1084) | |
| `partition` / `rpartition` | DONE | `.rx` (pin 1091) | three-element `Array[String]`; string separators only. |
| `freeze` / `frozen?` | MISSING | — | no freeze protocol on values. |
| `dup` / `clone` | DONE | `clone`, `.rx` `dup` (pin 1102) | both are an owned copy. Ruby's `clone` also copies frozen state and singleton methods, neither of which Ruxen has. |
| `hash` | MISSING | — | (`Hashable` mixin exists for user types; no `String#hash` surface.) |
| `eql?` / `casecmp` / `casecmp?` | DONE | `.rx` (pin 1072) | `casecmp` returns -1/0/1, ASCII folding only (CRuby folds full Unicode). Ruby's nil-for-a-non-String return has no analogue: the parameter is statically `&String`. `<=>` on String is still absent. |
| `encode`/`force_encoding`/`encoding`/`valid_encoding?`/`b`/`scrub` | MISSING | — | no encoding API (strings are UTF-8 bytes). |
| `unpack` / `unpack1` | MISSING | — | |
| `getbyte` / `setbyte` | MISSING | — | |
| `sum` / `crypt` / `unicode_normalize` | MISSING | — | |
| `upto` | MISSING | — | |
| `format` / `sprintf` (as method) | MISSING | use `"#{}"` interp | |
| `clear` / `truncate(n)` | DONE | `ruxen_string_clear`/`_truncate` | mutating (`var`). |
| `remove(i)` | DONE (Ruxen-native) | `ruxen_string_remove` → Char | removes+returns codepoint at `i`. |
| `new` / `with_capacity` / `from_bytes` | DONE | class methods | builders. |

---

## Symbol

| Ruby feature | Status | Note |
|---|---|---|
| `:sym` literal | MISSING | `:foo` is a **parse error** (`found Colon`); no `SymbolLiteral` token in the lexer. A Symbol type is a language-design decision, not a metaobject gap. |
| Symbol type + `to_s`/`to_sym`/`to_proc`/`length`/`[]`/`upcase`/`<=>`/`succ`/`id2name` | MISSING | No Symbol value type exists. |

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
| `+ - * / %` | DONE | operators. `/` and `%` TRUNCATE (C), where Ruby's floor. Use `div` / `modulo` for Ruby's answers. |
| `**` (pow) | MISSING (BUG) | lowered to `*`. The working spelling is `pow`. |
| `divmod` / `div` / `modulo` | DONE | FLOORED, Ruby's convention: `-7.div(2) == -4`, `-7.modulo(2) == 1`. Pin `1120`. |
| `remainder` | DONE | TRUNCATED, sign of the dividend (`-7.remainder(2) == -1`). Pin `1120`. |
| `fdiv` | DONE | Value matches; a whole Float RENDERS as `2` where CRuby prints `2.0` (pre-existing `ruxen_float_to_string` gap). |
| `gcd` / `lcm` / `gcdlcm` | DONE | Non-negative results; `0.gcd(0) == 0`, `0.lcm(5) == 0`. Pin `1121`. |
| `abs` / `magnitude` | DONE | `magnitude` is an `alias`. |
| `ceil` / `floor` / `round` / `truncate` | DONE / PARTIAL | Precision arg included; `round` is half-away-from-zero like CRuby. PARTIAL only for `ndigits <= -19`, which leaves the i64 range (returns 0). Pin `1111`. |
| `times` | DONE | Pre-existing; pinned by `1130`. |
| `upto` / `downto` / `step` | DONE / PARTIAL | Block-taking `.rx` methods; both `step` arities. PARTIAL: `step(limit, 0)` yields nothing where CRuby raises `ArgumentError`. Pin `1130`. |
| `pred` / `succ` / `next` | DONE | `next` is an `alias` of `succ`. |
| `to_s` | DONE | `ruxen_int_to_string`. |
| `to_s(base)` | DONE / PARTIAL | Arity-1 `.rx` overload, bases 2..36, correct for i64-min. PARTIAL: an out-of-range base falls back to 10 where CRuby raises `ArgumentError`. Pin `1131`. |
| `to_i` / `to_int` | DONE | identity; `to_int` is an `alias`. |
| `to_f` | DONE | `ruxen_int_to_f`. |
| `to_r` / `to_c` | MISSING | no Rational/Complex. |
| `digits` / `digits(base)` | DONE / PARTIAL | `1234.digits == [4,3,2,1]`, `0.digits == [0]`. PARTIAL: a negative receiver or `base < 2` returns an EMPTY array where CRuby raises `Math::DomainError` / `ArgumentError`. Pin `1121`. |
| `bit_length` | DONE | Complement-counting for negatives (`(-7).bit_length == 3`). Pin `1121`. |
| `pow(n)` | DONE / PARTIAL | Binary exponentiation. PARTIAL for a NEGATIVE exponent: CRuby returns a `Rational`, Ruxen returns its integer truncation (`0`, or `±1` for base `±1`). |
| `pow(n, m)` | DONE / PARTIAL | Modular exponentiation, Ruby's sign convention (`(-2).pow(3,5) == 2`). PARTIAL: exact only while `m*m` fits in i64, and a negative exponent (modular inverse) returns 0. |
| `Integer.sqrt` | DONE | Spelled `Int.sqrt(n)` (Ruxen's type name). Floors; a negative argument returns 0 where CRuby raises `Math::DomainError`. Pin `1121`. |
| `<< >> & \| ^ ~` | DONE | bitwise operators; `~` verified by pin `1121`. |
| `even?` / `odd?` / `zero?` | DONE | Sign-correct for negative receivers. Pin `1110`. |
| `positive?` / `negative?` | DONE | Pin `1110`. |
| `nonzero?` | MISSING | Needs an `Option[Int]` RETURN from a scalar method-home, which types as the payload at the call site (`no field present? on type Int`), so it cannot be consumed. A `Bool` version would be a different method (`!zero?`), not Ruby's. Blocked on the Option-return bug. |
| `integer?` | DONE | always `true`. |
| `chr` / `ord` | DONE | `chr` returns a real `Char` (CRuby returns a 1-char String). |
| `coerce` | MISSING | No numeric coercion protocol in the language: nothing dispatches on the `[other, self]` pair, so the method would have no caller. Revisit with a `Numeric` surface. |
| `<=>` | DONE | Primitive operator returns -1/0/1 (case `959`). |
| `clamp` / `between?` | DONE | Pin `1110`. `clamp` with `low > high` returns `low` where CRuby raises `ArgumentError`. |
| `[]` (bit ref) | PARTIAL | Ships as `bit(i)` with Ruby's infinite sign extension (`(-2).bit(100) == 1`). The `[]` SPELLING does not route: an index expression on a scalar head never reaches an `.rx` `def []` (it types as `<error>`). Pin `1121`. |
| `size` / `numerator` / `denominator` | MISSING | |
| `hash` | MISSING | |

---

## Float  (`library/std/scalar/src/lib.rx`, `class Float`)

Declares `to_string` / `to_s` / `to_i` (+ `include Add`). Everything else MISSING.

| Ruby method | Status | Note |
|---|---|---|
| `+ - * / %` | DONE | operators. |
| `**` | MISSING (BUG) | lowered to `*`. |
| `to_s` | DONE | `ruxen_float_to_string`. |
| `to_i` / `to_int` | DONE / PARTIAL | `to_i` = `ruxen_float_to_i` (truncation). |
| `to_f` | MISSING | identity; not declared. |
| `to_r` | MISSING | |
| `abs` / `magnitude` | MISSING | |
| `ceil` / `floor` / `round` / `truncate` | MISSING | |
| `nan?` / `infinite?` / `finite?` | MISSING | |
| `divmod` / `modulo` / `quo` / `fdiv` | MISSING | |
| `zero?` / `positive?` / `negative?` / `nonzero?` | MISSING | |
| `<=>` | MISSING | not lexed. |
| `clamp` / `between?` | MISSING | |
| `coerce` / `step` | MISSING | |
| `hash` | MISSING | |

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
| `ord` | `ord` | DONE | the codepoint. |
| `to_i` | `to_i` | DONE | the DIGIT value, matching `"7".to_i == 7`; a non-digit is `0`. Use `ord` for the codepoint. |
| `chr` | `chr` | DONE | the receiver itself (round-tripped through the codepoint: a bare `self` body is the FFI-shell type and fails `-> Char`). |
| `upcase` / `downcase` | same | DONE | ASCII range only. |
| `/[[:digit:]]/` | `digit?` | DONE | |
| `/[[:alpha:]]/` | `alpha?` | DONE | |
| `/[[:alnum:]]/` | `alnum?` | DONE | |
| `/[[:space:]]/` | `space?` / `whitespace?` | DONE | codepoints 32 and 9..13. `whitespace?` is an `alias`. |
| `/[[:punct:]]/` | `punct?` | DONE | ASCII runs 33..47, 58..64, 91..96, 123..126. |
| `<=>` | `<=>` | PARTIAL | The METHOD spelling works (`a.<=>(b)`), returning -1/0/1 by codepoint. The OPERATOR spelling does NOT: `<=>` keeps its dedicated comparison route, which rejects `Char` operands with E0707. Needs the comparison route to consult `.rx` methods. |
| `succ` / `next` | `succ` / `next` | PARTIAL | The next CODEPOINT (`?z.succ == ?{`). CRuby's `String#succ` CARRIES (`"z".succ == "aa"`), which a single `Char` cannot represent. |
| `to_s` | `to_s` | DONE | `ruxen_char_to_string` (pre-existing). |
| `casecmp` / `casecmp?` / `between?` | — | MISSING | |

---

## Array  (`library/std/array/src/lib.rx`)

Strongest core class. Verified working: builders, `size`/`count`/`capacity`/
`empty?`, `get`/`get_mut`/`first`/`last`/`include?`, `push`/`pop`/`insert`/
`remove`/`swap`/`clear`/`truncate`/`extend`/`dedup`, `reverse`/`sort`/`clone`/
`to_a`/`to_set`/`take`/`drop`/`chain`/`sum`, `[]`/`[]=`/`[range]`, and the
`Enumerable` combinators + `sort_by`/`zip`.

| Ruby method | Status | Ruxen symbol / gap | Note |
|---|---|---|---|
| `push` / `append` | DONE | `ruxen_vec_push` | `<<` operator is broken; use `push`. |
| `pop` | DONE | `ruxen_vec_pop` → `Option[T]` | |
| `shift` / `unshift` / `prepend` | MISSING | — | no front ops. |
| `<<` | MISSING (BUG) | use `push` | no-op operator. |
| `+` | DONE (`def +`→`chain`, pin 984); `concat` still MISSING. |
| `-` | MISSING (BUG) | — | crashes. |
| `*` (repeat/join) | MISSING (BUG) | — | garbage. |
| `&` / `\|` (set ops) | MISSING (BUG) | use `.to_set` ops | garbage. |
| `[]` (index) | DONE | subscript | `a[i]`, negative? untested. |
| `[]=` | DONE | subscript assign | `a[i]=x`. |
| `[range]` / `slice` | PARTIAL | `a[0..1]` works | `slice(i,n)` method MISSING; range-subscript works. |
| `at` | MISSING | use `get(i)` / `a[i]` | `get` → `Option`. |
| `values_at` / `dig` / `assoc` / `rassoc` | MISSING | — | |
| `first` / `last` | DONE | `ruxen_vec_first`/`_last` → `Option[T]` | no `first(n)`/`last(n)`. |
| `first(n)` / `last(n)` | MISSING | use `take(n)` | arity overloading does not reach a builtin head: with both `first` and `first(n)` defined, `a.first(2)` binds the ZERO-arg body and dies in the Cranelift verifier (`got 2, expected 1`). `take(n)` is what Ruby's `first(n)` returns. |
| `index` / `find_index` | PARTIAL | `index {block}` / `find_index {block}` (Enumerable, pin 1061) | predicate form only; the value-arg `index(x)` needs `T` equality (B20). |
| `rindex` | MISSING | — | |
| `include?` / `member?` | PARTIAL | `include?` DONE; `member?` MISSING | |
| `each` | DONE | `ruxen_vec_each` | iteration primitive. |
| `each_with_index` | DONE | Enumerable body | |
| `each_index` / `each_with_object` | PARTIAL | `each_with_object(memo)` `.rx` Enumerable (pin 1062) | `each_index` MISSING. `each_with_object` threads the memo THROUGH the block (`{ |x, memo| ... }` must return it) because Ruxen closures take arguments by value; the Ruby argument ORDER is preserved. |
| `reverse_each` / `cycle` | DONE | `.rx` Enumerable defaults (pin 1062) | `cycle(n)` counted form only: Ruby's arg-less infinite form and its enumerator form are not expressible. |
| `map` / `collect` | PARTIAL | `map` DONE (Enumerable); `collect` MISSING | the name `collect` is claimed by the iterator `collect[Target]` interception (`typeck/infer/expr.rs`, `mir/lower/expr/method_call.rs`), which fires for ANY receiver before method resolution. Ruby's `collect` needs that interception narrowed first. |
| `flat_map` / `collect_concat` | DONE | `.rx` Enumerable defaults (pin 1061) | one level deep, as in Ruby. Required dropping `flat_map` from the unimplemented-Vec blocklist in `codegen/lang_intrinsics.rs`. |
| `select` / `filter` | PARTIAL | `select` DONE; `filter` MISSING | no `filter` alias. |
| `reject` | DONE | Enumerable | |
| `filter_map` | DONE | `.rx` Enumerable default (pin 1061) | the block returns `Option[U]`; `nil` is the skip signal, which is the typed form of Ruby's truthiness test. |
| `find` / `detect` | PARTIAL | `find` DONE; `detect` MISSING | |
| `find_all` | DONE | `.rx` Enumerable delegating to `select` (pin 985). |
| `find_index` | PARTIAL | `.rx` Enumerable delegating to `index` (pin 1061) | predicate form only; the value form needs `T` equality (B20). |
| `all?` / `any?` | DONE | Enumerable | |
| `none?` / `one?` | DONE | `.rx` Enumerable defaults (pin 985). |
| `count` | PARTIAL | `ruxen_vec_count` (no-arg); `.rx` Enumerable `count` for other heads (pin 1060) | `count {block}` / `count(obj)` still MISSING. |
| `reduce` / `inject` | PARTIAL | `reduce(seed){}` DONE; `inject` MISSING | no symbol form `reduce(:+)`, no no-seed form. |
| `sum` | PARTIAL | `ruxen_vec_sum` (`where T: Add`) | no `sum(init)` / `sum{block}`. |
| `min` / `max` | DONE | `.rx` body, per-`T` monomorphized (pin 991) | Ruby semantics: `nil` on empty. Orders by the element type's own `<` / `>` (numeric for Int, lexicographic for String). |
| `minmax` | MISSING | — | |
| `min_by` / `max_by` / `minmax_by` | MISSING | — | |
| `sort` | DONE | `ruxen_vec_sort` | natural order. |
| `sort!` | MISSING | — | |
| `sort_by` | PARTIAL | `.rx` body | in-place comparator-closure `sort_by(cmp)`, signature differs from Ruby's key-extractor `sort_by{}` (returns nil, mutates). |
| `uniq` | DONE | `.rx` body, per-`T` monomorphized (pin 991) | keeps the FIRST occurrence of each distinct element, in order, by the element type's own `==` (content equality for `String`). No block form (`uniq {}`). |
| `uniq!` | MISSING | `dedup` (adjacent only) | `dedup` ≠ Ruby `uniq` (only removes consecutive dups). |
| `flatten` / `flatten!` | MISSING | — | |
| `compact` / `compact!` | MISSING | — | |
| `zip` | DONE | `.rx` body → `Array[(T,U)]` | |
| `take` / `drop` | DONE | `ruxen_vec_take`/`_skip` | |
| `take_while` / `drop_while` | DONE | `.rx` Enumerable defaults (pin 1060) | latch semantics match Ruby: `drop_while` keeps the element that first failed the predicate and everything after it. |
| `partition` | DONE | Enumerable → `(Array,Array)` | |
| `group_by` | MISSING | — | |
| `chunk` / `chunk_while` / `slice_when` | MISSING | — | |
| `each_slice` / `each_cons` | DONE | `.rx` Enumerable defaults (pin 1062, edges pin 1063) | short trailing run still yielded; a window wider than the collection yields nothing; `n <= 0` yields nothing instead of looping. |
| `tally` | MISSING | — | |
| `rotate` / `rotate!` | MISSING | — | |
| `sample` / `shuffle` / `shuffle!` | MISSING | — | (no PRNG surface, see Kernel.) |
| `combination` / `permutation` / `product` | MISSING | — | |
| `pack` | MISSING | — | |
| `join` | DONE | `.rx` body, per-`T` monomorphized (pin 991) | `[].join == ""`, separator defaults to `", "`. Formats every element type through its own display (`ruxen_vec_join` retired). Paren-less `a.join` still needs `a.join()`. Element type `Float` prints truncated — a pre-existing float-word ABI bug, not a `join` bug. |
| `fill` | MISSING | — | |
| `delete` / `delete_at` / `delete_if` / `keep_if` / `reject!` / `select!` | PARTIAL | `select!` has a resolver arm | most delete/keep ops MISSING; `select!` present via `collections.rs`. |
| `insert(i,x)` | DONE | `ruxen_vec_insert` | single-element. |
| `remove(i)` / `delete_at` | PARTIAL | `remove(i)` DONE (Ruxen name) | returns element by value. |
| `clear` / `truncate` | DONE | `ruxen_vec_clear`/`_truncate` | |
| `swap(i,j)` | DONE (Ruxen-native) | `ruxen_vec_swap` | |
| `extend(other)` | DONE (Ruxen-native) | `ruxen_vec_extend` | in-place concat. |
| `chain(other)` | DONE (Ruxen-native) | `ruxen_vec_chain` | returns new array (Ruby `+`). |
| `dedup` | DONE (Ruxen-native) | `ruxen_vec_dedup` | adjacent-dup removal. |
| `size` / `length` | PARTIAL | `size` DONE; `length` MISSING | |
| `capacity` | DONE (Ruxen-native) | `ruxen_vec_capacity` | |
| `to_a` | DONE | `alias to_a clone` | shallow copy. |
| `to_h` | PARTIAL | `ruxen_hash_from_iter` | from `Array[(K,V)]`; resolver-arm typed. |
| `to_set` | DONE | `ruxen_set_from_iter` | |
| `dup` / `clone` | PARTIAL | `clone` DONE; `dup` MISSING | shallow (heap elems aliased). |
| `freeze` / `frozen?` / `hash` / `eql?` / `<=>` | MISSING | — | |
| `bsearch` / `bsearch_index` | MISSING | — | |
| `lazy` | MISSING | — | |
| `new` / `with_capacity` | DONE | builders | |

---

## Hash  (`library/std/map/src/lib.rx`, surface class `Hash[K,V]`)

Verified working: builders, `size`/`empty?`, `get`/`key?`, `keys`/`values`/
`to_a`, `insert`/`remove`/`clear`, `[]`/`[]=`, and the `Enumerable[(K,V)]`
combinators.

| Ruby method | Status | Ruxen symbol / gap | Note |
|---|---|---|---|
| `[]` (fetch) | DONE | subscript | `h[k]`. |
| `[]=` / `store` | PARTIAL | `[]=` DONE; `store` MISSING | also `insert(k,v)` (Ruxen name) DONE. |
| `fetch` | MISSING | use `get` / `h[k]` | `get` → `Option[&V]`; no default/block `fetch`. |
| `dig` | MISSING | — | |
| `get(k)` | DONE (Ruxen name) | `ruxen_hash_get` → `Option[&V]` | |
| `key?` / `has_key?` / `include?` / `member?` | DONE | `key?` + `alias has_key?`/`include?`/`member?` (pins 986, 990). | one `Hash_key?` body serves all four; the delegating bodies went away with Q58. |
| `value?` / `has_value?` | MISSING | — | |
| `keys` / `values` | DONE | `ruxen_hash_keys`/`_values` | |
| `values_at` / `fetch_values` | MISSING | — | |
| `each` / `each_pair` | DONE | `each` (yields `(K,V)`) + `alias each_pair each` (pin 990). |
| `each_key` / `each_value` | DONE | `.rx` over `each` + tuple access (pin 986). |
| `map` / `select` / `filter` / `reject` | PARTIAL | `map`/`select`/`reject` DONE (Enumerable) | closure gets a `(K,V)` tuple; `filter` alias MISSING. |
| `filter_map` / `flat_map` | DONE | `.rx` Enumerable defaults (pin 1064) | the block sees the `(K, V)` pair. |
| `find` / `detect` | PARTIAL | `find` DONE; `detect` MISSING | |
| `all?` / `any?` / `none?` / `one?` | DONE | all four `.rx` Enumerable defaults (pin 985 for none?/one?). |
| `reduce` / `inject` | PARTIAL | `reduce(seed){}` DONE | |
| `partition` | DONE | Enumerable | |
| `each_with_index` | DONE | Enumerable | |
| `each_with_object` | DONE | `.rx` Enumerable default (pin 1064) | memo is threaded through the block, which must return it. |
| `count` | DONE | `.rx` Enumerable default (pin 1064) | O(n) walk over `each`, no `Hash` length primitive behind it. |
| `sum` / `min_by` / `max_by` / `sort_by` / `group_by` / `tally` | MISSING | — | Enumerable subset not covered. |
| `merge` / `merge!` / `update` | MISSING | — | |
| `transform_keys` / `transform_values` (+ `!`) | MISSING | — | |
| `invert` | MISSING | — | |
| `to_a` | DONE | `ruxen_hash_entries` → `Array[(K,V)]` | |
| `to_h` | MISSING | — | |
| `slice` / `except` | MISSING | — | |
| `delete` / `delete_if` / `reject!` / `select!` / `keep_if` | PARTIAL | `remove(k)` DONE (Ruxen name) → `Option[V]` | Ruby `delete`+block/predicate MISSING. |
| `default` / `default=` / `default_proc` | MISSING | — | no default-value protocol. |
| `key(v)` / `assoc` / `rassoc` | MISSING | — | |
| `length` / `size` | PARTIAL | `size` DONE; `length` MISSING | |
| `empty?` | DONE | `ruxen_hash_is_empty` | |
| `clear` | DONE | `ruxen_hash_clear` | |
| `replace` | MISSING | — | |
| `compare_by_identity` | MISSING | — | |
| `dup` / `clone` / `freeze` / `frozen?` / `hash` / `eql?` / `==` | MISSING | — | |
| `flatten` / `first` | MISSING | — | |
| `new` / `with_capacity` | DONE | builders | no default-value ctor. |

---

## Range

`for x in a..b` iteration works (verified sum-loop = 6). As a first-class object
the method surface is nearly empty and partly broken.

| Ruby method | Status | Note |
|---|---|---|
| `a..b` / `a...b` literals | DONE (COMPILE-TIME) | inclusive + exclusive; usable in `for`. |
| `each` | PARTIAL | resolves; `for` iteration works. |
| `map` | PARTIAL (BUG) | resolves but returns empty at runtime. |
| `size` | PARTIAL | resolves (correctness unverified). |
| `to_a` / `to_set` / `entries` | MISSING | |
| `include?` / `member?` / `cover?` / `===` | MISSING | |
| `step` | MISSING | |
| `min` / `max` / `minmax` | MISSING | |
| `sum` / `count` | MISSING | |
| `first` / `last` / `begin` / `end` | MISSING | |
| `select` / `reject` / `reduce` / `find` | MISSING | (Enumerable not mixed into Range.) |
| `exclude_end?` | MISSING | |
| `lazy` | MISSING | |

Range is currently an iteration construct, **not** an Enumerable object.

---

## Set  (`library/std/set/src/lib.rx`)

Verified working: builders, `size`/`empty?`/`include?`, `to_a`, `union`/
`intersection`/`difference`, `insert`/`remove`/`clear`, Enumerable combinators.

| Ruby method | Status | Ruxen symbol / gap | Note |
|---|---|---|---|
| `add` / `<<` | MISSING | use `insert(x)` | Ruby names absent. |
| `add?` | MISSING | — | |
| `insert(x)` | DONE (Ruxen name) | `ruxen_set_insert` | |
| `delete` | PARTIAL | `remove(x)` → `Bool` | Ruxen name. |
| `include?` / `member?` / `===` | PARTIAL | `include?` DONE | `member?` alias MISSING. |
| `size` / `length` / `count` | PARTIAL | `size` DONE; `count` `.rx` Enumerable (pin 1064) | `length` MISSING. `count` is an O(n) walk over `each`, not the `size` primitive. |
| `empty?` | DONE | `ruxen_set_is_empty` | |
| `to_a` | DONE | `ruxen_set_iter` → `Array[&T]` | |
| `union` / `\|` / `+` | PARTIAL | `union(o)` DONE; operators broken | `s1 \| s2` ⇒ 0. |
| `intersection` / `&` | PARTIAL | `intersection(o)` DONE; `&` broken | |
| `difference` / `-` | PARTIAL | `difference(o)` DONE; `-` broken | |
| `^` (symmetric diff) | MISSING (BUG) | — | operator garbage; no method. |
| `subset?` / `superset?` / `proper_*` / `<=` / `>=` / `<` / `>` | MISSING | — | |
| `disjoint?` / `intersect?` | MISSING | — | |
| `merge` | MISSING | — | |
| `map` / `select` / `reject` / `find` / `reduce` / `all?` / `any?` / `partition` / `each` / `each_with_index` / `index` | DONE | Enumerable | |
| `first` / `entries` / `take` / `drop` / `take_while` / `drop_while` / `flat_map` / `collect_concat` / `filter_map` / `find_index` / `each_slice` / `each_cons` / `each_with_object` / `cycle` / `reverse_each` | DONE | `.rx` Enumerable defaults (pin 1064) | `Set` iterates in hash-bucket order, NOT insertion order, so the order-sensitive members of this group differ from Ruby's insertion-ordered `Set`. |
| `clear` | DONE | `ruxen_set_clear` | |
| `to_set` / `dup` / `clone` / `freeze` / `==` | MISSING | — | |
| `new` / `with_capacity` | DONE | builders | |

---

## Enumerable (the mixin)  (`library/std/core/src/lib.rx`)

`mixin Enumerable[T]` is real and reused across Array / Hash / Set via a single
required `each`. It covers a **subset** of Ruby's ~60 Enumerable methods.

| Ruby Enumerable method | Status | Note |
|---|---|---|
| `each` (required) | DONE | the iteration primitive. |
| `map` / `collect` | PARTIAL | `map` DONE; no `collect` alias. |
| `select` / `filter` / `find_all` | DONE | `select`+`filter`+`find_all` all `.rx` Enumerable. |
| `reject` | DONE | |
| `find` / `detect` | PARTIAL | `find` DONE; no `detect`. |
| `find_index` | PARTIAL | predicate form delegates to `index` (pin 1061); the value form needs `T` equality (B20). |
| `index` | DONE | predicate form. |
| `all?` / `any?` | DONE | closure form only (no arg/pattern form). |
| `none?` / `one?` | DONE | `.rx` defaults (pin 985). |
| `partition` | DONE | |
| `each_with_index` | DONE | |
| `reduce` / `inject` | PARTIAL | `reduce(seed){}` DONE; no `inject`, no symbol/no-seed forms. |
| `flat_map` / `collect_concat` | DONE | `.rx` defaults (pin 1061); one level deep. |
| `filter_map` | DONE | `.rx` default (pin 1061); block returns `Option[U]`, `nil` skips. |
| `min` / `max` / `minmax` / `min_by` / `max_by` / `minmax_by` | MISSING | |
| `sort` / `sort_by` | MISSING (Enumerable) | (Array has its own.) |
| `sum` / `count` / `tally` | PARTIAL | `count` (no-arg) is an `.rx` default (pin 1060). `sum` / `tally` need `T` ordering / equality (B20). |
| `group_by` / `chunk` / `chunk_while` / `slice_when` | MISSING | `group_by` / `tally` need `T` equality to key the result hash, so they are B20, not B07. |
| `each_slice` / `each_cons` / `each_with_object` | DONE | `.rx` defaults (pin 1062, edges pin 1063). `each_with_object` threads the memo through the block. |
| `take` / `drop` / `take_while` / `drop_while` | DONE | all four `.rx` defaults (pin 1060), so `Set` / `Hash` get them too. |
| `first` / `first(n)` | PARTIAL | `first` DONE (pin 1060). `first(n)` MISSING: arity overloading does not reach a builtin head (verifier: `got 2, expected 1`); `take(n)` carries the behavior. |
| `zip` | MISSING (Enumerable) | (Array has its own.) |
| `to_a` / `entries` / `to_h` / `to_set` | PARTIAL | `entries` DONE (pin 1060). `to_a` stays per-head (`Array` / `Set` / `Hash` each home their own with a head-specific element type); `to_h` / `to_set` MISSING. |
| `include?` / `member?` | MISSING (Enumerable) | |
| `reverse_each` / `cycle` | DONE | `.rx` defaults (pin 1062); `cycle(n)` counted form only. |
| `uniq` / `flatten` / `lazy` / `chunk` / `grep` / `grep_v` | MISSING | `uniq` needs `T` equality (B20); `lazy` needs an enumerator protocol. |

Note: all combinators take a `do |x| … end` block (Ruby-style). `{ }` blocks
are *stored* closures, `do…end` are immediately-invoked (see workspace ledger).

---

## Comparable (the mixin)  (`library/std/core/src/lib.rx`)

| Ruby feature | Status | Note |
|---|---|---|
| `mixin Comparable` | DONE (ordering) | Requires `<=>`; shared `.rx` defaults provide `==`, `<`, `<=`, `>`, `>=`, and `between?`. |
| `<` `<=` `==` `>` `>=` | DONE | Dispatches through a hand-written `<=>` when `Comparable` is included (case `965`). |
| `<=>` | DONE | Primitive and user-defined forms are available (cases `959/965`). |
| `between?(min,max)` | DONE | Shared Comparable default (case `965`). |
| `clamp(min,max)` / `clamp(range)` | MISSING | Needs an ownership-safe return contract for `self` vs. either bound. |

`include Comparable` plus `def <=>(other: &Self) -> Int` is the hand-written
ordering protocol. `clamp` is the remaining core method gap.

---

## Kernel / Object

| Ruby method | Status | Ruxen form / gap | Note |
|---|---|---|---|
| `puts` | DONE | builtin | String/interp only. |
| `print` | DONE | builtin | |
| `p` | PARTIAL | `.rx` free fn, one overload per type (pin 1180) | prints the INSPECT form (a `String` gets quotes) and RETURNS its argument, as in Ruby. Defined for `Int` / `Bool` / `String` only: Ruxen has no universal `to_s`, so `p` cannot be universal the way Ruby's is. No escaping of embedded quotes or newlines. |
| `pp` | PARTIAL | `.rx` free fn (pin 1180) | identical output to `p`; the values reachable here have no nested structure to lay out. Same per-type limit as `p`. |
| `print` to stderr / `warn` | DONE | `warn(msg)` `.rx` free fn over `eputs` (pin 1181) | writes to stderr with a newline, leaving stdout untouched. Ruby's `uplevel:` / `category:` keywords need kwargs and are not modelled. |
| `puts`-family extras | DONE | `println`, `eputs`, `eprintln` | Ruxen-native. |
| `gets` / `readline` | MISSING | — | `undefined variable gets`. |
| `format` / `sprintf` | MISSING | use `"#{}"` interpolation | `undefined function`. |
| `printf` | MISSING | — | |
| `rand` / `srand` | PARTIAL | `rand(n)` `.rx` free fn over `Rand.random_u64` (pin 1181) | `rand(n)` returns `[0, n)`; the signed 64-bit draw is folded non-negative (twice, so `Int::MIN` cannot leak through) before the modulo. Ruby's arg-less `rand` returns a `Float` in `[0.0, 1.0)` and needs the `Float` ABI (B01), so `rand(0)` yields `0` here. `srand` MISSING and NOT shippable: `std/rand` exposes only OS entropy, with no seedable generator to make reproducible. |
| `loop do…end` | COMPILE-TIME | keyword | language construct (see `language.md`); runtime behavior of the observed probe was inconclusive. |
| `raise` / `fail` | MISSING | use `panic!(msg)` / `Result` + `?` | `raise` undefined; error model (exceptions vs `Result`) is a language-design call, see `language.md`, not a metaobject gap. |
| `panic!(msg)` | DONE (Ruxen-native) | MIR intrinsic | aborts. |
| `exit` / `exit!` | PARTIAL | `exit(code)` DONE, a pre-existing bare FFI decl in `process/src/lib.rx` (pin 1183) | `exit!` MISSING and deliberately not shipped: a free function whose name ends in `!` compiles clean and then its CALL SITE does nothing (verified with `def shout!(x)…end; shout!(5)`), so `exit!` would have been a user-callable silent no-op. Ruxen has no at-exit registry, so `exit` and `exit!` would have had identical behavior anyway. |
| `abort` | DONE | `.rx` free fn (pin 1182) | writes `msg` to stderr, then terminates with status 1. Returns `Never`. |
| `sleep` | PARTIAL | `sleep(secs)` `.rx` free fn over `Thread.sleep` (pin 1181) | INTEGER seconds only. Ruby's fractional form needs a `Float` argument, which does not survive the current ABI (B01) and would silently sleep zero. Sub-second waits use `Thread.sleep(Duration.from_millis(n))`. |
| `Integer()` / `Float()` | MISSING | use `"42".to_i` / `"1.5".to_f` | NOT shippable as written: a capitalized identifier lexes as a `TypeIdentifier`, so `def Integer(s)` is a parse error (`expected identifier, found TypeIdentifier("Integer")`). `Float()` additionally waits on the `Float` ABI (B01). |
| `String()` | MISSING | use `.to_s` | same capitalized-name parse block as `Integer()`, plus it needs a universal `to_s`. |
| `Array()` / `Hash()` | MISSING | — | same capitalized-name parse block as `Integer()`. |
| `inspect` | MISSING | `p(x)` / `"#{x}"` via `Debug` derive | no `.inspect` METHOD: a `mixin Kernel` default would have to call `self.to_s`, which is not universal, and `mixin` defaults are emitted for every including head, so it would break every type without one. The inspect FORM is reachable through `p` / `pp` for the types those cover. |
| `to_s` | DONE | per-type + `Debug`/`Displayable` derive | |
| `class` | COMPILE-TIME | — | see Reflection cluster below; `x.class` also needs a surface spelling (`class` keyword parse-clash today). |
| `is_a?` / `kind_of?` / `instance_of?` | COMPILE-TIME | — | static type + known `include` set → constant `Bool`. See Reflection cluster. |
| `respond_to?` (literal method) | COMPILE-TIME | — | fixed method set → constant-fold. See Reflection cluster. |
| `send` / `public_send` / `__send__` (literal name) | COMPILE-TIME | — | constant name → static dispatch. See Reflection cluster. |
| `define_method` / `method` / `methods` / `instance_variable_*` | COMPILE-TIME | — | derive/codegen + fn references. See Reflection cluster. |
| `tap` | DONE | `.rx` `mixin Kernel` (universal include), pin 973/983. |
| `then` / `yield_self` | DONE | `.rx` `mixin Kernel` (universal include); `yield_self` a real method, pin 973. |
| `freeze` / `frozen?` | MISSING | — | no value-freeze protocol. |
| `dup` / `clone` | PARTIAL | `.clone` on String/Array/collections | universal `dup` MISSING; `clone` derive for user types via `include Clone`. |
| `object_id` / `__id__` | MISSING | — | runtime identity; a pointer-identity intrinsic could supply it (not a metaobject need). |
| `==` / `eql?` / `equal?` | PARTIAL | `==` for scalars/String | structural `==` works for a **`struct`** via `include PartialEq`/derive, and is BROKEN for a **`class`**: `C.new(1) == C.new(1)` is `false` with or without an explicit include (**B74**). Pin `133` only covers a struct, which is why it went unnoticed. `eql?`/`equal?` MISSING. |
| `hash` | PARTIAL | `Hashable` mixin for user types | no universal `.hash` on values. A **`struct`** carrying `Hashable` + `PartialEq` now works as a `Hash`/`Set` KEY (B73, pin `1722`), routed through the synthesized `hash_code` and `eq`. A **`class`** key still compares by identity, blocked on B74. |
| `nil?` | MISSING | use `match`/`Option` | `x.nil?` unresolved even on `Option` (drop-Some coercion). |
| `!` / `!=` | DONE | operators | |
| `__method__` / `__callee__` | COMPILE-TIME | compiler knows the enclosing method statically | fold to the method-name string literal at the call site. |
| `caller` / `binding` | OUT-OF-SCOPE | — | needs a runtime call stack / live binding. |
| `catch` / `throw` | MISSING | — | non-local control flow; lowerable as a language construct (see `language.md`), not a metaobject. |
| `eval` / `instance_eval` / `class_eval` | OUT-OF-SCOPE | — | arbitrary code from a runtime string, irreducible without an interpreter. |

**Reflection cluster, COMPILE-TIME, not out-of-scope.** Ruxen is a pure
compiler: every type, field, and method is fully known at compile time, so the
*results* Ruby computes at runtime are computable at compile time. These are
tier-2 (compile-time) parity, delivered by a `match` over the fixed type/method
set, `derive`/mixin codegen, or a macro, not a runtime metaobject:

| Ruby reflective method | Status | Compile-time design that delivers it |
|---|---|---|
| `class` / `.class.name` | COMPILE-TIME | receiver's static type is known; lower `x.class` to its type token / name string. (Today `x.class` also parse-errors on the `class` keyword, needs a surface spelling.) |
| `is_a?` / `kind_of?` / `instance_of?` | COMPILE-TIME | static type + the known mixin/`include` set → a compile-time constant `Bool` (fold to `true`/`false`); for enums, a `match` on the variant. |
| `respond_to?(:m)` (literal `m`) | COMPILE-TIME | the method set of every type is fixed at compile time → constant-fold to `true`/`false`. |
| `send(:m, …)` / `public_send` (literal `m`) | COMPILE-TIME | a constant method name folds to direct static dispatch (`x.send("foo")` ≡ `x.foo`). |
| `send(name, …)` (name unknown until runtime) | OUT-OF-SCOPE | closest substitute: a `match` over the finite set of valid names dispatching statically. |
| `define_method` / `attr_accessor`-style | COMPILE-TIME | codegen: a `derive`/mixin or macro synthesizes the method bodies (the tier-2 mechanism the README describes for ORMs/DSLs). |
| `method(:m)` / `instance_method(:m)` | COMPILE-TIME | a first-class function/closure reference to the method (Ruxen already has fn references + `.()`); no `UnboundMethod` object. |
| `methods` / `instance_methods` | COMPILE-TIME | the method set is a compile-time-known list; emit it as a literal array. |
| `instance_variables` / `_get` / `_set` (literal name) | COMPILE-TIME | fields are statically known; enumerate via derive; `instance_variable_get("@x")` with a constant folds to field access. Runtime-unknown name → OUT-OF-SCOPE (substitute: `match` over the known field set). |

Genuinely irreducible (tier-3, need an interpreter/live metaobject Ruxen omits):
`eval`/`binding`/`instance_eval`, `ObjectSpace`, `TracePoint`, `caller`, and
`send`/`instance_variable_get` with a name not known until runtime, each with
its closest compile-time substitute noted above.

---

## NilClass / TrueClass / FalseClass

| Ruby feature | Status | Note |
|---|---|---|
| `nil` literal | DONE | also the empty `Option` (`nil` in `match` arms). |
| `Int?` optional types | DONE (COMPILE-TIME) | sugar for `Option[Int]`. |
| `Option[T] ?? T` fallback | DONE | case `966`; lazy and right-associative. |
| `true` / `false` literals | DONE | |
| `TrueClass#to_s` / `FalseClass#to_s` | DONE | `class Bool` → `ruxen_bool_to_string`. |
| `Bool#to_string` | DONE | |
| `true & false`, `\| `, `^` | DONE | operators, non-short-circuiting exactly as in Ruby (`&&` / `||` are the short-circuiting pair in both). Nothing was added in `.rx`; batch B18 pinned them (pin `1171`) so they cannot regress. |
| `!` (TrueClass/FalseClass) | DONE | machine unary operator; pin `1171`. |
| `Bool#to_i` | DONE | Batch B18. Ruby has NO `true.to_i`; the 1/0 mapping is what a Ruby developer writes as `flag ? 1 : 0`. Pin `1171`. |
| `NilClass#to_s` / `nil.to_s`→`""` | MISSING | no method surface on the nil value. |
| `nil?` / `to_a` / `to_h` on nil | MISSING | use `match`. |
| `TrueClass`/`FalseClass`/`NilClass` as reified classes | COMPILE-TIME | these types are known statically; no runtime class object is needed, `match` / static dispatch handles the branches Ruby resolves via the class. |

---

## Proc / Method / blocks

| Ruby feature | Status | Note |
|---|---|---|
| Blocks `do \|x\| … end` / `{ \|x\| … }` | DONE (COMPILE-TIME) | `do…end` = immediately-invoked, `{ }` = stored closure. |
| `&block` params, `yield`, `block_defined?` | DONE (COMPILE-TIME) | Ruby-style block protocol. |
| Closure types `any Fn[Fn(T)->U]`, call via `f.(args)` | DONE (COMPILE-TIME) | first-class closures. |
| `Proc.new` / `proc` / `lambda` / `->(){}` | MISSING | no `Proc` object constructors; closures are values, not `Proc` instances. |
| `Proc#call` / `#()` / `#[]` / `#===` | PARTIAL | `f.(…)` DONE; `[]`/`===` MISSING. |
| `Proc#arity` | COMPILE-TIME | closure signature is statically known → constant-fold the arity. |
| `Proc#curry` / `#lambda?` | MISSING | curry is a compile-time transform (closure over partial args); `lambda?` folds to a constant. |
| `method(:x)` (literal `x`) | COMPILE-TIME | a first-class function reference to the named method (Ruxen has fn references + `.()`); no `Method` object. |
| `Method` / `UnboundMethod` reification | OUT-OF-SCOPE | no runtime metaobject; the fn-reference substitute above covers call-site use. |
| `Symbol#to_proc` (`&:upcase`) | MISSING | blocked on the Symbol type (a language-design decision); a `do \|x\| x.upcase end` block is the direct substitute. |

Detailed block/closure semantics belong to `language.md`; summarized here for
core-class completeness.

---

## Regexp / MatchData  (`library/std/regex/src/lib.rx`)

Well-developed, PCRE2-backed. `Regex`, `Match`, `RegexError`, `/pat/flags`
literals, and the `~=` operator.

| Ruby feature | Status | Ruxen symbol | Note |
|---|---|---|---|
| `/pat/flags` literals | DONE (COMPILE-TIME) | `ruxen_regex_compile_const` | compiled once at module init; flags `[imsgx]`. |
| `=~` | PARTIAL | `~=` operator → `is_match` | Ruxen spelling `~=`; returns `Bool` (not index). |
| `Regexp.new` / `.compile` | DONE | `Regex.new(pat,flags)` → `Result[Regex,RegexError]` | |
| `match` | PARTIAL | `Regex#find` → `Option[Match]` | renamed (`match` is a keyword). |
| `match?` | DONE | `Regex#is_match` → `Bool` | |
| `scan` (on Regexp) | DONE | `Regex#scan` → `Array[Match]` | |
| `String#scan` / `#gsub(/re/)` / `#sub(/re/)` / `#split(/re/)` | DONE | `.rx` overloads on `String` delegating to `Regex` (pin 1101) | all four now spell as in Ruby, selected by argument TYPE against their string-argument siblings; `String#match` / `#match?` / `#index(regexp)` too. The `Regex` methods remain the direct route. |
| `Regexp.escape` / `.quote` / `.union` | MISSING | — | |
| `Regexp#source` / `#options` / `#names` / `#casefold?` | MISSING | — | |
| `MatchData#[]` / `#to_a` / `#captures` | PARTIAL | `Match#group(n)` / `#groups` | `group(0)`=whole; `groups`→`Array[Option[String]]`. |
| `MatchData#named_captures` / `#[name]` | PARTIAL | `Match#named(name)` / `#named_groups` | `named_groups`→`Hash[String,String]` (omits non-participating). |
| `MatchData#pre_match` / `#post_match` / `#names` / `#values_at` | MISSING | — | |
| `MatchData#begin` / `#end` / `#offset` | PARTIAL | `Match#start_pos` / `#end_pos` | whole-match byte offsets only (not per-group). |
| `MatchData#[0]` (matched) | DONE | `Match#matched` → `String` | |
| `RegexError#message` / `#offset` | DONE | | compile error + byte offset. |
| global `$~` `$1` `$&` etc. | MISSING | — | no implicit match globals; the explicit `Match` object (`find` → `Option[Match]`) is the substitute, not a metaobject gap. |

---

## Time / Duration / Instant  (`library/std/time/src/lib.rx`)

Ruxen ships **monotonic + wall-clock primitives**, not Ruby's calendar `Time`.

| Ruby Time feature | Status | Ruxen equivalent | Note |
|---|---|---|---|
| `Time.now` | PARTIAL | `Time.unix_ns` → Int (ns since epoch) | raw nanoseconds, not a Time object. |
| `Instant.now` / `#elapsed` / `#duration_since` / `-` | DONE | `class Instant` | monotonic clock (Ruxen-native). |
| `Duration` (from/as secs/millis/micros/nanos, `+`/`-`) | DONE | `class Duration` | Ruxen-native value type. |
| `Time.at` / `Time.new(y,m,d,…)` / `Time.parse` | MISSING | — | no calendar construction. |
| `#year`/`#month`/`#day`/`#hour`/`#min`/`#sec`/`#usec`/`#nsec` | MISSING | — | no broken-down time. |
| `#wday`/`#yday`/`#monday?`… | MISSING | — | |
| `#strftime` / `#iso8601` / `#to_s` | MISSING | — | no formatting. |
| `#to_i` / `#to_f` / `#to_r` | PARTIAL | `Time.unix_ns` | ns int only. |
| `#+`/`#-`(seconds) / comparison | PARTIAL | on `Duration`/`Instant` only | not on wall-clock time. |
| `#utc` / `#localtime` / `#zone` / `#getlocal` | MISSING | — | no timezone model. |
| `Date` / `DateTime` classes | MISSING | — | (stdlib `date`, see `stdlib.md`). |

---

## Struct

| Ruby feature | Status | Note |
|---|---|---|
| `Struct.new(:a,:b)` factory | MISSING (COMPILE-TIME substitute) | Ruxen has a native `struct Name … end` declaration instead. |
| `struct`/`class` with fields | DONE (COMPILE-TIME) | native aggregate types with typed fields + accessors. |
| `#members` / `#to_a` / `#to_h` / `#[]` / `#each` / `#==` | PARTIAL | structural `==` via `include PartialEq`; `to_h`/`members`/`[]` MISSING. |
| keyword_init / `Data.define` | MISSING | — | |

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
`p`/`pp` (debug-print, over the `Debug` derive), `format`/`sprintf` (or bless
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
generation via `derive`/mixin/macro. Only a name unknown until runtime falls
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
