# Ruxen Rust-ism audit: synthesis report

**Checkout:** `/tmp/claude-1000/-home-sheraz-Documents-ruxen-lang-ruxen/050e9991-43dd-44a4-930d-94a7a7074127/scratchpad/wt-audit` at `65aab1c`. That commit is on the feat/concurrency-adoption line, **193 commits ahead of `master` (`bebd7a3`)**. Nothing was edited and no agents were started.

**Direction:** Ruxen should match Ruby, static and compiled. The only thing it keeps from Rust is the ownership and borrow model.

## 1. Summary

**Raw findings per category** (214 items from the 6 auditors, before merging):

| Category | Specs | Tutorial and errors | Requirements | Decisions and parity | Grammar | Stdlib usage | Total |
|---|---|---|---|---|---|---|---|
| syntax | 15 | 15 | 17 | 20 | 24 | 13 | 104 |
| type-system | 5 | 7 | 5 | 6 | 6 | 7 | 36 |
| stdlib-naming | 7 | 4 | 8 | 4 | 1 | 5 | 29 |
| ownership-keep | 5 | 1 | 3 | 4 | 4 | 3 | 20 |
| semantics | 3 | 3 | 3 | 3 | 0 | 4 | 16 |
| tooling | 2 | 1 | 2 | 2 | 1 | 1 | 9 |

After merging duplicates there are **43 distinct Rust-isms** (section 2) and **9 ownership features to keep** (section 4).

**The canonical spec is a source of the problem.** `docs/specs/syntax/ruby-naming.spec.md` still prescribes many Rust forms:
- `let`, `match`, `Option`/`Result`/`Some`/`Ok`/`Err`, `&str` and `struct`/`enum`
- `as`, `Fn(...)`, `panic!`/`unwrap!`, `use`, and `.0` tuple access
- It also retires Ruby's own `Comparable` in favour of `Ord`/`PartialOrd`.

So the spec has to be rewritten first. Every other doc and the code take their direction from it.

**Top 10 Rust-isms by spread** (all appear in every area unless noted):

| # | Rust-ism | Code lines (stdlib / fixtures / examples) |
|---|---|---|
| 1 | `let` / `var` binding keywords | 546 / 1504 / 12 |
| 2 | `Option`/`Result` with `Some`/`Ok`/`Err` and a "no exceptions" model | about 260 / 715 / 3 |
| 3 | `match … Pat -> expr … end` | 32 / 314 / 0 (`case` is used on 2 lines in total) |
| 4 | `def main` boilerplate and `init` + `self.field` instead of `initialize`/`@ivar` | `def main` in 596 files; `self.x =` 186 stdlib lines |
| 5 | Rust stdlib naming (`len`, `is_empty`, `to_string`, `push_str`, `read_to_string`, `Duration.from_millis`, …) | about 100 stdlib definitions |
| 6 | Rust trait vocabulary (`Display`, `Debug`, `PartialEq`/`Eq`, `Ord`/`PartialOrd`, `Default`, `Into`, `Add`, `Hashable`) | 14 mixin declarations in core/fmt/hash |
| 7 | `struct` / `enum` / `newtype` / tuples with `.0` | 11 / 97 / 2 |
| 8 | Closure types (`Fn(A) -> R`, `FnOnce`, `FnVar`) and `{ \|\| }` | 66 / 47 / 0 |
| 9 | Async: `async def`, `.await`, `Future`/`Poll`/`Waker` | 26 / 81 / 0 |
| 10 | `as` casts plus sized integers (`USize`, `UInt8`, …) plus literal suffixes | 140 / 45 / 3 |

Runners-up: the `?` try operator, `use` imports (2 / 96 / 4), `mixin`/`extension`, and `where`/`some`/`any`.

## 2. Inventory (merged)

**Verdicts:** R = replace with the Ruby form · X = remove · D = needs owner decision (section 5).

**Where it lives:**
- **docs** means the compiler already rejects it or never had it, so only the docs are wrong.
- **parser** means the lexer or parser accepts it.
- **use** means the stdlib, fixtures or examples use it.

The Specs, Tutorial, Requirements and Decisions columns are doc line counts from each auditor's regex, so they are approximate. Code counts are stdlib / fixtures / examples.

| # | Rust-ism | Verdict | Ruby form | Lives | Specs | Tut | Req | Dec | Code |
|---|---|---|---|---|---|---|---|---|---|
| 1 | `let` binding (and `var` used as a binding) | R | `x = 1`; constants as `MAX = 3` | parser, use | 201 | 542 | 107 | 144 | 546/1504/12 |
| 2 | `Option[T]` / `Some(x)` | R/D | `T?` and `nil` | parser keyword, use | 245 (with #3) | 130 | 234 (with #3) | 140 | Some 27/124; `Option[` 51/26 |
| 3 | `Result`/`Ok`/`Err` as the error model, "no exceptions" | D | `raise` / `begin…rescue…ensure` | parser keywords, use | (in #2) | 177 | (in #2) | 80 | `Result[` 143/43/1; Ok/Err 41/328/1 |
| 4 | Postfix `?` try | R (after #3) | exceptions propagate on their own | parser, use | 9 | 14 | 11 | 38 | about 10/33/1 |
| 5 | `?.` taught as safe navigation | X | `&.` (already implemented) | docs | 3 | 3 | 0 | 0 | 0 |
| 6 | `match` with `->` arms (and `=>` arms in prompts) | R | `case … in … then` | parser, use | 33 | 154 | 34 | 23 | 32/314/0 |
| 7 | `if let` / `while let` | R | `if (x = opt)`, `while (x = q.pop)` | parser, use | (in #6) | 8 | 7 | 5 | 0/10/0 |
| 8 | `panic!` and the `!` macro convention | R | `raise "msg"`, `abort` | parser, use | 22 | 30 | 24 | 10 | 3/2/0 |
| 9 | `unwrap!`/`expect!`, `unwrap_or`, `ok_or`, `map_err`, `and_then` | R | `x \|\| d`, `fetch`, `x or raise` | MIR intrinsic, stdlib | (in #8) | about 47 | 22 | 17 | `unwrap!` 0/44/2; methods 11/51 |
| 10 | `struct`, plus `Point { x: 1 }` struct literals | R/D | `class`, `Struct.new`, `Data.define`, `Point.new(x: 1)` | parser, use (struct literal: docs only) | 88 | 96 | 61 | about 40 | 0/44/2 |
| 11 | `enum` with payload variants | D | `Data.define` per variant, symbols, `case/in` | parser, use | 60 | 49 | 15 | 13 | 11/27/0 |
| 12 | `newtype`, tuples with `.0`, `type X =` aliases | R | one-field class; arrays and `a, b = pair`; `X = Y` | parser, use | about 40 | about 30 | about 20 | 3 | `.N` 2/26; newtype 0/1; type 24/46 |
| 13 | Unit `()` / `Result[(), E]` | X | `nil` | docs (rejected by the parser) | 13 | n/a | about 20 | 3 | 0 |
| 14 | `[T; N]`, const generics, `const NAME =` | D | `MAX = 3`; maybe `StaticArray[T, N]` | parser | 75 | 75 | 29 | 2 | 0/5/0 |
| 15 | Rust trait vocabulary as mixins (Display, Debug, PartialEq, Ord, Default, Into, Add, Hashable) | R | `to_s`, `inspect`, `==`/`eql?`/`hash`, `include Comparable` with `<=>`, `def +` | stdlib/resolver, use | 190 | 106 | 176 | about 60 | 14 declarations, 8 fixture includes |
| 16 | `derive`, `@[attr]`, `#[attr]` | X | implicit include, body directives | docs (removed from the parser) | 25 | 20 | 114 | about 60 | 0 |
| 17 | Iterator layer (`iter`, `into_iter`, `collect`, `Iterator`/`FromIterator`, assoc `type Item`) | X | `Enumerable` (`each`, `map`, `to_a`) | docs (stdlib iter deleted); dead `mixin Iterable` | 35 | 30 | 165 | 21 | 0 (core `lib.rx:90` Iterable) |
| 18 | Rust String/Array/Hash names (`len`, `is_empty`, `to_string`, `push_str`, `to_upper`, `starts_with`, `trim`, `with_capacity`, `get_mut`, `insert`/`get`) | R | `size`, `empty?`, `to_s`, `<<`, `upcase`, `start_with?`, `strip`, `[]`/`[]=` | stdlib (Ruby twins often exist too), use | 60 | about 40 | 54 | 65 | about 40 definitions |
| 19 | Rust std io/fs/env/process/path/time/net shapes | R | `File.read`, `File.join`, `ARGV`, `ENV`, `system`, `Time.now`, `TCPServer` | stdlib, use | 200 | 230 | 120 | 4 | about 45 definitions |
| 20 | Rust concurrency surface (`JoinHandle`, `MutexGuard`, `lock()->Result`, atomics with `Ordering`, mpsc, `thread_local!`) | R | `Thread.new{}.value`, `Mutex#synchronize`, `Thread::Queue`, `Thread.current[:x]` | stdlib | 131 | 57 | 220 | 44 | std/sync |
| 21 | `println!`/`format!`/`dbg!`/`assert!` with `{}` formats | X | `puts`, `p`, `format("%d")` | docs only (not real macros) | about 10 | 0 | 50 | 2 | 0 |
| 22 | Rust format specs `#{x:?}`, `#{n:>5}`; Debug output shaped `Foo { x: 1 }` | R | `inspect`, `rjust`, `format` | lexer, use | 45 | 7 | about 5 | 4 | 0/8/1 |
| 23 | `&str` / `str` / `&static str` | X | `String`, `&String` | docs (E0730) | 30 | 34 | 71 | 62 | 0 |
| 24 | `Fn(A)->R`, `FnMut`/`FnOnce`/`FnVar`, `{ \|\| }`, bare `\|x\| e` | R/D | `&block` with `yield`, `Proc[(A) -> R]`, `->(x){}` | parser, use | 20 | 22 | 108 | 67 | Fn 66/24; `{\|\|}` 0/23 |
| 25 | Explicit receivers `(&self)`, `(self, …)`, `self: &Self` | X | `def m`, `def var m`, `def consume m` | docs (accepted only inside `lib` blocks) | 45 | 1 | 201 | 11 | 19 (`lib` only) |
| 26 | `use a.b.{C, D}` | R | `require` / `require_relative` (implemented, `module_loader.rs:357`) | parser, use | 45 | 58 | 43 | 7 | 2/96/4 |
| 27 | `as` numeric casts | R | `to_i`, `to_f`, `Integer()` | parser, use | about 10 | 11 | about 20 | 19 | 140/29/1 (about 67 are `.size as Int`) |
| 28 | Sized integer zoo and literal suffixes (`USize`, `UInt8`, `255u8`) | D | `Int`/`Float`; sized types for FFI only | lexer, stdlib | 83 | 55 | 198 | 55 | sized 67 std; suffixes 0/16/2 |
| 29 | `some`/`any` existentials, `[T: B]`, `where`, assoc types, GAT/HRTB/specialization | D | RBS-style `[T < B]`; drop GAT, HRTB, specialization | parser, use (HRTB/GAT: docs only) | 168 | 49 | 500+ | 60 | where 1/1; some/any 70/26 |
| 30 | `async def`, `.await`, `Future`/`Poll`/`Waker`, `block_on` | D | Fiber scheduler, `Async do … end`, `task.wait` | parser, use | 330 | 89 | 138 | 103 | 26 poll in std; 0/81 |
| 31 | `unsafe` blocks and modifiers | D | `lib` FFI only | parser | 36 | 55 | 21 | about 8 | 0/0/0 |
| 32 | `loop … end`, `continue`, `next` working only inside closures | R | `loop do … end`, `next` in all loops | parser | 12 | 3 | 7 | 4 | 0/2/0 |
| 33 | Brace-bodied `def f -> T { e }` | R | `def f = e` | parser, use | 14 | 24 | n/a | n/a | 31/0/2 |
| 34 | `mixin` (renamed trait) and `extension` (renamed impl); reopening a class is E0727 | D | `module` with `include`; reopen `class` | parser, use | (in #15) | 127 | n/a | 322 | mixin 26/30; ext 0/2 |
| 35 | `init`, `self.field`, typed field declarations, no `attr_*` | R | `initialize`, `@x`, `attr_reader` | parser, use | 123 | 137 | n/a | 84 | init 49/175/1; `self.x=` 186 std |
| 36 | Explicit `def main` | R | top-level statements (`implicit_main.rs` exists) | use | n/a | 223 | n/a | n/a | 596 files |
| 37 | Keywords that collide with Ruby method names (`match`, `where`, `any`, `loop`, `type`, `use`, `static`, `macro`, `package`) and `~=` instead of `=~` | R | free them up for `String#match`, `Relation#where`, `=~` | lexer | 1 | n/a | n/a | 13 | n/a |
| 38 | `fn`, `pub`, `impl`, `mod`, `::<>`, `Box<dyn>`, `Lazy<T>`, `Vec`, `Arc` in Ruxen examples | X | `def`, `include`, `Array`, `SharedSync` | docs | 25 | 14 | 27 | about 20 | 0 |
| 39 | `Map[K,V]` instead of `Hash` (including the ROADMAP "Map → Map" rename and the edition canary) | X | `Hash[K, V]` | docs | 8 | 4 | 37 | 15 | 0 |
| 40 | Retired literals still in docs: `None`, `r"…"`, `'\r'`, `'\u{…}'`, `..=` | X | `nil`, `'…'`, `?c`, `..` | docs | 18 | 2 | n/a | n/a | 0 |
| 41 | Editions, `no_std`, `cfg(feature)`, stability attributes, `r#ident`, user `macro_rules!` | D/X | semver, Bundler-style groups, `warn`/`deprecate` | docs (mostly unimplemented) | n/a | n/a | 245 | 27 | 0 |
| 42 | Rust prose and compiler internals in user docs ("matches Rust's…", `Ty::Infer`, stale `crates/` paths), plus Rust in `##` comments | R | Ruby and Ruxen wording | docs, comments | 45 | 28 | about 500 | n/a | 218 comment lines |
| 43 | Dead Rust plumbing: `TokenKind::Lifetime`/`Mut`, empty `derive_traits`, unparsed `macro`/`static`/`package` keywords; formatter prints `'a` | X | delete | parser, formatter | n/a | n/a | n/a | n/a | 15 sites |

## 3. Spot-check (read at 65aab1c)

**Confirmed** (cited lines read directly):
1. `let`, `var`, `match`, `loop`, `continue`, `where`, `as`, `use`, `package`, `static`, `const`, `Some`, `Ok`, `Err`, `Self`, `type`, `newtype`, `macro`, `struct`, `enum`, `unsafe`, `async`, `await`, `init`, `some`, `any`, `move`, `ref`, `consume` are all hard keywords in `lexer/token.rs:446-548`. `exclude`, `spawn` and `send` are **not** keywords.
2. `README.md:201-209` says "No exceptions" and teaches `find_user(42)?.name  # ?. safe navigation`. `tutorial/11-error-handling.md:125-134` teaches `?.` too. In the parser, `?.` means try followed by a method call; Ruby's `&.` is the real nil-safe operator.
3. `ruby-naming.spec.md:990-991` says user methods may not end in `!`. That contradicts `docs/errors/E1126.md:51` (`def shout!` works) and the lexer.
4. `ruby-naming.spec.md:596-598` retires `Comparable` in favour of `Ord`/`PartialOrd`. The stdlib still declares `mixin Comparable` (`core/src/lib.rx:17`) alongside `Ord`/`PartialOrd`/`Hash` (`:580-594`).
5. `option_result.spec.md:10` says "Ruxen's `Option` and `Result` mirror Rust's".
6. `ROADMAP.md:111` contains the broken rename "`Map[K,V]` → `Map[K,V]`".
7. `tier1_01_stdlib.md:457-458` says "Rust wins" and "Rust name; `?` is reserved for try". The file has 10 such lines.
8. `ruby-parity/language.md:34` says Ruby's exception model is "a deliberate design substitution… `Result` + `?` + `Drop` + `panic!`".
9. `_migration-agent-prompt.md:69-71` says "`Hash[K, V]`… is retired; use `Map[K, V]`". That is the opposite of the spec.
10. `docs/errors/E0003.md:19-25` teaches `r"path"`, which is retired (`lexer/tokens.rs:8-35`).
11. `ruby-naming.spec.md:734-738` says "`::` is not accepted", but `parser/expr/atoms.rs:512-514` accepts "Ruby's `::` namespace separator".
12. `ruby-naming.spec.md:890-892` says lifetimes use `'a`. The lexer turns `'a` into an unterminated raw string, error E0002 (`tokens.rs:13-35`).
13. `string.spec.md:9-17` says `&str` was removed (E0730), but `ruby-naming.spec.md:925` still lists `&str` and `:1089` types literals as `&static str`.
14. `parse_param` (`parser/methods.rs:349-378`) requires `name: Type`, so `(self, …)` and `(&self)` receivers do not parse in a regular `def`.
15. `case … in` also accepts the Rust `->` arm separator (`case_expr.rs:112-114`). `{}` parses as an empty closure, not an empty Hash (`atoms.rs:980-983`). `next` only works when `closure_depth > 0` (`atoms.rs:143`).
16. `BUILTIN_MACRO_NAMES = [panic, array, vec, map, hash, set]` (`parser/macros.rs:37`).
17. `T?` desugars to `Option[T]`; the comment calls it the Ruby-idiomatic form (`parser/types.rs:8-13`).
18. The stdlib has a brace-bodied `def` (`test/src/tester.rx:114`), `size -> USize` (`string.rx:19-22`), and `Option#unwrap`/`expect` defined without a bang (`option_result/src/lib.rx:64-69`).
19. `docs/superpowers/specs/2026-06-05-ruxen-gui-design.md:86` uses `fn`. `prompts/v2/01_actors…md:30-32` uses `=>` match arms.
20. No struct-literal expression node exists in `parser/ast.rs` (only `Pattern::Struct` at :259). So `Point { x: 1 }` in `ruby-naming:1284` and `user-modules:22` is unimplemented doc syntax.
21. Counts re-run: `let` in fixtures = 1112 lines; `use` = 102 lines; `Some(`/`Ok(`/`Err(` = 586 lines; `def main` in 592 fixture files; `case` = 2 lines; `initialize` = 0; `init` = 49 stdlib lines.

**Corrected or dropped:**
- **"A bare `x = 1` does not declare a variable"** (tutorial and grammar audits). Wrong at 65aab1c. `resolve/control_flow.rs:30-79` (`resolve_bare_assignment_binding`) turns an undeclared `x = v` into a **mutable** `let` binding. The probes that said otherwise ran old binaries built from `master` (`bebd7a3`), where this function does not exist. So the claim is true on master and false on this branch. Dropping `let` is therefore cheaper than the audits implied.
- **"`unwrap!`/`expect!` in the README would fail to resolve"** (specs audit). Wrong: both are MIR intrinsics (`mir/lower/expr/field_access.rs:28-29`) and are used on 44 fixture lines. The item stays as a Rust-ism, but the examples do work.
- **"The legacy `'a` lifetime path is still accepted"** (decisions audit). Wrong: the lexer never emits `TokenKind::Lifetime`. The parser arms at `types.rs:119,165,545` are dead code (item 43).
- **Citation fixes, substance unchanged:**
  - The ROADMAP P0.6 row is at `ROADMAP.md:26`, not :28 (:28 is P0.8).
  - "`puts` (legacy…)" is at `tier1_01_stdlib.md:107`, not :105.
  - The tutorial audit labels the checkout `bebd7a3`; it is `65aab1c`.
- **Cast counts conflict:** grammar reported 137/12/1 and stdlib-usage reported 140/29/1. Item 27 uses the stdlib-usage figure; the gap comes from regex scope.

## 4. Keep (ownership)

Each feature below is part of the borrow checker or the no-GC model, with the smallest surface that still looks like Ruby.

1. **Borrows `&x` / `&var x`, types `&T` / `&var T`.** `&mut` is already spelled `&var`. Later, consider auto-borrowing at call sites, so `Todo.where("x", [1])` works without `&"…"`, and keep the sigils on signatures.
2. **Receiver modes `def var m` / `def consume m`.** These are already Ruby-shaped. Delete every doc that shows an explicit `self` parameter instead (item 25).
3. **Mutability.** Ruby locals are always reassignable, and bare assignment already declares a mutable local. So `var` should survive only in `&var` and `def var`, not as a binding keyword.
4. **`move` closure capture** (`move { |x| … }`). Keep, or infer it from escape analysis.
5. **Lifetimes as bare lowercase `[a]` and `&a T`.** Keep as the escape hatch, with elision as the default. Fix `ruby-naming §3.10a`. Drop the HRTB `for[a]` and GAT designs, which exist only in docs.
6. **`Copy` / `Clone` / `Drop`.** Clone maps to Ruby's `dup`/`clone`. `def drop` is the destructor. Ruby's `ensure` and block-scoped resources (`File.open do … end`) should lower onto drop.
7. **`Send` / `Sync`.** Keep the check, and choose one opt-out spelling:
   - Today the parser accepts `include !Send` and `unsafe include Send`.
   - The spec's `exclude Send` is **not implemented**.
   - The docs also disagree on whether these markers are manual or inferred.

   Recommendation: inferred by default, with a single `exclude Send` directive.
8. **`Box` / `Shared` / `SharedSync`.** Keep the concepts. Whether to rename them is a decision (section 5).
9. **`ref` / `ref var` patterns.** Keep, or replace them with default binding modes.

## 5. Needs owner decision

1. **Error model.**
   - **Problem:** `Result` + `?` + `panic!` is used on about 590 code lines, and every fallible stdlib API returns `Result` (143 stdlib signatures). `begin`/`rescue`/`ensure` do not exist, and `raise` exists only as a call name.
   - **Options:** typed checked exceptions that lower to `Result` internally, or keep `Result` and rename the parts users see.
   - **Blocks:** items 3, 4, 8 and 9.
2. **Absence.** Should `T?` + `nil` be the only spelling, with `Some` no longer a keyword and no `Some(...)` wrapping? The parser already calls `T?` the Ruby form. Pattern matching then becomes `in nil` / `in v`.
3. **Static annotations.** Keep `name: Type` and `-> T`? Ruby has nothing here; Crystal writes `x : T` and RBS writes `(T) -> R`. My recommendation is to keep the current form, since it is the least churn and is not Rust-specific.
4. **Generics.**
   - Bounds: `[T: B]` versus the RBS form `[T < B]`.
   - `where` clauses: drop?
   - `some` / `any`: is a plain mixin-typed parameter enough?
   - Associated types: needed only by the retired iterator layer and by `Future::Output`.
5. **Value types.**
   - `Copy` currently requires `struct` (E0603).
   - Options: keep `struct`; or `class` with a `value` directive; or `Data.define`.
   - Enum ADTs: keep `enum` with Ruby construction, or use per-variant `Data.define`.
6. **`mixin` / `extension` versus Ruby `module` / reopening a class.** Reopening is currently an error (E0727). These keywords affect 322 lines of decision docs and every mixin in the stdlib.
7. **Async.** Keep colored `async def` / `.await` (81 fixture lines, stdlib future), or move to the Fiber-scheduler model with `Async do … end`. This is the largest semantic change.
8. **Numerics.**
   - Should `size`/`length` return `Int`? That alone removes about 67 casts.
   - Should sized integers be limited to FFI?
   - Replace `as` with `to_i8`-style methods? Drop literal suffixes?
9. **FFI and unsafe.**
   - canvas and rondo depend on `lib "…"` blocks with `def x as "sym"` (451 stdlib lines).
   - My recommendation is to keep `lib` (it already resembles Ruby FFI's `attach_function`).
   - Open questions: does `unsafe … end` survive, should `layout c` become `layout :c`, and should raw pointers be restricted to `lib`?
10. **Const generics and `[T; N]`.** Drop them, or keep as `StaticArray[T, N]`.
11. **Names of ownership types:** `Box`/`Shared`/`SharedSync` and `Send`/`Sync` (for example `Shareable`, after Ruby's Ractor).
12. **Scope of `no_std`, editions, `cfg` features and stability attributes** (245 lines in requirements docs, mostly unimplemented).
13. **`match` as an alias of `case/in`, or removed outright.** Removing it frees `String#match`. Also: should `case/in` stop accepting `->`?

## 6. Proposed cleanup phases

**(a) Docs only: two passes.**
- **a1, no decision needed.** Remove forms the compiler already rejects or never had: items 5, 13, 16, 17, 21, 23, 25, 38, 39, 40 and 43 (doc side), the `fn`/`=>` examples, stale `crates/` paths, Rust internals in `docs/errors/*.md`, the `?.` text, and `@[link]` described as live (`tier3_00:105`).
  - Delete `_migration-agent-prompt.md` and tutorial chapter 23.
  - Fix the internal spec contradictions found in section 3.
  - Mark `docs/dev/gui-stack-v1-issues.md`, `superpowers/plans/*` and `prompts/v1/*` as historical rather than rewriting them.
  - **Size:** about 120 files and about 1.5k changed lines. A few days of agent work, splittable by directory.
  - **Dependencies:** none.
- **a2, after the section 5 decisions.** Rewrite `ruby-naming.spec.md` first (1732 lines, the source of truth). Then the README, the 33 tutorial chapters, about 40 error docs, the stdlib specs and the requirements tier docs, for items 1-4, 6-12, 15, 18-20, 22, 24, 26-36.
  - **Size:** large, about 300 files and about 10k lines.
  - **Dependencies:** section 5. The spec rewrite should land first.

**(b) Parser: add the Ruby forms first, then deprecate the Rust forms.**
- **b1, add missing Ruby forms:**
  - `begin`/`rescue`/`ensure`/`raise` (if the decision is exceptions)
  - `initialize`, `attr_reader`/`attr_accessor`
  - `loop do … end`, `next` in all loops, `->(x) { }`
  - `{}` as an empty Hash, `exclude Send`
  - array and hash patterns `in [a, *rest]`
  - Bring bare assignment to `master` (it exists only on this branch).

  Each one needs an e2e fixture and a negative test, and must work the same in the compiler, REPL, LSP, IDE and formatter.
- **b2, deprecation warnings with fix-its** (new registered codes and `docs/errors/` files), for the Rust forms in order of usage:
  - the low-usage ones first: `loop`/`continue` (2), `if let`/`while let` (10), `const` (1), `newtype` (1), `[T;N]` (4), suffixes (18), brace `def` (33), `{||}` (23), `panic!` (5), `->` in `case/in`
  - then `use` (102), `match` (346), `let` (about 1.4k)
  - Remove the dead plumbing (item 43) and stop reserving `macro`, `static`, `package` and possibly `where`/`type`.
- **b3:** turn the warnings into hard errors once (c) is done.
- **Size:** medium-large, about 15-20 PRs.
- **Dependencies:** b1 depends on the section 5 decisions. b2 follows b1. b3 follows (c).

**(d) Formatter output.** This runs before (c), because it becomes the codemod.
- Make `ruxen fmt` (`ruxen_core::formatter`, the only formatter) print the canonical Ruby forms:
  - `@x` instead of `self.x`
  - no `let`
  - `case/in` instead of `match`
  - `require` instead of `use`
  - endless `def`
  - no `'a` (fix `format_expr.rs:690,694`)
- Add a `ruxen fix --ruby` mode (or `fmt --migrate`) built on the same formatter, for the mechanical rewrites.
- **Size:** medium.
- **Dependencies:** b1, plus golden-test updates.

**(c) Rewrite stdlib, fixtures, examples and the GUI/web projects.**
- Run the (d) codemod over the 90 stdlib files, 593 fixtures (plus their `.out` files where output changes, such as Debug output moving from `Foo { x: 1 }` to `#<Foo x=1>`), and the 9 examples.
- Then canvas, quiver and rondo, which were **not audited**. Check rondo's W-series workarounds first.
- By hand:
  - Stdlib renames (items 15, 18-20): delete the Rust-named duplicates where a Ruby twin already exists (`starts_with`/`start_with?`, `to_string`/`to_s`, `trim`/`strip`).
  - Rewrite the 145 Rust-flavoured stdlib `##`/`#` comments.
- Stdlib changes reach the `.rx` projects only after `ruxen upgrade --from-source`.
- **Size:** large. About 2k `let` lines, 346 `match`, 586 `Some`/`Ok`/`Err` and about 100 stdlib definitions; the error-model change touches every fallible API.
- **Dependencies:** (b1) and (d). Run it per module, with `cargo test --workspace` and `tests/release-e2e/run.sh` green after each step.

**Order:** a1 (now, in parallel) → section 5 decisions → spec rewrite (start of a2) → b1 → d → c → b2/b3 → rest of a2.