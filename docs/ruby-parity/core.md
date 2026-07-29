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
| `Array[i]`, `Array[i]=`, `Hash[k]`, `Hash[k]=` | **DONE** | `a[1]`=20, `a[0]=9`, `h["a"]` all correct. |
| `Array[range]` (e.g. `a[0..1]`) | DONE | compiles+runs. |
| **`**` (power)** | **DONE** | `2**8`⇒256. `StarStar` token + `BinOp::Pow` → `ruxen_int_pow`/`ruxen_float_pow` (pin 952). |
| **`<=>` (spaceship)** | **DONE** | Primitive Int/Float/String pin `959`; user method and Comparable dispatch pin `965`. |
| **String `*`** | **DONE** | `"ab"*3`⇒"ababab". `.rx` `def *`→`repeat`, builtin-head operator route (pin 984). |
| **String `<<`** | **MISSING (BUG)** | no-op (receiver unchanged). Use `.push_str`. (`<<` mutation semantics pending.) |
| **String `[i]` (index)** | **MISSING (BUG)** | `"abc"[1]` ⇒ empty/null Char. No `[]` in `string.rx`. |
| **Array `+`** | **DONE** | `[1,2]+[3,4]`⇒`[1,2,3,4]`. `.rx` `def +`→`chain` (pin 984). **`- * &  \|`** still MISSING (need element-eq set ops). |
| **Array `<<`** | **MISSING (BUG)** | no-op. Use `.push(x)`. |
| **Set `\| & -`** | **DONE** | `.rx` `def \|`/`&`/`-`→`union`/`intersection`/`difference` (pin 984). **`^`** (symmetric diff) MISSING. |

**Also broken:** `Array[Int]#join` returns empty; `Array[String]#join` works.
`Range#map` returns empty (though `for x in a..b` iteration works).

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
| `length` | DONE | `ruxen_string_len` | Ruby `length` aliased to the same C symbol as `size`. Pin `972`. |
| `size` | DONE | `ruxen_string_len` | |
| `bytesize` | MISSING | — | no byte-length accessor. |
| `+` (concat) | DONE | operator | `"a"+"b"`. |
| `*` (repeat) | DONE | `def *`→`repeat` (pin 984). |
| `<<` / `concat` | MISSING (BUG) | use `push_str` | `<<` is a no-op. |
| `%` (format) | MISSING | use `"#{}"` interp | no `String#%`. |
| `[]` / `slice` (index) | MISSING (BUG) | — | `"abc"[1]` yields null Char; no 2-arg `[i,n]`, no `[range]`. |
| `at` | MISSING | — | |
| `byteslice` | MISSING | — | |
| `sub` / `sub!` | MISSING | — | no first-match replace. |
| `gsub` / `gsub!` | PARTIAL | `replace(from,to)` | `replace` is literal replace-**all** (≈ `gsub` with a string arg); no regex/block form, no `sub`. |
| `tr` / `tr_s` | MISSING | — | |
| `replace` (whole-string) | PARTIAL | `ruxen_string_replace` | Ruxen `replace(from,to)` = substitute, NOT Ruby `replace(other)` (overwrite contents). Name collision. |
| `split` | DONE | `ruxen_string_split` | string delimiter only; no regex/limit (see `splitn`). |
| `splitn(n, delim)` | DONE | `ruxen_string_splitn` | Ruxen-specific limited split. |
| `strip` | DONE | `ruxen_string_trim` | Ruby `strip` aliased to the same C symbol as `trim`. Pin `975`. |
| `lstrip` / `rstrip` | DONE | `trim_start` / `trim_end` | Ruby spellings aliased to the same C symbols. Pin `975`. |
| `strip!`/`lstrip!`/`rstrip!` | MISSING | — | no in-place variants. |
| `chomp` / `chop` | MISSING | — | |
| `upcase` | DONE | `to_upper` | Ruby `upcase` aliased to the same C symbol; `to_upper` also kept. Pin `972`. |
| `downcase` | DONE | `to_lower` | Ruby `downcase` aliased to the same C symbol; `to_lower` also kept. Pin `972`. |
| `capitalize` | DONE | `.rx` over `chars` + upcase/downcase (pin 987). `swapcase` MISSING. |
| `upcase!`/`downcase!`/… | MISSING | — | no bang variants. |
| `start_with?` | DONE | `ruxen_string_starts_with` | Ruby `start_with?` aliased; single string arg (no multi-arg / regex). Pin `975`. |
| `end_with?` | DONE | `ruxen_string_ends_with` | Ruby `end_with?` aliased; single arg. Pin `975`. |
| `include?` | DONE | `ruxen_string_contains` | |
| `index` | PARTIAL | `find` | `find` → `Option[USize]`; no start-offset, no regex. |
| `rindex` | MISSING | — | |
| `count` | MISSING | — | |
| `delete` / `squeeze` | MISSING | — | |
| `center` / `ljust` / `rjust` | PARTIAL | `ljust`/`rjust` DONE (`.rx`, `pad` defaults to space; pin 988/989); `center` MISSING. |
| `chars` | DONE | `ruxen_string_chars` → `Array[Char]` | |
| `bytes` | DONE | `ruxen_string_bytes` → `Array[UInt8]` | |
| `codepoints` | MISSING | — | |
| `lines` | DONE | `ruxen_string_lines` | |
| `each_char` / `each_line` / `each_byte` | MISSING | iterate `chars`/`lines`/`bytes` via `.each` | no direct `each_*`. |
| `each_grapheme_cluster` | MISSING | — | |
| `scan` | MISSING | use `Regex#scan` | on `String`: absent. |
| `match` / `match?` / `=~` | MISSING | use `Regex#find`/`is_match` / `~=` | no `String`-side regex methods; `~=` is the operator. |
| `to_i` / `to_f` | PARTIAL | `parse_int` / `parse_float` | return `Result[…]` (not lenient like Ruby `to_i`); no radix. |
| `parse_int`/`parse_float`/`parse_uint` | DONE | `ruxen_string_parse_*` | Ruxen-native, `Result`-returning. |
| `to_sym` / `intern` | MISSING | — | blocked on the Symbol type (a language-design decision), not a metaobject need. |
| `to_r` / `to_c` | MISSING | — | no Rational/Complex. |
| `to_s` / `to_str` | DONE / PARTIAL | `ruxen_string_to_string` | `to_s` DONE; `to_str` absent. |
| `ord` / `chr` | MISSING | — | |
| `hex` / `oct` | MISSING | — | |
| `succ` / `next` | MISSING | — | |
| `reverse` | DONE | `.rx` over `chars`+`Array#reverse` (pin 987). |
| `insert` | PARTIAL | `insert(i, codepoint)` / `insert_str(i, s)` | Ruxen splits into codepoint- vs string-insert; arg is an Int codepoint, not a String, for `insert`. |
| `prepend` | MISSING | — | |
| `delete_prefix` / `delete_suffix` | MISSING | — | |
| `partition` / `rpartition` | MISSING | — | |
| `freeze` / `frozen?` | MISSING | — | no freeze protocol on values. |
| `dup` / `clone` | PARTIAL | `clone` | `clone` = borrow→owned copy (DONE); no `dup`. |
| `hash` | MISSING | — | (`Hashable` mixin exists for user types; no `String#hash` surface.) |
| `eql?` / `casecmp` / `casecmp?` | MISSING | `==` works | only `==`; no `<=>`, `casecmp`. |
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

Method-home declares only `to_string` / `to_s` / `to_f` (+ `include Add`).
Arithmetic is compiler-level (see Operator Reality). **Everything else is
MISSING.**

| Ruby method | Status | Note |
|---|---|---|
| `+ - * / %` | DONE | operators. |
| `**` (pow) | MISSING (BUG) | lowered to `*`. |
| `divmod` / `fdiv` / `div` / `modulo` / `remainder` | MISSING | |
| `gcd` / `lcm` / `gcdlcm` | MISSING | |
| `abs` / `magnitude` | MISSING | (no `.abs`; `0 - n` works for negation). |
| `ceil` / `floor` / `round` / `truncate` | MISSING | |
| `times` | MISSING | use `for i in 0...n`. |
| `upto` / `downto` / `step` | MISSING | `upto`/`downto` via ranges only for ascending `for`. |
| `pred` / `succ` / `next` | MISSING | |
| `to_s` | DONE | `ruxen_int_to_string`. |
| `to_s(base)` | MISSING | 1-arg `to_s` unresolved (`no method to_s`). |
| `to_i` / `to_int` | MISSING | identity; not declared. |
| `to_f` | DONE | `ruxen_int_to_f`. |
| `to_r` / `to_c` | MISSING | no Rational/Complex. |
| `digits` | MISSING | |
| `bit_length` | MISSING | |
| `pow(n)` / `pow(n,m)` | MISSING | |
| `<< >> & \| ^ ~` | DONE (`~` untested) | bitwise operators work. |
| `even?` / `odd?` / `zero?` | MISSING | |
| `positive?` / `negative?` / `nonzero?` | MISSING | |
| `integer?` | MISSING | |
| `chr` / `ord` | MISSING | |
| `coerce` | MISSING | |
| `<=>` | DONE | Primitive operator returns -1/0/1 (case `959`). |
| `clamp` / `between?` | MISSING | (not exposed on the Int method-home.) |
| `[]` (bit ref) | MISSING | |
| `bit_length`/`size`/`numerator`/`denominator` | MISSING | |
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
| `first(n)` / `last(n)` | MISSING | — | arity-1 form unimplemented. |
| `index` / `find_index` | PARTIAL | `index {block}` (Enumerable) | predicate form only; no value-arg `index(x)`. |
| `rindex` | MISSING | — | |
| `include?` / `member?` | PARTIAL | `include?` DONE; `member?` MISSING | |
| `each` | DONE | `ruxen_vec_each` | iteration primitive. |
| `each_with_index` | DONE | Enumerable body | |
| `each_index` / `each_with_object` | MISSING | — | |
| `reverse_each` / `cycle` | MISSING | — | |
| `map` / `collect` | PARTIAL | `map` DONE (Enumerable); `collect` MISSING | no `collect` alias. |
| `flat_map` / `collect_concat` | MISSING | — | |
| `select` / `filter` | PARTIAL | `select` DONE; `filter` MISSING | no `filter` alias. |
| `reject` | DONE | Enumerable | |
| `filter_map` | MISSING | — | |
| `find` / `detect` | PARTIAL | `find` DONE; `detect` MISSING | |
| `find_all` | DONE | `.rx` Enumerable delegating to `select` (pin 985). |
| `find_index` | MISSING | — | |
| `all?` / `any?` | DONE | Enumerable | |
| `none?` / `one?` | DONE | `.rx` Enumerable defaults (pin 985). |
| `count` | PARTIAL | `ruxen_vec_count` (no-arg) | `count {block}` / `count(obj)` MISSING (verifier error). |
| `reduce` / `inject` | PARTIAL | `reduce(seed){}` DONE; `inject` MISSING | no symbol form `reduce(:+)`, no no-seed form. |
| `sum` | PARTIAL | `ruxen_vec_sum` (`where T: Add`) | no `sum(init)` / `sum{block}`. |
| `min` / `max` / `minmax` | MISSING | — | |
| `min_by` / `max_by` / `minmax_by` | MISSING | — | |
| `sort` | DONE | `ruxen_vec_sort` | natural order. |
| `sort!` | MISSING | — | |
| `sort_by` | PARTIAL | `.rx` body | in-place comparator-closure `sort_by(cmp)`, signature differs from Ruby's key-extractor `sort_by{}` (returns nil, mutates). |
| `uniq` / `uniq!` | MISSING | `dedup` (adjacent only) | `dedup` ≠ Ruby `uniq` (only removes consecutive dups). |
| `flatten` / `flatten!` | MISSING | — | |
| `compact` / `compact!` | MISSING | — | |
| `zip` | DONE | `.rx` body → `Array[(T,U)]` | |
| `take` / `drop` | DONE | `ruxen_vec_take`/`_skip` | |
| `take_while` / `drop_while` | MISSING | — | |
| `partition` | DONE | Enumerable → `(Array,Array)` | |
| `group_by` | MISSING | — | |
| `chunk` / `chunk_while` / `slice_when` | MISSING | — | |
| `each_slice` / `each_cons` | MISSING | — | |
| `tally` | MISSING | — | |
| `rotate` / `rotate!` | MISSING | — | |
| `sample` / `shuffle` / `shuffle!` | MISSING | — | (no PRNG surface, see Kernel.) |
| `combination` / `permutation` / `product` | MISSING | — | |
| `pack` | MISSING | — | |
| `join` | PARTIAL (BUG) | `ruxen_vec_join` | works for `Array[String]`; **empty/broken for `Array[Int]`** and other element types. |
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
| `filter_map` / `flat_map` | MISSING | — | |
| `find` / `detect` | PARTIAL | `find` DONE; `detect` MISSING | |
| `all?` / `any?` / `none?` / `one?` | DONE | all four `.rx` Enumerable defaults (pin 985 for none?/one?). |
| `reduce` / `inject` | PARTIAL | `reduce(seed){}` DONE | |
| `partition` | DONE | Enumerable | |
| `each_with_index` | DONE | Enumerable | |
| `each_with_object` | MISSING | — | |
| `count` | MISSING | — | (no `Hash#count`; Array-only.) |
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
| `size` / `length` / `count` | PARTIAL | `size` DONE | |
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
| `find_index` | MISSING (`index` is predicate-based) | |
| `index` | DONE | predicate form. |
| `all?` / `any?` | DONE | closure form only (no arg/pattern form). |
| `none?` / `one?` | MISSING | |
| `partition` | DONE | |
| `each_with_index` | DONE | |
| `reduce` / `inject` | PARTIAL | `reduce(seed){}` DONE; no `inject`, no symbol/no-seed forms. |
| `flat_map` / `collect_concat` | MISSING | |
| `filter_map` | MISSING | |
| `min` / `max` / `minmax` / `min_by` / `max_by` / `minmax_by` | MISSING | |
| `sort` / `sort_by` | MISSING (Enumerable) | (Array has its own.) |
| `sum` / `count` / `tally` | MISSING (Enumerable) | (Array-specific only.) |
| `group_by` / `chunk` / `chunk_while` / `slice_when` | MISSING | |
| `each_slice` / `each_cons` / `each_with_object` | MISSING | |
| `take` / `drop` / `take_while` / `drop_while` | MISSING (Enumerable) | (Array has `take`/`drop`.) |
| `first` / `first(n)` | MISSING (Enumerable) | |
| `zip` | MISSING (Enumerable) | (Array has its own.) |
| `to_a` / `entries` / `to_h` / `to_set` | MISSING (Enumerable) | |
| `include?` / `member?` | MISSING (Enumerable) | |
| `uniq` / `flatten` / `reverse_each` / `cycle` / `lazy` / `chunk`/`grep`/`grep_v`/`flat_map` | MISSING | |

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
| `p` | MISSING | — | `undefined variable p`. |
| `pp` | MISSING | — | |
| `print` to stderr / `warn` | PARTIAL | `eputs`, `eprintln` (Ruxen) | stderr writers exist under Ruxen names. |
| `puts`-family extras | DONE | `println`, `eputs`, `eprintln` | Ruxen-native. |
| `gets` / `readline` | MISSING | — | `undefined variable gets`. |
| `format` / `sprintf` | MISSING | use `"#{}"` interpolation | `undefined function`. |
| `printf` | MISSING | — | |
| `rand` / `srand` | MISSING (Kernel) | — | `undefined`. (A `std/rand` package exists; not exposed as bare Kernel fns.) |
| `loop do…end` | COMPILE-TIME | keyword | language construct (see `language.md`); runtime behavior of the observed probe was inconclusive. |
| `raise` / `fail` | MISSING | use `panic!(msg)` / `Result` + `?` | `raise` undefined; error model (exceptions vs `Result`) is a language-design call, see `language.md`, not a metaobject gap. |
| `panic!(msg)` | DONE (Ruxen-native) | MIR intrinsic | aborts. |
| `exit` / `exit!` | PARTIAL | `exit(code)` DONE | `exit!`/`abort` MISSING. |
| `abort` | MISSING | — | |
| `sleep` | MISSING (Kernel) | — | (async/time packages elsewhere.) |
| `Integer()` / `Float()` | MISSING | use `String#parse_int`/`_float` | conversion functions absent. |
| `String()` | MISSING | use `.to_s` | |
| `Array()` / `Hash()` | MISSING | — | |
| `inspect` | MISSING | `"#{x}"` via `Debug` derive | no `.inspect` method; `include Debug` gives structural render through interpolation/`to_s`. |
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
| `==` / `eql?` / `equal?` | PARTIAL | `==` for scalars/String | structural `==` for user types via `include PartialEq`/derive; `eql?`/`equal?` MISSING. |
| `hash` | PARTIAL | `Hashable` mixin for user types | no universal `.hash` on values. |
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
| `true & false`, `\| `, `^` | DONE | operators. |
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
| `String#scan` / `#gsub(/re/)` / `#sub(/re/)` / `#split(/re/)` | PARTIAL | `Regex#replace`/`replace_all`/`split` | regex ops live on `Regex`, not `String`. |
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
3. **`Array[Int]#join`**, returns empty for non-`String` element types; only
   `Array[String]` works. Fix `ruxen_vec_join` element stringification.
4. **Collection/String operators that crash or no-op**, `Array +/-/*/&/|/<<`,
   `String */<<`, `String[i]`, `Set |/&/-/^`, `Range#map`. Either implement or
   make them a compile error (never a silent crash). Prefer wiring the operators
   to the working methods (`chain`, `push`, `union`, …).

### P1, Integer / Float methods (whole family; `class Int`/`class Float`)
`abs`, `times`, `upto`, `downto`, `step`, `pow`, `gcd`, `lcm`, `divmod`, `fdiv`,
`ceil`, `floor`, `round`, `truncate`, `even?`, `odd?`, `zero?`, `positive?`,
`negative?`, `nonzero?`, `pred`, `succ`, `digits`, `bit_length`, `chr`, `to_i`,
`clamp`, `between?`, `to_s(base)`. Highest day-one-ergonomics payoff; each is a
small C symbol + one `.rx` `def`.

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
