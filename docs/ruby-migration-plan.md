# Ruxen → Ruby Surface Migration — Plan & Handoff

**Goal:** Remove every Rust-flavored surface spelling from Ruxen and replace it
with the Ruby idiom. "Reads like Ruby, compiles like Rust." Canonical spec:
`docs/specs/syntax/ruby-naming.spec.md` (this file tracks execution against it).

This document is a working handoff — pick up here in a new session.

---

## 0. CRITICAL — current working-tree state (read first)

The working tree has **~72 uncommitted files** from the in-progress phase
(predicate methods, `&.` safe-nav, collection renames, **iterator-layer removal**).
Rondo is clean (its Phase 1+2 already committed).

Before continuing:

1. **Verify, then commit** the current ruxen working tree (it is in a green,
   coherent state — see §2 "Done, uncommitted"):
   ```bash
   cd ~/.projects/ruxen
   cargo test -p ruxen_core --test stdlib_iterator --test stdlib_array_negatives \
     --lib method_resolvers::golden   # focused — should be all green
   git add -A && git commit   # message: "ruby: predicate methods, &. safe-nav, collection renames, remove iterator layer"
   ```
2. **Reinstall the toolchain** so `ruxen` on PATH has these changes (the user has
   been testing against a *stale* installed binary — that's why new methods like
   `s.to_a` looked "missing"):
   ```bash
   ruxen upgrade --from-source /Users/hassan/.projects/ruxen
   ```
3. Re-verify rondo: `cd ~/.projects/rondo && ruxen build && ruxen test` (72 tests).

> **Testing rule (user preference, saved to memory):** run *focused* tests
> (`--test <file>`, `--lib <module::path>`, or one `.rx` fixture), **not** the
> whole crate. Reserve one full `cargo test -p ruxen_core` for an end-of-phase
> integration pass. The golden corpus regenerates with
> `RECORD_GOLDEN=1 cargo test -p ruxen_core --lib method_resolvers::golden::golden_parity`.

---

## 1. Done & committed (earlier in the effort)

- **`None` → `nil`** — the single empty literal (Option::None, null pointer, **and
  unit**). Bare `None` rejected at lex time (`E0008`). `()` removed too — the unit
  type/value is `nil` (`-> nil`, `Result[nil, E]`, `Ok(nil)`); `()` type/value
  rejected by the parser.
- **String/char literals** — `'…'` is a **raw** string; `"…"` interpolates; chars
  are **`?a` / `?\n` / `?\u{…}`**. The Rust `r"…"` / `r#"…"#` form is gone.
- **Ranges** — Ruby semantics: `..` **inclusive**, `...` **exclusive**. `..=` is
  removed entirely (`TokenKind::DotDotEq` deleted; lexer rejects with `E0009`).
- **Lifetimes** — no `'a` sigil (`E0002` on a stray `'a`); lifetimes are bare
  lowercase names in the `[…]` param slot (`def f[a](x: &a String)`).
- **The `Regex.new -> Result` codegen fix** (FFI `.new` return type was shadowed
  by the builtin "constructor returns Self" rule) + regression spec
  `tests/release-e2e/cases/907_regex_match_dispatch.rx`.

## 2. Done & UNCOMMITTED (this session — commit per §0)

- **Predicate `?` method names** — lowercase identifiers absorb a trailing `?`
  (and `!`): `def empty?`, `arr.include?(x)`, `arr.any? { … }`.
  Lexer: `lex_identifier_or_keyword` in `compiler/ruxen_core/src/lexer/tokens.rs`.
- **Safe navigation is `&.`** (Ruby `h&.now`), replacing the Rust/TS `?.`.
  `TokenKind::QuestionDot` renamed to `AmpDot`; lexed from `&.`.
- **Collection query renames** — `is_empty`→`empty?`, `contains`→`include?`,
  `contains_key`→`key?`, `len`→`size` (compiler dispatch + lang_intrinsics +
  stdlib FFI decls + ~all `.rx` call sites + docs).
- **Block combinators are Ruby and work DIRECTLY on `Array`** (no `.iter`):
  `each` / `map` / `select` / `reject` / `reduce` / `find` / `index` /
  `all?` / `any?` / `partition` / `sort_by` / `select!`. These are **inlined**
  in `compiler/ruxen_core/src/mir/lower/closure_inline/` (loop codegen, no
  closure-env C ABI needed) — call them with a **trailing block**:
  `arr.map { |x| … }`, `arr.reduce(0) { |a, x| a + x }`.
  Renamed from Rust: `filter`→`select` (+ new `reject`), `fold`→`reduce`,
  `position`→`index`, `retain`→`select!`, `all`→`all?`, `any`→`any?`, `skip`→`drop`.
- **The entire iterator layer is removed.** `Array` no longer has
  `.iter` / `.into_iter` / `.iter_mut` / `.collect` / `.collect_vec` / `.to_vec`
  / `.as_slice` / `.enumerate` / `.from_iter`. `for x in arr` iterates the Array
  directly (the for-loop lowering reads `ruxen_vec_len`/`get` on the raw handle —
  `mir/lower/expr/for_loop.rs` — it never needed `into_iter`).
- **Direct conversions (Ruby):** `arr.to_set` → Set, `pairs.to_h` → Map (from
  `[(k,v)]`), `arr.to_a` (identity), `arr.zip(b)`, `arr.chain(b)`, `arr.take(n)`,
  `arr.drop(n)`. **`from_iter` removed** — use `to_set` / `to_h`. `Map.iter` /
  `Set.iter` renamed to **`to_a`** (Map's currently returns keys, see §3).
- **`["a"].sum` now rejected** (`E0700`, numeric guard on the Array `sum` arm) —
  Ruby's `["a"].sum` errors too.
- All e2e iter cases (119, 120, 127, 600–611), `stdlib_iterator`,
  `stdlib_array_negatives`, and the golden corpus are migrated and green.
  Obsolete cases removed: `610_iter_collect_map` (no `to_h`-via-collect),
  the `iter_flat_map` and `collect[Map]`-rejection canaries, `vec_from_iter_static`.

Key compiler files touched (for orientation):
`typeck/method_resolvers/{collections,iter,strings,mod}.rs`,
`codegen/lang_intrinsics.rs`, `mir/lower/closure_inline/{mod,filter}.rs`,
`lexer/{tokens,token,tests}.rs`, `library/std/{array,map,set}/src/lib.rx`.

---

## 3. Remaining tasks (to finish in the new session)

### 3a. Empty `||` optional in `do … end` closures  ← user's last request
- Today: `{ … }` (no `||`) already parses as a no-param closure, but
  `do … end` (no `||`) parses as a **block-value** expression — inconsistent.
  The user wants `do … end` (no `||`) to be a no-param closure too (drop the `||`).
- Site: `parser/expr/atoms.rs` ~line 200 (`TokenKind::Do` arm) routes to
  `parse_do_closure` only when `|` / `||` follows; otherwise `parse_do_block_expr`.
- **Ambiguity to resolve:** the bare `do … end` block-VALUE form is a real
  feature (`let v = do … end`) and is used by exactly 3 fixtures:
  `tests/release-e2e/cases/59_do_end_block.rx`,
  `src/ruxenc/tests/fixtures/ruxen/parser_do_end_block_expr.rx`,
  `compiler/ruxen_core/tests/fixtures/fmt/do_end_block_expression_preserved.rx`.
  Decision needed: (a) make `do … end` always a no-param closure and drop/migrate
  the block-value feature (Ruby has no standalone `do…end` value — it's always a
  block on a call), or (b) keep block-value and only treat `do…end` as a closure
  in argument/trailing-block position. (a) is more Ruby-faithful. There are only
  2 real `do ||` call sites (`library/std/bench` benches) to simplify to `do`.

### 3b. `Map#to_a` should return `[(k, v)]` pairs, not keys
- `Map.iter` was renamed to `to_a` but still maps to `ruxen_hash_iter` which
  returns **keys** (`Array[&K]`). Ruby `hash.to_a` returns **pairs**
  (`Array[[K, V]]`). Either add a pairs runtime helper (`ruxen_hash_entries`) and
  type `(Map(k,v), "to_a") => Array[(K,V)]`, or document keys-only as a v1 gap.
  `Map#keys` / `Map#values` already cover the key/value cases.

### 3c. `zip` / `chain` consume their argument (non-Ruby)
- The FFI param is `Int` (a moved Vec handle), so `a.chain(b)` *moves* `b`;
  Ruby doesn't consume. e2e 606/607 were rewritten to avoid reuse as a stopgap.
  Proper fix needs a borrow-able array arg, which is blocked on `class Array`
  having a real `[T]` param (see §3f) — or a `&`-handle ABI.

### 3d. `USize#to_string` missing runtime symbol
- `arr.count` / `arr.size` return `USize`; `n.to_string` then fails to link
  (`_USize_to_string`). Map `USize_to_string` → the same runtime as `Int_to_string`
  in `codegen/lang_intrinsics.rs`. (Tests pass because they stop before link;
  real programs hit it.)

### 3e. More Rust-isms to audit & convert (Ruby idioms)
- `Option#is_some` / `is_none` → Ruby predicate **`nil?`** / present check
  (`collections.rs` Option arms). `Result#is_ok` / `is_err` → keep or Ruby-ify.
- `x as Int` cast → Ruby uses `.to_i` / `.to_f` / `.to_s` (the `as` keyword is
  Rust). Big change; audit usage first.
- `each_with_index` (Ruby) — `enumerate` was removed; add `each_with_index { |x, i| }`
  as an inlined combinator if the `(i, x)` pattern is wanted back.
- Verify nothing else: `where` clauses, `unsafe`, `move` closures, `ref` patterns —
  these are spec-sanctioned "foreign concept" retentions (NG1); confirm against
  `ruby-naming.spec.md` §2 before touching.
- `library/std/iter/src/lib.rx` — the `Iterator` / `FromIterator` mixins (`next`,
  `from_iter`) are now vestigial (no `.iter`, no `from_iter`). Remove or repurpose.

### 3f. (Enabler, optional) extension generics on builtins
- A clean fix for extension generic-param scoping was found and reverted this
  session: in `resolve/items.rs::resolve_impl`, the target type resolves *before*
  the generic-param scope is pushed → `undefined type T` for `extension[T] Array[T]`.
  Fix = push the impl scope + register `generic_params` via
  `scopes.insert_type(name, DefKind::TypeParam{..})` **before** resolving
  `target_ty`/method bodies (mirror `resolve_class`, items.rs ~117-152). This,
  plus giving `class Array` a real `[T]`, would let combinators be pure-Ruxen
  `extension[T] Array[T]` methods and fix §3c borrow semantics. Not required for
  the current inlined approach.

### 3g. Docs sweep
- `docs/tutorial/*` and `docs/specs/*` still describe the iterator layer
  (`.iter`/`.collect`/`fold`/`filter`) in places. Sweep code fences to the direct
  Ruby API. The `ruby-naming.spec.md` already has §3.3 (lifetimes), §3.4a
  (predicates + `&.`), §3.10 (nil/`()`), §3.10a (string/char), §3.10b (ranges) —
  add a §3.x "Collections are Ruby `Enumerable`-shaped (no `.iter`)" section.

---

## 4. Reference

### Migration patterns (the throwaway scripts lived in `/tmp`; re-derive from these)
String/comment/interpolation-aware rewrite over `*.rx`, applied **per code
segment** (skip `"…"`, `'…'`, `#`-comments; descend into `#{…}` interpolation):
- query renames: `.is_empty`→`.empty?`, `.contains(`→`.include?(`,
  `.contains_key(`→`.key?(`, `\.len\b`→`.size`
- combinators: `.filter`→`.select`, `.fold`→`.reduce`, `.position`→`.index`,
  `.retain`→`.select!`, `.skip(`→`.drop(`, `.all(\s*(\{|do\b))`→`.all?\1`,
  `.any …`→`.any?`
- iter removal: `.into_iter.`/`.iter.`→`.`, standalone `\.iter\b(?!_)`→``,
  `\.collect_vec\b`→``, `\.to_vec\b`→``; then migrate `Set.from_iter(v)`→`v.to_set`,
  `Map.from_iter(pairs)`→`pairs.to_h`, `String.from_iter(v)`→`v.join("")`
- Dirs to cover (this session missed `src/ruxenc/tests` once): run over the whole
  repo — `find . -name '*.rx' -not -path './target/*'` — plus `~/.projects/rondo`.

### Focused test targets
```bash
cargo test -p ruxen_core --lib method_resolvers::golden        # dispatch parity
cargo test -p ruxen_core --test stdlib_iterator                # collection API
cargo test -p ruxen_core --test stdlib_array_negatives
cargo test -p ruxen_core --test typecheck_sample --test borrow_check_sample  # sample_program
cargo test -p ruxen_core --lib lexer::tests                    # syntax
ruxen test tests/<file>_test.rx                                # rondo
```

### Quick sanity script (after reinstall)
```ruxen
def main
  let a = [1, 2, 3, 4]
  let evens = a.select { |x| x % 2 == 0 }
  let total = a.reduce(0) { |acc, x| acc + x }
  let s = [1, 2, 2, 3].to_set
  let m: Map[Int, Int] = [1, 2].zip([10, 20]).to_h
  a.each { |x| puts "#{x}" }
  puts "#{evens.size} #{total} #{s.size} #{m.size}"
end
```
