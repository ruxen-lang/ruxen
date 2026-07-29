# QUEUE.md — the batched Ruby-parity work queue

Execution queue for the Ruby-parity push. **One subagent takes one batch.** Every
batch below is self-contained: it names its goal, its exact method/feature list,
its target files, its blocker status, and the matrix rows to tick on completion.
A subagent running a batch does **not** need to read the big matrices
(`core.md` / `language.md` / `stdlib.md` / `gems-db.md` / `gems-web.md`); it needs
this file plus the local `CLAUDE.md` of the module it edits.

Source of the ordering: `HANDOFF.md` (rules, bugs, enablers) plus the five
matrices. Ordering rule: unblocked before blocked, then value to a Ruby developer
building a real app, then low risk before high risk.

---

## 0. Read this before running any batch

### The two hard rules

1. **`.rx`-first.** Implement in `library/std/**/*.rx` by extending the stdlib
   packages. Touch the Rust compiler only for a genuinely inexpressible
   primitive or a missing *generic mechanism*, never a per-feature special-case.
2. **No derive macros.** The compile-time surface is mixins (`include`) plus
   `extension`, not derive/macros.

### Authoring rules that cost hours to rediscover

| Rule | Detail |
|---|---|
| **`String` bodies MAY dispatch** | No generic `T`, so build results char-by-char over `self.chars` (`Array[Char]`) with interpolation: `var result = ""` then `result = result + "#{c}"`. |
| **Never use `self` as a `String` value** | Inside `class String`, `self` is the FFI shell (`&String` / `Class{"String"}`). It does not compose with the `String + String` concat route (`type mismatch: expected String, found String`), and `"#{self}"` interpolates the buffer pointer (garbage). `self.clone` / `self.to_string` return the shell type too. Go through `self.chars`. |
| **Generic collections must NOT dispatch on `T`** | `Array` / `Set` / `Hash` method bodies lower ONCE with `T` abstract (only `each` / `push` / closure-call are available). Anything needing `to_s(T)`, `T` ordering, or `T` equality is blocked on per-`T` mono (enabler E1). Pure slot-moving and delegating bodies are fine today. |
| **Key-addressed `Hash` / `Set` ops are fine** | `key?` / `get` / `insert` / `include?` hash inside the C runtime, so they do not need `.rx`-level `T` equality. `Hash#invert` and `Hash#value?` DO need value equality, so they are blocked. |
| **`alias` on a generic builtin head link-fails (Q58)** | Use a real delegating method (`def has_key?(k) = self.key?(k)`), not `alias`, until E2 lands. |
| **Operators are just `.rx` methods now** | A non-comparison operator on a `String` / `Array` / `Hash` / `Set` receiver routes to the receiver's `.rx` `def OP` when one exists (builtin-head operator route). Define `def *`, `def +`, `def ^` and they dispatch. |
| **Default args work on builtin heads** | `def ljust(w, pad = " ")` resolves on `String` / `Array` / `Hash` / `Set`. |
| **Default-arg fill only covers the ALL-omitted call (scalar heads)** | Verified 2026-07-29 on `class Int`: `1234.truncate` against `truncate(ndigits = 0)` works, but `2.pow(10)` against `pow(exp, mod = 0)` type-checks and then fails CODEGEN (`mismatched argument count ... got 2, expected 3`). A defaulted parameter followed by a `&block` parameter is worse: `1.step(4)` against `step(limit, by = 1, &block)` fails in TYPECK (`no method step on type Int`). Workaround that works today: ship the two arities as two real `def`s (same name, different arity) — overload resolution by arity is fine, including next to an FFI-declared `to_s`. |
| **`Option` RETURNS from a scalar method-home are unusable** | Verified 2026-07-29: a `def probe -> Option[Int]` on `class Int` types as `Int` at the call site (`no field present? on type Int`), even with an explicit `let o: Option[Int]` annotation. This is why `Integer#nonzero?` is MISSING rather than shipped. |
| **Index (`[]`) never routes to a scalar head's `.rx` `def []`** | Verified 2026-07-29: with `def [](i)` on `class Int`, `n[0]` types as `<error>`. `Integer#[]` (bit reference) therefore ships as `bit(i)`. The builtin-head OPERATOR route does not cover indexing, nor the scalar heads. |
| **A bare `self` body does not satisfy a `-> Char` return** | Inside `class Char`, `def chr -> Char; self; end` is `type mismatch: expected Char, found Char` (the FFI-shell / value split, same family as the `String` rule above). Round-trip it: `let n: Int = self as Int` then `n as Char`. Casts on a scalar `self` (`self as Int`, `n as Char`, `b as Int`) all work. |
| **`<=>` as an OPERATOR does not reach an `.rx` `def <=>`** | Comparison operators keep their dedicated route: `c1 <=> c2` on `Char` is E0707 even with `def <=>(other: Char)` defined. The METHOD spelling `c1.<=>(c2)` dispatches fine. |
| **Scalar `self` on the LEFT of arithmetic link-fails** | Verified 2026-07-29: inside `class Int`, `self % 2` and `self + 1` emit calls to nonexistent `Int_%` / `Int_+`. **Workaround that works today: rebox first,** `let n = 0 + self` (or `var a = 0 + self`), then use `n` freely. `self` on the RIGHT of a primitive-left operand (`0 - self`, `2 * self`) and all comparisons (`self < 0`, `self == 0`) work as-is. This is why the Integer batches are READY-NOW. Enabler E3 removes the reboxing. |
| **`Float` `.rx` method bodies are BROKEN** | Verified 2026-07-29: inside `class Float`, `self` arrives truncated to an integer (`2.5.probe` sees `2`, and a computed Float receiver sees `0`). Returning a Float constant is fine, so it is a receiver-ABI bug, same family as the known "Float closure-arg passes as 0". **Do not write Float `.rx` methods until B01 lands.** |
| **Char literal as a receiver mis-parses** | `?7.digit?` compiles but produces no output; `let c = ?7; c.digit?` works. Bind to a variable. |

### Build / test loop

- Toolchain build: `cargo build --release --bin ruxen` (binary at `target/release/ruxen`).
- **Fast `.rx` iteration without rebuilding:** `RUXEN_STDLIB_PATH="$PWD/library/std" ./target/release/ruxen compile f.rx -o f && ./f`.
- ⚠️ **`RUXEN_STDLIB_PATH` is NOT in the compile-cache key, so this loop silently replays stale objects.** The key covers the source hash, compiler version, target, the project's `runtime/*.c`, and a toolchain fingerprint (the binary's mtime and size), and **nothing about the stdlib the binary was pointed at** (verified in `src/ruxenc/src/compile.rs:200` and `cache/hash.rs`). Editing `library/std` and recompiling an **unchanged** `.rx` therefore hits the cache and reuses the old object, so your stdlib edit appears to do nothing, or an old result poses as a fresh pass.
  - **Operating rule: wipe `<checkout>/target/ruxen/incremental` before any verification run that follows a stdlib edit.**
  - **Treat an `Up to date` line in compile output as a red flag, not good news**, whenever you have just changed `library/std`.
  - The embedded-stdlib path is fine: `cargo build` changes the binary, which changes the toolchain fingerprint, which misses the cache. Only the `RUXEN_STDLIB_PATH` loop is exposed.
  - Tracked as **B75**. Until it lands, this is a manual discipline and it is easy to forget.
- Narrow test loop for additive `.rx` work: `cargo test -p ruxen_core --lib` (~0.5s). That is the whole gate for a purely additive stdlib batch (global rule 42).
- Full gate (compiler changes only, once per phase): `RUXEN_WORKSPACE="$PWD" PHASES=compile bash tests/release-e2e/run.sh`, ~5 to 6 min, cache to `tmp/test-cache/`.
- **Do not edit `library/std` or run `cargo build` while a `RUXEN_WORKSPACE` e2e run is in flight.**

### Definition of done for a batch

1. The `.rx` methods (or the compiler mechanism) land.
2. One or more pinned cases under `tests/release-e2e/cases/NNN_*.rx` plus `expected/NNN_*.out`.
3. The matrix rows named in the batch are updated in the same commit. **The line
   numbers in each batch's "Matrix rows" line are a hint recorded on 2026-07-29,
   not an address.** Any batch that edits a matrix shifts them, and B06 rewrites
   them wholesale. Locate rows by searching for the method name, never by
   jumping to the cited line.
4. A `CHANGELOG.md` entry under `[Unreleased]`.
5. Reviewed under CLAUDE.md rule 44.

### Pin-number allocation (avoid collisions between parallel batches)

Highest existing case is `989`. **`990` to `999` are reserved** for the in-flight
per-`T` mono and Q58 work. Every batch owns a block of ten:

> **Batch `Bnn` owns pins `1000 + 10*(nn-1)` through `+9`.**
> B01 → 1000-1009, B02 → 1010-1019, B07 → 1060-1069, B20 → 1190-1199, and so on.

**The formula is not universal. It has one exception, and any batch that splits
into sub-batches needs the same treatment:**

| Exception | Allocation |
|---|---|
| **B35** (split into six sub-batches, §6) | The formula's block `1340-1349` is **abandoned, do not use it**. B35a → 1900-1909, B35b → 1910-1919, B35c → 1920-1929, B35d → 1930-1939, B35e → 1940-1949, B35f → 1950-1959. |

A ten-pin block cannot hold a batch that splits. When a batch splits, take a
fresh run of blocks from **1900 upward**, give each sub-batch ten, abandon the
formula block rather than reusing it (a half-used block invites a later
collision), and record the reallocation in this table. Blocks 1900-1959 are now
taken; the next split batch starts at 1960.

---

## 1. Enablers (compiler mechanisms that unblock the BLOCKED batches)

| id | Enabler | Where | Unblocks |
|---|---|---|---|
| **E1** | **Per-`T` monomorphization of generic-collection method bodies.** Extend the generic-CLASS mono (`mir/lower/monomorphize.rs`, which already clones user generic-class bodies per concrete receiver as `Box__mono__String_eq`) to emit `Array[Int]_join` / `Array[Int]_min` per concrete `T` at the call site, rewriting `T` to the concrete type in the body (the `trait_default.rs::rewrite_self_in_func` pattern applied to the element param). **In progress (agent `per-t-mono`).** | `compiler/ruxen_core/src/mir/lower/{monomorphize.rs,trait_default.rs}` | B20, B21, B23 (partly), and the element-typed half of B07/B15/B16 noted per batch |
| **E2** | **Q58: `alias` to a method on a generic builtin head link-fails.** `mir/lower/mod.rs::select_method_symbol_name` compares `parent_def.name == class_name`, but the alias target's parent is the BARE class while `class_name` may carry the generic suffix. Strip `class_name`'s `[...]` there (`canonical_method_name` already strips). **LANDED** (`7b1c1907`, merged `42008fa3`). | `compiler/ruxen_core/src/mir/lower/mod.rs` | B22 (and lets several other batches collapse delegating methods back to one-line aliases) |
| **E3** | **Scalar method-home `self` does not reach the primitive arithmetic floor.** A scalar `self` is typed nominally as `Class[Int]`, so `self + 1` lowers to a method call `Int_+` that does not exist. Make a scalar-class `self` flow into the primitive `+`/`-`/`*`/`/`/`%` floor. | `typeck/infer/ops.rs`, `mir/lower/expr/binops.rs` | Not a hard blocker (rebox with `let n = 0 + self`), but it removes the workaround from B12/B13/B14 and is a prerequisite for clean `Float` bodies |
| **E4** | **Float receiver ABI on `.rx` method homes and closure args.** A `Float` receiver reaching an `.rx` body is read from an integer slot (truncates, or reads 0 for a computed value). Same family as the closure-arg bug. Delivered by **B01**. | codegen receiver coercion (`primitive_ffi_receiver_ty` neighbourhood), closure arg marshalling | B23 (all `Float` methods), the `Array[Float]` half of B20 |
| **E5** | **Symbol type (`:sym`).** A compile-time-closed primitive: a 64-bit id into a static intern table, no runtime interning. **Designed: [`design/B29-symbols.md`](design/B29-symbols.md).** Delivered by **B29**. Note `&:sym` already works without it, and `Hash[Symbol, V]` additionally needs **B73**. | lexer, parser, typeck, plus a `library/std/symbol` `.rx` surface | Hash symbol shorthand `{a: 1}`, `%i[]`, `String#to_sym`, hash patterns, and the literal-symbol folding in B34 |
| **E6** | **Field-walk synthesis substrate**: the compiler synthesizes three ordinary typed methods from a class's declared fields (`field_names`, `accept[V]`, `from_fields[R]`); every feature on top is an `.rx` visitor, so the compiler learns one mechanism and zero features. **Designed: [`design/B33-codegen-substrate.md`](design/B33-codegen-substrate.md).** Note the design places synthesis at the **end of typeck as HIR**, not in `mir/lower/derive.rs`, specifically to avoid adding a second typeck↔MIR lockstep. Delivered by **B33** (substrate) and **B55** (the `Model` consumer). | a new synthesis pass at the typeck boundary; `implicit_includes/mod.rs` for gating and diagnostics | B33, B34, B44, B55 to B60, B63, B64 (the whole ORM and serializer surface) |
| **E7** | **Splat params `*args` / `**kwargs`.** Delivered by **B27**. | parser, typeck | `format`/`sprintf` free fns, `puts a, b`, variadic builders, `Set[a, b, c]` literal |
| **E8** | **Range as a first-class Enumerable object.** Range is an iteration construct today (`Range#map` returns empty). Needs a real Range surface that can `include Enumerable`. Delivered by **B24**. | `library/std` (new `range` package) + typeck/MIR range lowering | B24, and Range rows across `core.md` |
| **E9** | **Toolchain codegen step** (a `ruxen build` preprocessor emitting `.rx` from a non-`.rx` input). Delivered first by **B65** (the `.rhtml` template compiler). | `src/ruxen_cli` | B65, route-helper generation, I18n tables, asset manifests |
| **E10** | **Mutating-operator route** (`<<` and friends taking `&var self`). Delivered by **B04**. | typeck operator route + MIR | `String#<<`, `Array#<<`, `Set#<<`, `StringIO#<<`, `CSV#<<` |

---

## 2. Known bugs (each sized as its own batch)

These are correctness bugs that silently produce wrong answers or crash. They
erode trust in the whole surface, so they lead the queue.

**Order in this section is priority order, not id order.** Batch ids are stable
identifiers (they own pin blocks that running agents already depend on), so a
bug found later keeps its next-free id and is simply placed at its rank. B75
ranks first and B71 second, both found after B01 to B05.

### B75 — The compile cache omits stdlib identity, so a stdlib edit can be silently ignored
- **Goal:** editing `library/std` invalidates the compiled objects that depend on it, without the caller having to remember an env var or a manual wipe.
- **Cause (verified in the source, 2026-07-29):** the cache key is built in `src/ruxenc/src/compile.rs:200` as `backend`, `opt`, `release`, `runtime_c` (path plus content hash of the **project's** `runtime/*.c`), `link_args`, and `toolchain` (the running binary's mtime and size), combined in `cache/hash.rs::content_hash` with the source hash, compiler version, and target. **`RUXEN_STDLIB_PATH` appears nowhere in it**, and nowhere in `src/ruxenc/src/` at all.
- **Consequence:** with an unchanged binary pointed at a workspace stdlib (the "fast `.rx` iteration" loop this file recommends in §0), editing `library/std` and recompiling an unchanged `.rx` produces an identical key and **replays the stale object**. The stdlib edit appears to do nothing, or an old result poses as a fresh pass.
- **Why the embedded path is already safe, and why that matters:** `cargo build` changes the binary, so the `toolchain` fingerprint changes and the cache misses. The comment on that field says it was added **precisely** for this class of problem (Q24: "changing an embedded-stdlib `.rx`/`.c` leaves `compiler_version` unchanged"). So the design intent is already correct; the `RUXEN_STDLIB_PATH` case is simply an unhandled second door into the same room.
- **Fix, with a precedent to copy:** fold the resolved `RUXEN_STDLIB_PATH` plus a content hash of its sources into `flags`, exactly as `runtime_c_fingerprint` already does for the project's C sources ("path + content hash ... so editing a project's `runtime/*.c` invalidates the cached binary even when no `.rx` source changed"). The mechanism, the placement, and the justifying comment all already exist; this extends them to one more input.
- **Why it ranks first, above every other bug in this section:** it does not produce a wrong answer, it produces **an unreliable answer about whether anything is wrong**. A stale replay can hide a real defect or fabricate a passing test, so every other batch's evidence is only as trustworthy as this. It is also not a parallel-agent artifact: a single developer editing `library/std` hits it alone, on the loop the docs recommend, which makes it strictly more important than a coordination problem would be. It is the stdlib-side twin of the "CLI caches by source hash" warning already in the root `CLAUDE.md`.
- **The anomaly that exposed it:** a bogus `mid used here after move` on `965_comparable_spaceship` mid-sweep, which passed on re-run with no code change. That is consistent with stale replay, so **the defect it appeared to show was never real**. Worth remembering as the signature: a failure that vanishes on re-run without an edit is a cache symptom, not a flake to shrug at.
- **One narrow cross-agent path, for completeness:** the compile cache is project-local (`compile.rs:878` walks up for `Cargo.toml` or `ruxen.toml` and uses `<that dir>/target/ruxen`), so worktrees already have private caches. The only sharing risk is running with a working directory **outside** any checkout, where the walk finds no manifest and falls back to a relative `./target/ruxen`. Note that `XDG_CACHE_HOME` (`cache/store.rs:234`) resolves only the **global** cache behind `ruxen clean --global` and has no effect on the compile path, so it is not a workaround for any of this.
- **Files:** `src/ruxenc/src/compile.rs` (the `flags` construction), `src/ruxenc/src/cache/hash.rs`.
- **Status:** NEEDS-COMPILER-FEATURE (toolchain).
- **Size:** small, and well precedented. The risk is not the change, it is choosing the fingerprint's granularity: hashing every stdlib source on every compile costs time on the hot path, so consider mtime plus size first and a content hash only when that is ambiguous.
- **Pins:** 1740-1749. A pin cannot easily prove a cache miss from inside the e2e harness, so this most likely wants a Rust integration test under `src/ruxenc/tests/` asserting that two compiles of identical source with different `RUXEN_STDLIB_PATH` contents produce different cache keys.

### B77 — Static dispatch through an abstract type fails (`T.static`, `Self.static`)
- **Goal:** a generic body can call a class method on its type parameter, and a mixin default can call a static on `Self`.
- **Two distinct failures, one missing mechanism (verified 2026-07-29):**
  1. **In a generic class:** `class Relation[T: FromRow]` whose body calls `T.from_row(row)` → `codegen: no runtime symbol for '?T::from_row' (mangled '?T1817_from_row')`.
  2. **In a mixin default:** `mixin Model` with `def self.all` calling `Self.from_row` → `Failed to define function 'User_all': Verifier(... "mismatched argument count ...: got 2, expected 1")`. This is very likely the Cranelift arity mismatch reported previously as a defaulted-parameter symptom.
- **The contrast that characterises it:** a mixin default calling a required **instance** method on `Self` **works** (verified in the same program). It is specifically **statics** on an abstract type that fail.
- **Why it matters far beyond a corner case:** it is the reason `gems-db.md`'s Phase 2 (generic CRUD inherited from `mixin Model`, whose bodies call `Self.table_name`/`Self.columns`/`Self.from_row`) **cannot be built as written**, and it is what forced the folio design to a repository shape carrying metadata as values. See [`design/B54-folio-orm.md`](design/B54-folio-orm.md) §1.4 and §2.
- **Workaround that folio uses meanwhile:** pass the decoder as a **closure value** into the generic function, which works today.
- **Files:** `compiler/ruxen_core/src/mir/lower/{monomorphize.rs,trait_default.rs}`, plus the static-call mangling in `mir/lower/mod.rs`.
- **Status:** NEEDS-COMPILER-FEATURE (enabler). Landing it would let the active-record facade be generic and would simplify B56.
- **Pins:** 1760-1769. Pin both failing shapes and the working instance-method contrast.

### B78 — `where` is a reserved keyword, so it cannot be a method name
- **Goal:** `def where(...)` and `.where(...)` are legal, so the single most recognisable line in the Ruby ORM surface (`User.where(...)`) can be spelled.
- **Repro (verified 2026-07-29):** `def where(frag: &String) -> Relation[T]` → `error: expected expression, found RParen` at the parameter list and `error: expected identifier, found Where` at the call site. Renaming to `filter` compiles immediately, so nothing else is wrong.
- **Cause:** `where` is taken by generic bounds (`where T: Add`).
- **Fix, with a precedent:** make it a **contextual keyword** in method-name and method-call position, exactly as `.then` was handled (pin 973, per `language.md` Backlog A item 16).
- **Severity:** low as a defect, high as a papercut. Every ORM doc, every downstream app, and every Ruby developer's muscle memory uses `where`. Shipping `filter` and renaming later means a breaking change across the whole folio surface.
- **Files:** `compiler/ruxen_core/src/parser/{methods.rs,expr/calls.rs}`.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Pins:** 1770-1779.

### B01 — Float receiver ABI: `self` truncates, closure arg passes as 0
- **Goal:** a `Float` value keeps its bits when it crosses into an `.rx` method body or a closure parameter.
- **Repros (both verified at baseline):**
  - `[2.5].map { |v| v + 0.5 }` yields `0` (affects `Array.map`, not just `Kernel#then`).
  - A `def probe -> String` on `class Float` whose body is `let x = 0.0 + self; "#{x}"` prints `2` for receiver `2.5`, and `0` for a computed receiver such as `0.0 - 2.5`. Returning a Float *constant* from the same method is correct, so the defect is on the receiver/argument inbound path, not the return path.
- **Files:** codegen receiver coercion (`primitive_ffi_receiver_ty` and the `coerce_value` call-site zero-extension neighbourhood), closure argument marshalling in `mir/lower`.
- **Status:** NEEDS-COMPILER-FEATURE (E4).
- **Size:** one self-contained defect; expect it to be a register-class/width mistake, not a redesign.
- **Matrix rows:** `core.md` Float table (200-224) and the Operator-reality note (44-68); `HANDOFF.md` "Known PRE-EXISTING bugs".
- **Pins:** 1000-1009.

### B71 — Non-scalar parameter default drops to the null sentinel on a paren-less call
- **Goal:** `def show(prefix: String = "v=")` called as `p.show` uses the declared default instead of silently substituting the null sentinel.
- **Repro (verified at baseline 2026-07-29, on a plain non-generic class, so not a Q58 regression):**

  ```
  class P
    n: Int
    def init(@n: Int) end
    def show(prefix: String = "v=") -> String
      "#{prefix}#{self.n}"
    end
  end
  # p.show      => "3"     <- WRONG, the default vanished
  # p.show()    => "v=3"   correct
  # p.show("v=")=> "v=3"   correct
  ```

- **Cause:** `compiler/ruxen_core/src/mir/lower/mod.rs::default_expr_to_sentinel` maps only `nil`, Int, Bool, and Float. Every other default expression falls through to the null sentinel. Int, Bool, and Float defaults therefore land correctly, which is exactly what makes this easy to miss.
- **Trigger surface, narrowed by probe (this is better news than it first looks):**
  - **Paren-less calls only.** `p.show()` with empty parens is correct, so **adding parens is a complete workaround** for any call site.
  - **The default-fill path itself is fine.** `"ab".ljust(6)` correctly pads with the declared `pad = " "` default on a builtin head (probed). So the already-shipped `ljust`/`rjust`/`center` from pins 988/989 are **not** broken: they take a required argument, so they are always called with parens.
  - The exposed surface is therefore **methods whose arguments are all defaulted**, called Ruby-style without parens. That is a real and growing set: `chomp(suffix = "\n")` (B09), `join(sep = ", ")` (B20), `strftime(fmt = "%Y-%m-%d")` and `iso8601` (B35d), `inspect`, and every `to_s(base = 10)`-shaped method.
- **Why it ranks second, above B02 to B05 and below B01:** it is a silent wrong answer on the *idiomatic* Ruby call form, and the root `CLAUDE.md` forbids a silent wrong answer for any user-callable surface outright. It sits below B01 because B01 is the same class of silent numeric corruption with **no** workaround and it blocks a whole batch (B23), while this one has a total workaround (parens). It sits above B02 because a segfault announces itself and this does not.
- **Fix shape, and why this is probably an enabler rather than a bug fix:** extending the scalar whitelist to include String would make the repro pass and **move the same silent failure to the next unlisted type** (a const, an enum variant, `Array.new`, a computed expression such as `" " * 2`). That is the per-feature-special-case anti-pattern the root `CLAUDE.md` names. The real fix is to **lower an arbitrary default expression at the call site** the way an explicitly-passed argument is lowered, which is a reusable generic mechanism and would also make non-literal defaults work for the first time. Classified **NEEDS-COMPILER-FEATURE** on that basis. If the general lowering proves too large, a whitelist extension is acceptable *only* if the fall-through case becomes a hard compile error rather than a null sentinel, so the failure stops being silent.
- **Files:** `compiler/ruxen_core/src/mir/lower/mod.rs` (`default_expr_to_sentinel` and the paren-less call path).
- **Status:** NEEDS-COMPILER-FEATURE.
- **Size:** one defect if whitelisted, one small generic mechanism if done properly. Budget for the latter.
- **Pins:** 1700-1709. Pin **both** call forms (paren-less and parens) and at least one non-literal default, so a future whitelist regression is caught.
- **Matrix rows:** `language.md` §4 (the `def f(x = 1)` default-args row, currently DONE and now known to be conditionally wrong); `HANDOFF.md` known-bugs list (lines 161-165), where it is already recorded.

### B76 — A defaulted parameter destroys a trailing `do…end` block
- **Goal:** `def each(sep: String = ", ", &block: …)` called as `each do |x| … end` receives its block.
- **Repro (verified 2026-07-29):**

  ```
  def show(prefix: String = "v=", &block: Fn[(Int) -> nil]) -> nil
    yield(1)
  end
  show    do |n| … end   # ruxen panic: yield called without a block in `show`
  show()  do |n| … end   # SAME panic, so parens do not help
  show("p=") do |n| … end # works: prefix=[p=], n=1
  ```

- **Rule:** whenever a defaulted parameter is **actually defaulted**, the trailing block is lost. Supplying every defaulted parameter explicitly preserves it. Without a default in the signature (`show("v=") do … end`) it works.
- **Distinct from B71:** B71 is paren-less-only and concerns the default's *value*; this loses the **block**, with or without parens. Likely a shared call-path root, so fixing them together may be cheaper, but they are separately observable and separately pinnable.
- **Severity:** high. `def each(sep = ", ", &block)` is an everyday Ruby shape, and the whole Enumerable and iteration surface is built on defaulted-plus-block signatures. It panics rather than corrupting, which is the one mercy.
- **Blocks B27:** the splat and keyword work extends the same call path, so building on this defect would make it far harder to isolate. See [`design/B27-splat-kwargs.md`](design/B27-splat-kwargs.md) §7.
- **Files:** `compiler/ruxen_core/src/mir/lower/mod.rs` (default fill and the block-argument slot), `typeck/infer/expr.rs`.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Pins:** 1750-1759. Pin all four call forms from the repro, since three of them differ only in whether the default was supplied.

### B02 — `do…end` block plus bare-variable `puts` segfaults
- **Goal:** `arr.each { |x| puts(x) }` stops crashing.
- **Repro:** `arr.each { |x| puts(x) }` segfaults; `arr.each { |n| puts "#{n}" }` (interpolation) works. So the bare-variable `puts` path inside a block passes the wrong thing.
- **Files:** `typeck/infer/expr.rs` (the `puts` universal-`to_s` conversion added by pin 956) plus `mir/lower` block-parameter binding.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Size:** one defect.
- **Matrix rows:** `language.md` §12 (353-371, the `puts` row); `HANDOFF.md` bug list.
- **Pins:** 1010-1019.

### B03 — `String[i]` / `[i, n]` / `[range]` indexing returns null
- **Goal:** Ruby's `String#[]` and `#slice` family work.
- **Scope:** `"abc"[1]` → `"b"`, `"abc"[1, 2]` → `"bc"`, `"abc"[1..2]` → `"bc"`, negative indices, out-of-range → `nil`. Ruby returns a 1-char **String**, not a Char, so decide and document the return type (recommend `Option[String]` for the 1-arg form to keep `nil` honest).
- **Files:** a `ruxen_string_char_at` / `ruxen_string_slice` in `library/std/string/runtime/string.c`, the `.rx` declarations in `library/std/string/src/string.rx`, and an Index-path arm in `compiler/ruxen_core/src/mir/lower/expr/index.rs`.
- **Status:** NEEDS-RUNTIME-C (plus one small lowering arm).
- **Size:** one feature, 3 to 4 methods.
- **Matrix rows:** `core.md` String table rows `[]`/`slice`/`at`/`byteslice` (87-89) and the Operator-reality row for `String [i]` (57).
- **Pins:** 1020-1029.

### B04 — `<<` mutation semantics for `String` and `Array`
- **Goal:** `s << "x"` and `a << 1` mutate the receiver instead of being silent no-ops.
- **Scope:** the operator route currently passes the receiver by value for non-comparison operators, so a mutating operator has nowhere to write. Add a `&var self` operator route (E10) and define `def <<` on `String` (delegating to `push_str`), `Array` (to `push`), and `Set` (to `insert`). Chainability (`a << 1 << 2`) is Ruby behaviour, so return the receiver.
- **Files:** `typeck/infer/ops.rs`, `mir/lower/expr/binops.rs`, then `library/std/{string/src/string.rx,array/src/lib.rx,set/src/lib.rx}`.
- **Status:** NEEDS-COMPILER-FEATURE (E10).
- **Size:** one mechanism plus three one-line methods.
- **Matrix rows:** `core.md` Operator-reality rows (56, 59) plus String `<<`/`concat` (85), Array `<<` (240), Set `add`/`<<` (396).
- **Pins:** 1030-1039.

### B05 — Q56: async `block_on` client I/O no-ops
- **Goal:** close the one live correctness bug tracked in `docs/dev/gui-stack-v1-issues.md`.
- **Scope:** client-side I/O driven under `block_on` does nothing. Reproduce from the Q56 entry, find whether the reactor never registers the fd or the waker never fires, fix, and pin an async client round-trip case.
- **Files:** `library/std/future`, `library/std/async_net`, the reactor in the async runtime; possibly `async_lowering`.
- **Status:** NEEDS-COMPILER-FEATURE (runtime/reactor).
- **Size:** one defect, but investigation-heavy; give it a full session.
- **Matrix rows:** `docs/dev/gui-stack-v1-issues.md` Q56; `stdlib.md` §9 net/http (291-328) if the fix changes the client surface.
- **Pins:** 1040-1049.

### B72 — `Hash[k] = v` subscript-assign is a silent no-op — **DONE**
- **Landed:** index-assign now routes to the receiver's `[]=` for a BUILTIN head
  too, not only a nominal one — the write-side twin of the builtin-head operator
  route, sharing `binops.rs`'s head predicate (`is_builtin_op_head`) and method
  probe (`builtin_head_has_op_method`) so the read/binop side and the write side
  cannot drift. `Hash[k] = v` works because `class Hash[K, V]` gained
  `alias []= insert`; nothing names `Hash` in the Rust, so any head that
  resolves `[]=` gets the route. `alias`, not a delegating `def`, is
  load-bearing: it emits the same `ruxen_hash_insert` call `insert` does, so
  drop elaboration applies the key/value ownership TRANSFER already recorded for
  that symbol. A separate `Hash_[]=` body is classified as BORROWING its args by
  the `Hash_` runtime-prefix rule in `runtime_abi.rs`, and the caller then
  double-frees the key at scope exit (observed, SIGTRAP). The machine floor's
  index-assign arm now returns a lowering ERROR for a receiver it has no store
  path for, instead of silently discarding the write. Pins `1710` (String key)
  and `1711` (Int key, kept below 4096 so it does not depend on B73), each
  asserting `size` + `key?` + a subscript read-back plus overwrite semantics.
- **Two shapes that used to no-op silently now REFUSE to compile** (they never
  produced a correct store, so this is loud-instead-of-wrong, not a lost
  feature). Both are still open gaps, neither is in scope for B72, and each
  names itself in the refusal:
  - **Parallel assignment through an index** (`h["a"], h["b"] = 1, 2`).
    `MultiAssign` reaches the machine floor with an ALREADY-LOWERED value, so it
    has no HIR argument to hand a synthetic `[]=` call and cannot take the
    desugar. Assign each index in its own statement. Fixing it means giving
    `store_value_into_target` a way to call a method with a pre-lowered arg, and
    it has to settle the evaluation-order semantics of a parallel write into a
    collection, so it wants its own batch.
  - **Fixed-size array `[T; N]` index-assign with a COMPUTED index**
    (`var a: [Int; 3] = …; a[i] = 9`). The floor stores a `[T; N]` slot only for
    a literal index. Use an `Array` for a computed index.
- **Adjacent PRE-EXISTING gap found while verifying the generality (needs its
  own id).** The new route reaches any builtin head whose `.rx` CLASS method
  home resolves `[]=`, which is how the whole stdlib is written (`class String`,
  `class Array[T]`, `class Hash[K, V]`, `class Set[T]` all register their
  methods under the BARE head name). It does NOT reach an `extension Array` /
  `extension Hash` `def []=`: an `extension` on a GENERIC builtin head registers
  its methods under an inference-suffixed parent spelling — `extension Array`
  with `def []=` emits `Array[?T3522]_[]=`, verified via `--emit=mir` — which no
  bare-base lookup can match. That is the same `Hash[K, V]`-versus-`Hash`
  name-mismatch family as Q58, one layer further out (an unresolved `?T` rather
  than a concrete arg), and it is NOT a regression: the pre-existing
  builtin-head OPERATOR probe misses it identically, so `extension String` with
  `def *` is equally unreachable today. Whoever picks it up should fix it where
  the extension's parent name is FORMED, not by widening the lookups.
- **Goal:** `h["a"] = 42` actually stores.
- **Repro (verified 2026-07-29):**

  ```
  var h = Hash[String, Int].new
  h["a"] = 42
  # size=0, key?("a")=false      <- the store vanished
  h.insert("b", 7)
  # size=1, key?("b")=true, and h["b"] reads back 7
  ```

- **Shape:** the subscript **read** works and `insert` works, so only the subscript **assign** path is dead. `Array[i] = x` is fine (`a[0] = 9` verified), so this is `Hash`-specific.
- **Severity:** high. `h[k] = v` is the single most common `Hash` operation in Ruby code, it fails silently rather than crashing, and it is currently documented as working.
- **Refutes a matrix row:** `core.md`'s Operator-reality table lists "`Array[i]`, `Array[i]=`, `Hash[k]`, `Hash[k]=` **DONE** ... `h["a"]` all correct". The `Hash[k]=` half of that row is wrong. **The matrix files are owned by other agents right now, so this correction is recorded here and must be applied to `core.md` by whoever next holds it.**
- **Files:** `compiler/ruxen_core/src/mir/lower/expr/assign.rs` (the index-assign arm) and the `Hash` route in `binops.rs`/`index.rs`.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Pins:** 1710-1719. Pin `[]=` followed by `size`, `key?`, and a subscript read, so a partial fix cannot pass.

### B73 — The `Hash` key-kind protocol is a memory-probing heuristic
- **Goal:** a `Hash` key's comparison and hashing are decided by its **type**, not by guessing from its bit pattern.
- **Cause:** `library/std/hash/runtime/hash.c` stores every key in an `int64_t` and resolves a tristate `string_keys` flag **on the first insert** via `ruxen_hash_looks_like_string`, which **dereferences the key value as a pointer** and scans up to 256 bytes for a NUL.
- **Three defects, one root (all verified 2026-07-29):**
  1. **`Hash[Int, V]` SIGSEGVs for any key ≥ 4096.** `h.insert(7, 42)` and `h.insert(4095, 42)` work; `h.insert(5000, 42)` and `h.insert(100000, 42)` die with exit 139. The heuristic short-circuits below `0x1000` and dereferences address 5000 otherwise. Integer keys are ordinary (ids, ports, status codes), so this is a crash on plausible input.
  2. **User types cannot be `Hash` keys.** A `struct` or `class` key is compared by object identity: `h.insert(SymI.new(3), 99)` then `h.key?(SymI.new(3))` is `false`. The `Hashable` mixin's `hash_code` and the structural `==` derive are **never consulted** by the C table.
  3. The key kind is decided by the **first** insert, so a table's behaviour depends on insertion order.
- **Fix shape:** pass the key kind down from the typed `.rx` side (the call site knows whether `K` is `String`, an integer-like primitive, or a user type) instead of inferring it in C, and route user types through `Hashable#hash_code` plus the `==` derive. That is a real protocol, not a heuristic, and it is what makes `Hash[UserType, V]` possible at all.
- **Severity:** the segfault alone makes this high. Note it also **blocks B29c**: `Hash[Symbol, V]` cannot work until it lands (see [`design/B29-symbols.md`](design/B29-symbols.md) §1.4).
- **Files:** `library/std/hash/runtime/hash.c`, plus the `.rx` key-kind plumbing in `library/std/map/src/lib.rx` and `library/std/set/src/lib.rx`.
- **Status:** NEEDS-RUNTIME-C.
- **Size:** one focused session. It is contained, but it is load-bearing for every `Hash` and `Set` in the workspace, so pin generously.
- **Pins:** 1720-1729. Pin an integer key above and below 4096, a user-type key, and an insertion-order swap.

### B74 — `class` structural equality returns false; `struct` works
- **Goal:** `C.new(1) == C.new(1)` is `true` for a class, as it already is for a struct.
- **Repro (verified 2026-07-29):** `struct P { x: Int; y: Int }` gives `P.new(1,2) == P.new(1,2)` → `true`, but `class C { x: Int }` gives `C.new(1) == C.new(1)` → `false`, with **or without** an explicit `include PartialEq`, and likewise for a `String` field.
- **Why it went unnoticed:** pin `133_implicit_partial_eq.rx` covers a **`struct`**. There is no class-equality pin.
- **Refutes a matrix claim:** `core.md`'s Kernel row says "structural `==` for user types via `include PartialEq`/derive" and `language.md` §9 lists auto equality as DONE (cases 133/207/139). Both are true for `struct` and false for `class`. **Record the narrowing in `core.md` and `language.md` when those files are free.**
- **Files:** `compiler/ruxen_core/src/mir/lower/derive.rs` (`synthesize_struct_eq` and the class path), `resolve/symbols.rs::ty_has_derive_trait`.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Pins:** 1730-1739. Pin a class with an `Int` field, a class with a `String` field, and an inherited field.

### B79 — `_1` / `it` are invisible inside a string interpolation
- **Goal:** `xs.each { puts "#{_1}" }` resolves.
- **Repro (verified 2026-07-29):** `nums.each { puts "#{_1}" }` → `error: undefined variable '_1' (at 4:23)`; same for `it`. Outside interpolation both resolve fine (pin `943`).
- **Cause:** `resolve/implicit_block_params.rs::Scan::expr` has no `InterpolatedString` arm, and this is STRUCTURAL rather than a missing match arm: `StringPart::Expr` (`lexer/token.rs:71`) holds UNPARSED `tokens`, so the scan physically cannot descend into it. A fix means parsing interpolation parts before the implicit-param scan, or re-running the scan after.
- **Severity:** high. `language.md` §5 marks `_1` and `it` DONE, and per `HANDOFF.md` interpolation is the WORKING way to print from a block (bare-variable `puts` in a block segfaults), so this is exactly the spelling a Ruby developer reaches for first. The DONE row fails on its most idiomatic use.
- **Related:** the same unparsed-tokens design causes the interpolation re-lexer trap in HANDOFF's probing rules (invalid syntax inside `"#{…}"` compiles and emits an empty string). A fix here should consider fixing both.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Pins:** 1790-1799. Pin `_1` and `it` inside interpolation, both alone and combined with `next`.

### B80 — A synthesized `it` shadows an in-scope local named `it`
- **Goal:** match Ruby 3.4, where an existing local named `it` WINS over the implicit block parameter.
- **Repro (verified 2026-07-29):** `let it = 99; xs.map { it + 0 }` prints `1 2 3`; CRuby prints `99 99 99`.
- **Cause:** `resolve/control_flow.rs:269` calls `implicit_param_names` with no scope lookup, so the synthesized `it` binds unconditionally.
- **Severity:** medium. Wrong value, silently. Narrow trigger (a local actually named `it`), which is why it ranks below B79.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Pins:** 1800-1809.

### B81 — `E1125` is emitted but unregistered and undocumented
- **Goal:** satisfy the root `CLAUDE.md` convention that every emitted code is in `diagnostics::codes::REGISTRY` and has `docs/errors/E####.md`.
- **Repro:** `E1125` is referenced at `hir/nodes.rs:241`, `parser/ast.rs:444`, `mir/lower/expr/control.rs:181`, but is absent from `REGISTRY` and has no `docs/errors/E1125.md`. Note all three references are COMMENTS describing a rejection, not `error_with_code` call sites, so first establish whether `E1125` is reachable at all: if the rejection it documents fires under a different code, the fix is to correct the comments; if it is genuinely emitted somewhere, register and document it.
- **Why no test caught it:** no test enforces this direction. The `codes.rs` tests only iterate `REGISTRY` outward (no duplicates, well-formed codes, non-empty titles); nothing scans the source for emitted codes and asserts each is registered and documented. That missing test is the durable fix and is worth more than the one-off correction.
- **Also:** `resolve/control_flow.rs:404`'s doc comment says "No enclosing block → E1120" while `:422` emits `E1124`; E1120 is the unrelated `alias`-target diagnostic (`codes.rs:473`). Fix the comment in the same pass.
- **Severity:** medium, and cheap.
- **Status:** READY-NOW (no enabler needed).
- **Pins:** 1810-1819, plus the registry test.

### B82 — `drops.rs` rebuilds instructions without updating `inst_spans`, misattributing debug line info
- **Goal:** debug line info stays aligned, or degrades to absent, never to WRONG.
- **Cause:** `mir/lower/drops.rs:1126-1149` does `std::mem::take(&mut block.instructions)` and re-pushes with interleaved `MirInst::Assign` drop-flag updates via raw `push`, never touching `inst_spans` (`grep inst_spans drops.rs` → zero hits). The LLVM emitter reads spans POSITIONALLY (`codegen/llvm/emit/mod.rs:402`, `mir_block.span_at(inst_idx)`), so from the first inserted flag update onward every `!dbg` is attributed to an earlier unrelated instruction.
- **Why it matters:** `mir/nodes.rs:290-298` documents the invariant that a mismatch "degrades to no line info, never a panic or a wrong-program bug". That holds for a SHORTER span vector and is false for a MISALIGNED one. Silently wrong line numbers are worse for a debugger user than absent ones, so the documented invariant is itself wrong and should be corrected alongside the fix.
- **Coverage gap:** neither `llvm_debug_info.rs` test (hand-built `MirProgram`) nor `mir_debug_spans.rs` exercises a function that needs a drop flag.
- **Severity:** medium. LLVM is not the default backend and DWARF is debug-only, which caps the blast radius.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Pins:** 1820-1829. Pin a function with a conditional drop, asserting span alignment after `insert_drops`.

### B83 — Audit every `Return | Break` match arm for a missing `Next`
- **Goal:** find and fix every scanner/walker that handles `Return`/`Break` and silently omits `Next`, which carries the identical `Option<Box<Expr>>` payload.
- **Why this is a batch and not a one-off:** the omission has now been confirmed in one place and fixed (`resolve/implicit_block_params.rs::Scan::expr`, pin `1780`, where it made `xs.map { next _1 * 100 }` fail to resolve). At least two more sites have the same shape: `resolve/yield_scan.rs::find_first_yield_args_in_expr:141` and `typeck/mod.rs::resolve_expr_types:449`. `Next` was added by the Block Tier-2 batch AFTER these walkers were written, so the pattern is likely repeated wherever a walker predates it.
- **Method:** `grep -rn "Return(.*) | Break(" compiler/ruxen_core/src` and check each hit. For each site, construct a REPRO first, then fix.
- **⚠️ A candidate patch exists but is UNVERIFIED and was deliberately NOT committed:** `tmp/patches/next-arm-siblings.patch` (uncommitted work of unknown provenance, found in the working tree and reverted). It adds the `Next` arm to both sites above with plausible comments. It builds and the narrow gate stays at 655 passed / 0 failed, but **its stated repro does not reproduce**: `next yield v` cannot be exercised because `yield` inside a nested `do…end` block fails identically WITHOUT `next` (`error: function 'f' expects 0 arguments, got 1`), and `next` outside a block is `undefined variable 'next'`. So the patch may well be correct by symmetry, but nothing demonstrates it fixes a reachable bug, and this repo requires a pinned case per fix. Do not commit it until a real repro exists.
- **Blocked-by note:** finding a repro for the `yield_scan` site probably requires `yield` inside a nested block to work first, which is a separate gap worth filing if confirmed.
- **Severity:** medium. One confirmed instance was high and is fixed; the rest are latent.
- **Status:** READY-NOW (audit), then NEEDS-COMPILER-FEATURE per site.
- **Pins:** 1830-1839, one per site actually proven broken.

---

## 3. READY-NOW batches (`.rx` only, no enabler needed)

### B06 — Matrix re-verification sweep (no production code)
- **Goal:** stop subagents from re-implementing rows that already landed. Several matrix rows are stale.
- **Known-stale examples found while building this queue:** `core.md` marks `Comparable#clamp` MISSING (456-469) but `library/std/core/src/lib.rx:50` defines it; `Enumerable` rows mark `filter` / `detect` / `inject` / `find_all` / `none?` / `one?` MISSING (418-453) but all six exist in `core/src/lib.rx`; the Integer and Float tables still mark `**` as "MISSING (BUG), lowered to `*`" although the Operator-reality table and pin 952 record it DONE.
- **Method:** for every `PARTIAL`/`MISSING` row in `core.md`, compile a one-line probe with `RUXEN_STDLIB_PATH` against the workspace stdlib, and correct the row. Batch the probes (one `.rx` file per class) rather than one file per method.
- **Files:** `docs/ruby-parity/core.md`, `language.md`, `stdlib.md` (rows only, no `library/std` edits).
- **Status:** READY-NOW.
- **Size:** one session; it is mechanical.
- **Matrix rows:** all of `core.md`; the Enumerable table (418-453) and both scalar tables (161-224) first.
- **Pins:** 1050-1059 (only for rows promoted to DONE that lack a pin).

### B07 — Enumerable breadth I: closure-only combinators
- **Goal:** one implementation lands on `Array` + `Hash` + `Set` simultaneously. Highest leverage `.rx` batch in the queue.
- **Methods** (all expressible with `each` + a closure + `push`, so no `T` dispatch): `flat_map`, `collect_concat`, `filter_map`, `take_while`, `drop_while`, `each_slice(n)`, `each_cons(n)`, `each_with_object(memo)`, `first`, `first(n)`, `take(n)`, `drop(n)`, `count` (no-arg), `cycle(n)`, `reverse_each`, `collect`, `find_index` (predicate form), `to_a`/`entries`.
- **Files:** `library/std/core/src/lib.rx` (`mixin Enumerable[T]`).
- **Status:** **LANDED.** 16 of the 18 shipped as `mixin Enumerable[T]` defaults. Two did not, and neither is blocked on E1, so they are recorded here rather than moved to B20:
  - `first(n)` — arity overloading does not reach a builtin head. With both `first` and `first(n)` defined, `a.first(2)` binds the ZERO-arg body and dies in the Cranelift verifier (`mismatched argument count ... got 2, expected 1`). Reproduced on `Set`, which homes no `first` of its own, so it is the overload and not the `Array` FFI decl. `take(n)` carries the behavior.
  - `collect` — the name is intercepted for EVERY receiver by the iterator `collect[Target]` arm (`typeck/infer/expr.rs`, `mir/lower/expr/method_call.rs`) before method resolution runs. Ruby's `collect` needs that interception narrowed first.
  - `to_a` is intentionally not a mixin default: `Array` / `Set` / `Hash` each home their own with a head-specific element type. `entries` ships instead.
  - Required one compiler fix: `flat_map` was on the unimplemented-Vec blocklist in `codegen/lang_intrinsics.rs`, which made the emitted `Array_flat_map` body unreachable. Removed, exactly as `map` / `select` / `reduce` were when they migrated.
- **Size:** 18 methods; large but uniform. Split at `each_with_object` if it runs long.
- **Matrix rows:** `core.md` Enumerable (418-453), plus the Array rows it satisfies (257-286) and the Hash/Set rows (334-343, 411).
- **Pins:** 1060-1064 (1065-1069 unused).

### B08 — String query and inspection
- **Goal:** the read-only half of Ruby's `String` surface.
- **Methods:** `bytesize`, `count(str)`, `index(sub, offset = 0)`, `rindex(sub)`, `casecmp(other)`, `casecmp?(other)`, `eql?(other)`, `start_with?` multi-arg, `end_with?` multi-arg, `each_char`, `each_line`, `each_byte`, `codepoints`, `empty?` (verify), `length`/`size` (verify), `chars`/`lines`/`bytes` (verify).
- **Files:** `library/std/string/src/string.rx`.
- **Status:** READY-NOW (char-by-char over `self.chars`; remember the `self`-as-String trap).
- **Size:** 13 new methods.
- **Matrix rows:** `core.md` String table 82-146 (rows `bytesize`, `count`, `index`, `rindex`, `each_*`, `codepoints`, `casecmp`, `eql?`).
- **Pins:** 1070-1079.

### B09 — String transformation: case, trim, pad, edges
- **Goal:** the everyday string-shaping methods.
- **Methods:** `swapcase`, `chomp(suffix = "\n")`, `chop`, `center(width, pad = " ")`, `succ`/`next`, `delete_prefix`, `delete_suffix`, `prepend`, `squeeze(set = "")`, `delete(set)`, `tr(from, to)`, `tr_s(from, to)`, `upcase!`/`downcase!`/`capitalize!`/`swapcase!`/`strip!` (bang variants, decide on `Option[String]` Ruby semantics or drop them and record why).
- **Files:** `library/std/string/src/string.rx`.
- **Status:** READY-NOW (`center` works now that default args fill on builtin heads).
- **Size:** 12 non-bang methods plus a decision on the 5 bang variants.
- **Matrix rows:** `core.md` String rows 99-146 (`chomp`/`chop`, `swapcase`, `center`, `tr`, `delete`, `squeeze`, `prepend`, `delete_prefix`/`delete_suffix`, `succ`, the bang rows at 98/103).
- **Pins:** 1080-1089.

### B10 — String search and replace
- **Goal:** `sub`/`gsub` and the partition family, string-argument forms.
- **Methods:** `sub(pattern, replacement)`, `sub!`, `gsub(pattern, replacement)` (align with the existing `replace(from,to)`, which is really `gsub`; also rename-or-alias so Ruby's `replace(other)` whole-string overwrite is available under its own name), `gsub!`, `partition(sep)`, `rpartition(sep)`, `slice(i, n)` (coordinate with B03 for the operator form), `split(sep, limit)` (limit arg over the existing `splitn`), `squeeze` if not taken by B09, `start_with?` regex form (defer to B11).
- **Files:** `library/std/string/src/string.rx`.
- **Status:** READY-NOW for the string-argument forms. The block form of `gsub` and the regex forms belong to B11.
- **Size:** 9 methods plus the `replace` naming fix (a real name collision, call it out in the CHANGELOG).
- **Matrix rows:** `core.md` String rows 90-95 (`sub`, `gsub`, `replace`, `split`) and 132 (`partition`/`rpartition`).
- **Pins:** 1090-1099.

### B11 — String conversion and the Regex bridge
- **Goal:** Ruby's conversion methods and `String`-side regex spellings.
- **Methods:** `to_i` / `to_i(base)` / `to_f` (lenient Ruby semantics returning `0` on garbage, keeping the `Result`-returning `parse_int`/`parse_float` alongside), `hex`, `oct`, `ord`, `chr`, `to_str`, `dup`, `scan(re)`, `match(re)`, `match?(re)`, `=~`/`~=` returning an index, `gsub(re, repl)`, `sub(re, repl)`, `split(re)`.
- **Files:** `library/std/string/src/string.rx` delegating to `library/std/regex/src/lib.rx`.
- **Status:** READY-NOW for the regex delegations and the lenient parsers. **Flag:** `ord`/`chr` need a `Char` to `Int` conversion; if `0 + self` does not type on `Char`, add a `ruxen_char_ord` shim to `library/std/string/runtime/string.c` and mark that sub-row NEEDS-RUNTIME-C.
- **Size:** 15 methods.
- **Matrix rows:** `core.md` String rows 118-127 (`scan`, `match`, `to_i`/`to_f`, `ord`/`chr`, `hex`/`oct`, `succ`) and the Regexp table row for `String#scan`/`#gsub(/re/)` (590).
- **Pins:** 1100-1109.

### B12 — Integer predicates, sign, rounding — **DONE**
- **Landed:** all 18 methods; `nonzero?` is the one MISSING row (an `Option` return from a scalar method-home types as its payload at the call site). Pins `1110` (predicates/sign) and `1111` (rounding, incl. the negative-`ndigits` forms). Expected output diffed against CRuby 3.3.
- **Goal:** the day-one-ergonomics half of `Integer`.
- **Methods:** `abs`, `magnitude`, `even?`, `odd?`, `zero?`, `nonzero?`, `positive?`, `negative?`, `integer?`, `succ`, `next`, `pred`, `clamp(min, max)`, `between?(min, max)`, `ceil`, `floor`, `round`, `truncate` (the last four are identity or precision-argument forms on Integer).
- **Files:** `library/std/scalar/src/lib.rx` (`class Int`).
- **Status:** READY-NOW **with the reboxing rule**: open any body that needs arithmetic with `let n = 0 + self`, then operate on `n`. Verified working (`even?` via reboxing returns `true`/`false` correctly).
- **Size:** 18 methods, all one-liners.
- **Matrix rows:** `core.md` Integer table 161-197.
- **Pins:** 1110-1119.

### B13 — Integer arithmetic and number theory — **DONE**
- **Landed:** `divmod`/`fdiv`/`div`/`modulo`/`remainder`/`gcd`/`lcm`/`gcdlcm`/`pow(n)`/`pow(n, mod)`/`bit_length`/`digits`/`Int.sqrt` (class method) and `~` (verified). `Integer#[]` ships as `bit(i)`: an index expression on a scalar head never reaches an `.rx` `def []`. Pins `1120` (division/modulo sign conventions) and `1121` (number theory). Expected output diffed against CRuby 3.3.
- **Goal:** the arithmetic family Ruby developers reach for.
- **Methods:** `divmod`, `fdiv`, `div`, `modulo`, `remainder`, `gcd`, `lcm`, `gcdlcm`, `pow(n)`, `pow(n, mod)` (modular exponentiation), `bit_length`, `digits(base = 10)`, `sqrt` (as `Integer.sqrt`, a class method), `[]` (bit reference), `~` verification.
- **Files:** `library/std/scalar/src/lib.rx` (`class Int`).
- **Status:** READY-NOW with the same reboxing rule (`var a = 0 + self`). Verified: a full `gcd` loop with `%` over reboxed locals compiles and returns `12.gcd(18) == 6`.
- **Size:** 14 methods.
- **Matrix rows:** `core.md` Integer table 161-197; `stdlib.md` §31 rows that cross-reference core (785-788).
- **Pins:** 1120-1129.

### B14 — Integer iteration and base conversion — **DONE**
- **Landed:** `upto`/`downto`/`step` (both arities, as two real `def`s) / `to_s(base)` (an arity-1 overload beside the FFI `to_s`) / `to_i` / `to_int` / `ord` / `chr`, plus the `times` pin. `coerce` is MISSING: nothing in the language dispatches on the `[other, self]` pair, so the method would have no caller. Pins `1130` (iteration, incl. empty ranges) and `1131` (base conversion, incl. i64-min). Expected output diffed against CRuby 3.3.
- **Goal:** close the iterator family and stringification.
- **Methods:** `upto(n)`, `downto(n)`, `step(limit, step)`, `to_s(base)`, `to_i`, `to_int`, `chr`, `ord`, `coerce` (decide and record), `times` (already DONE, verify and pin).
- **Files:** `library/std/scalar/src/lib.rx` (`class Int`).
- **Status:** READY-NOW. `upto`/`downto` were previously blocked precisely because `self` would not reach the arithmetic floor; the reboxing rule removes that. `to_s(base)` builds its digits into a `String` via interpolation, which is allowed.
- **Size:** 9 methods.
- **Matrix rows:** `core.md` Integer rows 176-180; `language.md` §6 row 209 and Backlog A item 15 (407).
- **Pins:** 1130-1139.

### B15 — Array access and slot-moving
- **Goal:** the `Array` methods that only move slots, so none of them touch `T`.
- **Methods:** `shift`, `shift(n)`, `unshift`/`prepend`, `at(i)`, `first(n)`, `last(n)`, `values_at(*idx)` (use an `Array[Int]` arg until E7 lands), `slice(i, n)`, `delete_at(i)`, `each_index`, `rotate(n = 1)`, `rotate!`, `length`, `concat(other)`, `fill(value)`, `sort!`, `reverse!`, `dup`, `bsearch_index` (index form only; the value form needs `T` ordering, so leave it to B20).
- **Files:** `library/std/array/src/lib.rx`.
- **Status:** READY-NOW.
- **Size:** 18 methods.
- **Matrix rows:** `core.md` Array table 235-312 (rows `shift`/`unshift`, `at`, `first(n)`/`last(n)`, `values_at`, `slice`, `delete_at`, `each_index`, `rotate`, `length`, `concat`, `fill`, `sort!`, `dup`).
- **Pins:** 1140-1149.

### B16 — Hash Ruby surface (key-addressed and closure-only)
- **Goal:** make `Hash` read like Ruby's. Everything here is key-addressed (hashing happens in C) or closure-only, so none of it needs `T` equality in `.rx`.
- **Methods:** `store(k, v)`, `length`, `fetch(k)` / `fetch(k, default)`, `dig(k)`, `merge(other)`, `merge!`/`update`, `transform_values`, `transform_keys`, `each_with_object`, `count`, `slice(*keys)`, `except(*keys)`, `keep_if`, `delete_if`, `reject!`, `select!`, `to_h`, `values_at`, `first`, `replace(other)`, `delete(k)` (Ruby name over the existing `remove`).
- **Files:** `library/std/map/src/lib.rx`.
- **Status:** READY-NOW. **Excluded (blocked, see B21):** `value?`/`has_value?`, `invert`, `key(v)`, `assoc`/`rassoc`, `==`, `default`/`default_proc`.
- **Size:** 20 methods; split after `each_with_object` if it runs long.
- **Matrix rows:** `core.md` Hash table 321-361.
- **Pins:** 1150-1159.

### B17 — Set completion
- **Goal:** finish `Set` to Ruby's surface, including the predicates and the missing operator.
- **Methods:** `add` (delegate to `insert`), `add?`, `delete(x)`, `delete?`, `member?`, `===`, `length`, `count`, `merge(other)`, `subtract(other)`, `to_set`, `^` (symmetric difference, `(a|b) - (a&b)`), `subset?`/`<=`, `proper_subset?`/`<`, `superset?`/`>=`, `proper_superset?`/`>`, `disjoint?`, `intersect?`, `Set.from(Array[T])`, `classify` (block, returns `Hash`).
- **Files:** `library/std/set/src/lib.rx`.
- **Status:** READY-NOW. The predicates test membership through `include?`, which hashes in C, so no `.rx`-level `T` equality is involved. Note that `<=`/`<`/`>=`/`>` are **comparison** operators, which keep their dedicated route, so ship the named predicates first and check whether the operator spellings dispatch; if they do not, record it as a new enabler row rather than forcing it.
- **Size:** 19 methods.
- **Matrix rows:** `core.md` Set table 394-415; `stdlib.md` §2 (66-97).
- **Pins:** 1160-1169.

### B18 — Char and Bool scalar methods — **DONE**
- **Landed:** the full `Char` surface plus `Bool#to_i`; `Bool`'s `&`/`|`/`^`/`!` needed no `.rx` (they are already the machine operators, non-short-circuiting as in Ruby) and are pinned against regression. `Char#<=>` works in METHOD spelling only, and `Char#succ` is the next codepoint (Ruby's `String#succ` carries). A `Char` table was added to `core.md`. Pins `1170` (Char) and `1171` (Bool). Expected output diffed against CRuby 3.3.
- **Goal:** a usable `Char`, which every `String` char-by-char body depends on.
- **Methods:** `Char#ord`, `#to_i`, `#chr`, `#upcase`, `#downcase`, `#digit?`, `#alpha?`, `#alnum?`, `#space?`/`whitespace?`, `#punct?`, `#<=>`, `#succ`; `Bool#&`, `#|`, `#^`, `#!`, `#to_i`.
- **Files:** `library/std/scalar/src/lib.rx` (`class Char`, `class Bool`), plus `library/std/string/runtime/string.c` only if `ord` needs a shim.
- **Status:** READY-NOW. Verified: a `Char` `.rx` method comparing `self` against char literals (`self >= ?0 && self <= ?9`) compiles and returns correct results when the receiver is a variable. Remember the literal-receiver parse quirk (`?7.digit?` yields no output) when writing pins: bind to a variable.
- **Size:** 17 methods.
- **Matrix rows:** `core.md` has no Char table today, so **add one** next to Integer/Float (around 224) and fill the Bool rows in NilClass/TrueClass/FalseClass (540-554).
- **Pins:** 1170-1179.

### B19 — Kernel conveniences
- **Goal:** the free-function ergonomics a Ruby script assumes.
- **Methods:** `warn(msg)`, `abort(msg)`, `exit!`, `sleep(secs)`, `rand`/`srand` (expose the existing `library/std/rand` as bare Kernel functions), `Integer(str)`, `Float(str)`, `String(x)`, `Array(x)`, `p(x)` and `pp(x)` (over the `Debug`/`to_s` path), `inspect`.
- **Files:** `library/std/core/src/lib.rx` (`mixin Kernel`, the universal include) plus a small free-function module.
- **Status:** **LANDED** for `warn`, `abort`, `sleep(secs)`, `rand(n)`, `p` and `pp`, all as stdlib FREE FUNCTIONS in `core/src/lib.rx` (they take no receiver, so a `mixin Kernel` default would be emitted once per head for nothing). Overload-by-argument-type on a free function resolves, which is how `p` / `pp` cover `Int` / `Bool` / `String`. Not shipped, each for a proven reason rather than as a stub:
  - `exit!` — a free function whose name ends in `!` compiles clean and its CALL SITE then does nothing. Verified with `def shout!(x) ... end; shout!(5)`, which printed nothing and fell through. Shipping `exit!` over that would be a user-callable silent no-op. **This is a compiler bug worth its own batch.** Plain `exit(code)` needed no work: it is already a bare Kernel FFI decl in `library/std/process/src/lib.rx`, and Ruxen has no at-exit registry, so the two would have behaved identically anyway.
  - `Integer()` / `Float()` / `String()` / `Array()` — a capitalized identifier lexes as a `TypeIdentifier`, so `def Integer(s)` is a parse error (`expected identifier, found TypeIdentifier("Integer")`). Not a shape `.rx` can express today.
  - `srand` — `library/std/rand` exposes only OS entropy (`ruxen_rand_random_u64`) with no seedable generator, so `srand` could only lie about reproducibility.
  - `inspect` — as a `mixin Kernel` default it would call `self.to_s`, which is not universal, and mixin defaults are emitted for every including head. The inspect FORM is reachable through `p` / `pp`.
  - `rand` (arg-less) and fractional `sleep` need the `Float` ABI (B01).
- **Size:** 11 methods plus 3 flagged.
- **Matrix rows:** `core.md` Kernel/Object 473-513; `language.md` §12 (353-371).
- **Pins:** 1180-1183 (1184-1189 unused).

---

## 4. BLOCKED batches (write them the day the enabler lands)

### B20 — Element-typed collection methods
- **Goal:** the `Array`/`Enumerable` methods that need `to_s(T)`, `T` ordering, or `T` equality.
- **Methods:** `join` (including the `Array[Int]` case that returns empty today), `min`, `max`, `minmax`, `min_by`, `max_by`, `minmax_by`, `sum(init)`, `sum {}`, `uniq`, `uniq!`, `tally`, `group_by`, `sort_by` (Ruby's key-extractor form, distinct from the current in-place comparator `sort_by`), `include?(x)`, `index(x)`, `rindex(x)`, `delete(x)`, `flatten`, `compact`, `count(obj)`, `bsearch`.
- **Files:** `library/std/array/src/lib.rx`, `library/std/core/src/lib.rx` (Enumerable).
- **Status:** **BLOCKED-ON-PER-T-MONO (E1).**
- **Size:** 22 methods; split into two batches (ordering/aggregation, then equality/grouping) once unblocked.
- **Matrix rows:** `core.md` Array 272-292, Enumerable 439-449, plus the Operator-reality `Array[Int]#join` note (62).
- **Pins:** 1190-1199.

### B21 — Element-equality operators and Hash value operations
- **Goal:** the operators and methods that crash or return garbage because they need element equality.
- **Scope:** `Array` `-`, `*` (both repeat and join forms), `&`, `|`; `Hash#value?`/`has_value?`, `#invert`, `#key(v)`, `#assoc`/`#rassoc`, `#==`; `Array#==`/`eql?`/`hash`; `Set#==`/`hash`.
- **Files:** `library/std/{array,map,set}/src/lib.rx`.
- **Status:** **BLOCKED-ON-PER-T-MONO (E1).** Until then these must be a clean compile error, never a silent crash. If E1 slips, a stopgap batch that turns them into diagnostics is worth doing on its own.
- **Size:** 14 items.
- **Matrix rows:** `core.md` Operator-reality (44-68), Array 241-244, Hash 329-358, Set 404-413.
- **Pins:** 1200-1209.

### B22 — Collapse delegating methods back to `alias`
- **Goal:** now that Q58 is fixed, the Ruby-spelling synonyms become one-line aliases instead of hand-written delegating methods.
- **Scope:** revisit every `def x(...) = self.y(...)` delegator written for `Hash` (`has_key?`, `include?`, `member?`), `Array`, `Set`, and `String`, and replace with `alias`. Add the aliases that were skipped because of Q58 in the first place.
- **Files:** `library/std/{map,array,set,string}/src/*.rx`.
- **Status:** **READY-NOW.** E2 landed (`7b1c1907`, merged `42008fa3`), so this is unblocked.
- **Size:** small, mechanical; a good first batch for whoever follows the Q58 fix.
- **Caution:** it touches four `library/std` files that first-wave batches own, so run it **after** the first wave has merged, not alongside it.
- **Matrix rows:** wherever a row says "delegating method (pin 985/986)".
- **Pins:** 1210-1219.

### B23 — Float method family
- **Goal:** `class Float` reaches parity with `class Int`.
- **Methods:** `abs`, `ceil`, `floor`, `round(n)`, `truncate`, `nan?`, `infinite?`, `finite?`, `zero?`, `positive?`, `negative?`, `nonzero?`, `divmod`, `modulo`, `quo`, `fdiv`, `to_f`, `clamp`, `between?`, `<=>`, `step`.
- **Files:** `library/std/scalar/src/lib.rx` (`class Float`).
- **Status:** **BLOCKED-ON-B01** (the Float receiver ABI). Writing these before B01 produces methods that compile and return wrong numbers, which is worse than not having them.
- **Size:** 21 methods, all one-liners once unblocked.
- **Matrix rows:** `core.md` Float table 204-224.
- **Pins:** 1220-1229.

### B24 — Range as an Enumerable object
- **Goal:** `(1..5).map { }` stops returning empty and Range becomes a real value.
- **Scope:** a `Range[T]` surface that carries `begin`/`end`/`exclude_end?`, `include Enumerable[T]` over a real `each`, then `to_a`, `to_set`, `include?`/`member?`/`cover?`/`===`, `step`, `min`, `max`, `minmax`, `sum`, `count`, `first`/`last`, `size`, `lazy`. Beginless ranges (`..5`) pair with B26.
- **Files:** a new `library/std/range` package plus the Range lowering in `typeck` / `mir`.
- **Status:** **NEEDS-COMPILER-FEATURE (E8)**, with the aggregation methods additionally gated on E1.
- **Size:** one self-contained feature; give it a full session.
- **Matrix rows:** `core.md` Range table 364-386 and P5 in the priority backlog (684-687).
- **Pins:** 1230-1239.

---

## 5. Language / compiler batches

### B25 — Percent literals and heredocs
- **Scope:** `%q(...)`, `%Q(...)`, `%r{...}` (`%w[...]` is DONE, `%i[...]` waits on B29), and heredocs `<<~ID` / `<<-ID` / `<<ID`. The triple-quote string is the existing partial substitute, so keep it working.
- **Files:** `compiler/ruxen_core/src/lexer/{strings.rs,token.rs}`, `formatter`.
- **Status:** NEEDS-COMPILER-FEATURE. Lexer-local, low risk.
- **Matrix rows:** `language.md` §1 (60-79) and Backlog A items 17 and 19 (412, 414).
- **Pins:** 1240-1249.

### B26 — Small syntax gaps
- **Scope:** beginless range `(..5)`, `defined?(x)`, `redo`, `begin…end while` (do-while), `puts a, b` (multi-arg), `__FILE__` / `__LINE__` / `__method__`, `=begin`/`=end` block comments, bare octal `017` value semantics.
- **Files:** `compiler/ruxen_core/src/{lexer,parser}`, `typeck` for `__method__` folding.
- **Status:** NEEDS-COMPILER-FEATURE. Each item is small and independent; this is a good "many small wins" session.
- **Matrix rows:** `language.md` §1 (80-85), §3 (132), §6 (196, 203), §12 (356, 362, 367), Backlog A items 20, 22, 23, 26.
- **Pins:** 1250-1259.

### B27 — Splat parameters and keyword arguments
- **Designed. Read [`design/B27-splat-kwargs.md`](design/B27-splat-kwargs.md) before starting.** It decides that keyword arguments are a **compile-time call-site binding, not sugar over `Hash[Symbol, V]`** (a homogeneous hash cannot even type `create(name: "ada", age: 36)`, so the hash model is ruled out on expressiveness, not performance), refuses `**opts` with arbitrary runtime keys and heterogeneous `*args` in favour of typed structs, fixes the splat ABI (caller materialises the pack, callee borrows), states the splat-versus-arity-overload resolution rules, and splits the work into **B27a-B27e**.
- ⚠️ **Probes refuted a DONE row.** `language.md` §4 lists "Named-argument call `f(w: 3, h: 4)`" as **DONE (probe PASS)**. It **does not parse in any position** (`sq(w: 3)` → `error: expected expression, found Colon`) and there is no named-argument code in the parser. The row cites "probe PASS" rather than a case pin, and the original probe was almost certainly run inside a string interpolation, which is permissive. **Correct that row when the matrix files are free.** So B27 builds the keyword surface from nothing rather than extending anything.
- ⚠️ **`f(*xs)` at a call site is a silent no-op today**: it compiles and passes the array as one argument. It must become either a real splat or an error, per the no-silent-wrong-answer rule.
- **Scope:** positional splat `*args`, keyword splat `**kwargs`, required keyword args `def f(a:)`, splat call sites `f(*arr)`, and the `format`/`sprintf` free functions that ride on them. Also unblocks `puts a, b` if B26 has not landed it, `Set[a, b, c]`, and `values_at(*idx)`.
- **Files:** `compiler/ruxen_core/src/{parser,typeck,mir/lower}`.
- **Status:** NEEDS-COMPILER-FEATURE (E7). High value, medium-to-high risk (it changes the call ABI).
- **Matrix rows:** `language.md` §4 (145-147) and Backlog A item 12 (404); `core.md` Kernel `format`/`sprintf` (482).
- **Pins:** 1260-1269.

### B28 — Stabby lambda, `Proc`, `lambda`, `curry`
- **Scope:** `->(x) { }` syntax, `Proc.new { }` / `lambda { }` constructors over the existing closure values, `Proc#call`/`#[]`/`#()`/`#===`, `#arity` (constant-folded), `#curry` (a compile-time partial-application transform), `#lambda?`. Non-local `return` from a stored block stays out of scope; record why.
- **Files:** `compiler/ruxen_core/src/{lexer,parser}` plus a `.rx` `Proc` shim if a nominal type is wanted.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Matrix rows:** `language.md` §5 (176-180), Backlog A item 25; `core.md` Proc table (561-570).
- **Pins:** 1270-1279.

### B29 — Symbols (the big one)
- **Designed. Read [`design/B29-symbols.md`](design/B29-symbols.md) before starting.** It chooses the representation (a compiler primitive holding a 64-bit id into a static compile-time intern table, with the method surface in `.rx`), closes the symbol set at compile time so `String#to_sym` returns `Option[Symbol]` and there is no runtime interning and therefore no Ruby-style DoS vector, keeps `&:sym` exactly as it is, refuses `send` by runtime name and `:"#{x}"` dynamic literals, and splits the work into **B29a-B29e**. Two probes reshaped the batch: **`&:upcase` already works today with no symbol type** (a resolve-time desugar to a closure, so the highest-value symbol feature is already delivered), and **`Hash` cannot key on any user-defined type**, which rules out an `.rx` `Symbol` and makes **B73 a hard prerequisite for B29c**.
- **Scope:** a first-class `Symbol` value type plus the `:sym` lexer token, then everything that rides on it: hash symbol-key shorthand `{ a: 1 }`, `%i[a b]`, dynamic symbols `:"#{x}"`, `String#to_sym`/`#intern`, `Symbol#to_s`/`#to_proc`/`#length`/`#<=>`/`#succ`/`#id2name`, and hash patterns `in { a:, b: }` in `case/in`.
- **Files:** `compiler/ruxen_core/src/{lexer,parser,typeck,mir}` plus a `library/std/symbol` package for the method surface.
- **Status:** NEEDS-COMPILER-FEATURE (E5). This is the single largest language batch; scope it to the value type plus hash shorthand first, and defer `to_proc` and hash patterns to a follow-up.
- **Matrix rows:** `language.md` §1 (65-67, 72, 75), §7 (233), Backlog A items 8 and 9; `core.md` Symbol section (149-158).
- **Pins:** 1280-1289.

### B30 — `begin`/`rescue`/`ensure`/`raise`/`retry` sugar over `Result`
- **Scope:** parser-level sugar lowering to the existing machinery: `rescue` to the `Err` match arm, `ensure` to a `Drop`/defer scope guard, `raise X` to an early-return `Err(X)` inside a `Result` function, `retry` to the bounded-loop combinator. The substitute already works, so this is ergonomics, not capability. Ship it only after B27/B29 if capacity is tight.
- **Files:** `compiler/ruxen_core/src/parser`, a desugar pass, `formatter`.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Matrix rows:** `language.md` §8 (251-262), Backlog B item B7.
- **Pins:** 1290-1299.

### B31 — Pattern-matching completion
- **Scope:** hash patterns (needs B29), find patterns `in [*, x, *]`, the pin operator `^var`, `deconstruct`/`deconstruct_keys` protocols, one-line `x => pat` and `x in pat`, and array patterns with a splat in the middle.
- **Files:** `compiler/ruxen_core/src/parser/patterns.rs`, `typeck`, `mir` match lowering.
- **Status:** NEEDS-COMPILER-FEATURE, partly BLOCKED-ON-B29.
- **Matrix rows:** `language.md` §7 (221-238), Backlog A item 24.
- **Pins:** 1300-1309.

### B32 — `extension` maturation: `prepend`, `extend`, refinements, `class << self`
- **Scope:** `prepend` (insert a mixin ahead of the type in the compile-time method-resolution order, with `super` chaining), `extend` (lift a module's `def`s to class methods), lexically-scoped `extension` as the refinements substitute, `class << self … end` as parser sugar over `def self.x`, and type-level `def obj.x` singleton methods.
- **Files:** `compiler/ruxen_core/src/{resolve,typeck/infer/mixins.rs,parser}`.
- **Status:** NEEDS-COMPILER-FEATURE.
- **Matrix rows:** `language.md` §9 (274-277), §10 (298-303), Backlog B item B4.
- **Pins:** 1310-1319.

### B33 — Attribute-driven codegen substrate (THE enabler for tier 2)
- **Designed. Read [`design/B33-codegen-substrate.md`](design/B33-codegen-substrate.md) before starting.** It chooses the mechanism (the compiler synthesizes `field_names` / `accept[V]` / `from_fields[R]` from the field walk; every feature is an `.rx` visitor), places it at the end of typeck as real HIR (no typeck↔MIR lockstep), settles the metadata surface (an overridable `.rx` class method, since `@[...]` is retired), allocates diagnostics **E1201-E1205**, and splits the work into **B33a-B33f**. Three probes shaped it: a mixin default **cannot** read a field of the including class today (`no field 'name' on type 'Self: Describable'`), while a hand-written `accept[V: FieldVisitor]` and `self.from_fields[R: FieldReader]` both compile and run, so the only missing piece is writing those bodies from the walk.
- **Scope:** one reusable field-walk/codegen mechanism (an extension of the derive walker in `mir/lower/derive.rs` plus eligibility and diagnostics in `implicit_includes/`), then express these ON TOP of it in `.rx` or as declarations: `attr_accessor`/`attr_reader`/`attr_writer`, `Struct.new(:a, :b)`, `Data.define(:a, :b)`, `delegate`/`Forwardable#def_delegators`, `define_method` for a compile-time-known spec, `include Singleton`.
- **Files:** `compiler/ruxen_core/src/mir/lower/derive.rs`, `compiler/ruxen_core/src/implicit_includes/mod.rs`, plus `.rx` consumers.
- **Status:** NEEDS-COMPILER-FEATURE (E6). **This is the highest-leverage compiler batch in the queue after E1**: it unblocks the serializer derives, the ORM `Model` synthesis, and the web strong-params derive. Land the mechanism generically; do not special-case any one consumer.
- **Size:** one substantial feature. Give it a full session and a design note in `docs/decisions/`.
- **Matrix rows:** `language.md` §9 (271, 283-284), §11 (329-347), Backlog B item B1; `stdlib.md` §14 (420-439), §15 (449-456), §16 (466-471).
- **Pins:** 1320-1329.

### B34 — Compile-time reflection folding
- **Scope:** fold Ruby's reflective results at compile time: `is_a?`/`kind_of?`/`instance_of?` and `respond_to?(:m)` to constant `Bool`s over the known type and `include` set; `.class`/`.class.name` to the static type token (needs a non-keyword surface spelling); `send(:m, …)`/`public_send` with a literal name to static dispatch; `method(:m)` to a first-class function reference; `methods`/`instance_variables` to literal lists; `instance_variable_get("@x")` with a constant to field access; `__method__` to the enclosing method name.
- **Files:** a fold pass in `typeck`, plus `mir/lower`.
- **Status:** NEEDS-COMPILER-FEATURE, BLOCKED-ON-B29 (literal symbols) and leaning on B33.
- **Matrix rows:** `core.md` Reflection cluster (515-536) and P9 (705-714); `language.md` §11 (327-348), Backlog B items B2 and B3.
- **Pins:** 1330-1339.

---

## 6. Large self-contained stdlib features

Each of these is one package, one subagent, one session (or a scoped first
slice of one). The scoping paragraph is the brief.

### B35 — Civil `Time` / `Date` / `DateTime` (P0, the biggest single gap)

**Designed. Read [`design/B35-time-date.md`](design/B35-time-date.md) before
touching any sub-batch below.** It settles the representation, the timezone
scope, the leap-second and DST policies, and the C boundary, and it records a
probed trap (Ruxen integer division truncates toward zero, so every calendar
formula needs an explicit floor-division helper or every pre-1970 date is off by
one day while every post-1970 date is correct).

Ruxen ships only a monotonic `Instant`/`Duration` plus a raw `Time.unix_ns`, so a
real app cannot store a `created_at`, emit an ISO 8601 API field, write a log
timestamp, or set a cookie expiry.

**Split into six sub-batches, one agent each.** B35 as one batch is too large for
a single session. Two corrections to the original scoping note, both from the
design: the calendar math is **`.rx`, not C** (probed: a multi-`Int`-field class
with field-read arithmetic works, so a C shim would be unjustifiable under the
`.rx`-first rule), and the only new C is one function returning the local UTC
offset for a given instant.

| Sub-batch | Goal | Depends on | Status | Pins |
|---|---|---|---|---|
| **B35a** calendar kernel | `Calendar.fdiv`/`fmod` floor-division helpers, days-from-civil and civil-from-days, leap year, month lengths, day-of-week, day-of-year, and the `Date` type with day arithmetic and comparison. Pure `.rx`, **no OS, no C, no formatting** | nothing | **READY-NOW** | 1900-1909 |
| **B35b** the `Time` value type | `Time` as `secs`/`nsec`/`utc_offset`, normalisation, construction from epoch and from civil fields (UTC and explicit-offset only), field accessors, `include Comparable`, seconds arithmetic, `Time`↔`Date` conversion. Still **no OS call** | B35a | BLOCKED-ON-B35a | 1910-1919 |
| **B35c** the OS boundary | `Time.now` routed through the existing TestClock seam, the new `ruxen_time_local_offset_secs` C shim, `Time.local`/`#localtime`/`#getlocal`/`#utc`/`#zone`/`#utc_offset`/`#dst?`, and the ambiguous-time and gap policies | B35b | **NEEDS-RUNTIME-C**, BLOCKED-ON-B35b | 1920-1929 |
| **B35d** formatting | `strftime` over the design's v1 directive set, plus `iso8601`, `xmlschema`, `rfc3339`, `rfc2822`, `httpdate`, `to_s`, `inspect` | B35b | BLOCKED-ON-B35b | 1930-1939 |
| **B35e** parsing | `strptime`, `Time.iso8601`, `Date.iso8601`, the bounded `Time.parse`/`Date.parse` grammar, and the `TimeError` enum | B35d | BLOCKED-ON-B35d | 1940-1949 |
| **B35f** calendar arithmetic | `>>`, `<<`, `next_month`, `prev_month`, `next_year`, `prev_year`, `next_day`, `prev_day`, `succ`, `step`, `upto`, `downto`, weekday predicates, `Date.jd`, and the `DateTime` delegation layer | B35a, B35b | BLOCKED-ON-B35b | 1950-1959 |

**Ship order: B35a, then B35b, then B35d and B35f in parallel (they do not touch
each other), then B35c, then B35e.**

**B35c is the riskiest** and is deliberately last: it is the only sub-batch with
new C, the only one whose behaviour depends on the host, and the one where the
DST policies become real. Everything except local-zone rendering is useful
without it, and most downstream consumers (B41 logger timestamps, B45 http date
headers, B60 ORM timestamps) need only B35a plus B35b plus a slice of B35d.

Files: `library/std/time` (`src/lib.rx` plus `runtime/time.c`). Matrix rows:
`stdlib.md` §3 (112-166) end to end, `core.md` Time table (607-620). The
formula's pin block `1340-1349` is **abandoned**; see the exception table in §0.

### B36 — `digest` plus HMAC plus the hex codec
SHA-256, SHA-1, and MD5 (one-shot class methods and a streaming instance API:
`new`/`update`/`<<`/`digest`/`hexdigest`/`base64digest`/`reset`/`digest_length`),
a shared `hexencode`/`hexdecode` helper (also consumed by `SecureRandom.hex`),
`Digest::CLASS.file(path)` streaming from `fs`, a constant-time `==`, and
`HMAC.hexdigest(digest, key, data)`/`.digest`. This single batch unblocks signed
cookie sessions, CSRF, JWT HS256, ETags, and the WebSocket handshake, so it is
the critical path for the whole web security surface. Files: a new
`library/std/digest` package (`src/lib.rx` plus `runtime/digest.c`, or an FFI to
a vendored implementation). Status: **NEEDS-RUNTIME-C**. Matrix rows:
`stdlib.md` §5 (200-214) and the HMAC row in §8 (270). Pins: 1350-1359.

### B37 — `securerandom` conveniences
`library/std/rand` already gives CSPRNG bytes. Add the Ruby-facing conveniences
on top: `hex(n)`, `base64(n)`, `urlsafe_base64(n, padding:)`, **`uuid` (v4)**,
`uuid_v7` (needs B35), `random_number(n)` for both Integer and Float arguments,
`alphanumeric(n)`, and `choose`. `uuid` is a P0 because it is the default
primary key for any real app. Depends on B36's hex codec and the existing
`library/std/base64`. Files: `library/std/rand/src/lib.rx`. Status:
**BLOCKED-ON-B36** for `hex`; the rest is READY-NOW `.rx`, so it can ship in two
halves. Matrix rows: `stdlib.md` §4 (176-189). Pins: 1360-1369.

### B38 — `uri` plus `cgi` escaping
One RFC-3986 parser producing a `Uri` value (scheme, userinfo, host, port, path,
query, fragment), `URI.parse`/`URI.join`/`#to_s`/`#normalize`/`#merge`/
`#request_uri`/`#relative?`/`#absolute?`, plus one shared percent-codec powering
`URI.encode_www_form`/`decode_www_form` and their `_component` variants, and the
`CGI` helpers `escape`/`unescape`/`escapeHTML`/`unescapeHTML`/
`escapeURIComponent`. `escapeHTML` is what makes view output XSS-safe, so it
pairs with the view work in B65. Note that rondo has an inline `url_decode` in
`helpers.rx` that should later delegate here. Files: a new `library/std/uri`
package. Status: **READY-NOW** (`.rx` only, string processing over `chars`).
Matrix rows: `stdlib.md` §7 (239-254) and §10 (336-345). Pins: 1370-1379.

### B39 — `stringio`
An in-memory buffer with the `IO` shape: `StringIO.new(str, mode)`, `read([len])`,
`gets`, `readline`, `readlines`, `each_line`, `getc`, `ungetc`, `getbyte`,
`write`, `<<`, `puts`, `print`, `printf`, `string`, `rewind`, `seek`, `pos`,
`pos=`, `eof?`, `truncate`, `size`, `close`, `closed?`. Cheap, self-contained,
and a dependency of clean `csv` and `erb` output. `<<` wants B04's mutating
operator route; ship the `write` method first and add `<<` after. Files: a new
`library/std/stringio` package. Status: **READY-NOW** (`<<` row only:
BLOCKED-ON-B04). Matrix rows: `stdlib.md` §17 (481-489). Pins: 1380-1389.

### B40 — `csv`
The standard data-exchange path. Scope: `CSV.parse(str)`, `CSV.parse_line`,
`CSV.read(path)`, `CSV.foreach(path) { |row| }`, `CSV.generate { |csv| }`,
`CSV.generate_line(row)`, `CSV.new(io_or_string)` with `shift`/`read`/`each`/
`<<`/`to_a`, and the `CSV::Row` header-addressed view (`[]`, `[]=`, `field`,
`headers`, `to_h`, `to_a`). Options to support at minimum: `headers:`,
`col_sep:`, `quote_char:`, `skip_blanks:`, `write_headers:`. Correct RFC-4180
quoting and embedded-newline handling is the part to get right; write the pins
around quoted fields containing separators, quotes, and newlines. Build on B39
for the in-memory case. Files: a new `library/std/csv` package. Status:
**READY-NOW** (`.rx` only; nicer with B39). Matrix rows: `stdlib.md` §11
(355-368). Pins: 1390-1399.

### B41 — `logger`
Every service needs leveled logging. Scope: `Logger.new(sink)` over stdout,
stderr, or a file; the level enum (DEBUG < INFO < WARN < ERROR < FATAL <
UNKNOWN); `debug`/`info`/`warn`/`error`/`fatal`/`unknown` taking a message or a
block; `add(sev, msg, progname)`; `level`/`level=` and the `debug?`-style
predicates; a `formatter=` closure over severity/time/progname/message;
`progname=`; `<<` for raw writes; `close`/`reopen`/`with_level`. Thread-safe
writes via `library/std/sync`. Timestamps need B35, so ship first with a
monotonic or epoch-second stamp and upgrade the formatter when `strftime` lands.
Log rotation (`LogDevice` by age or size) is an explicit follow-up. Files: a new
`library/std/logger` package. Status: **READY-NOW** (timestamp formatting row:
BLOCKED-ON-B35). Matrix rows: `stdlib.md` §24 (636-647). Pins: 1400-1409.

### B42 — `optparse`
Pure string processing, so directly buildable. Scope: `OptionParser.new { |o| }`,
`on(*opts, desc) { |val| }` plus `on_head`/`on_tail`, `banner=`, `separator`,
`program_name=`, `version=`, `parse`/`parse!`, `order`/`permute`, required and
optional argument syntax (`--flag VAL`, `[VAL]`), typed coercion for Integer,
Float, and Array, generated `help`/`summarize`, automatic `--version`/`--help`,
and `Result`-shaped errors replacing `ParseError`/`InvalidOption`/
`MissingArgument`. Variadic `on(*opts)` wants B27; until then take an
`Array[String]` of option spellings. Files: a new `library/std/optparse`
package. Status: **READY-NOW** (with the `Array[String]` signature; the variadic
spelling is BLOCKED-ON-B27). Matrix rows: `stdlib.md` §25 (656-669).
Pins: 1410-1419.

### B43 — `yaml` (safe subset)
Config and fixtures, which is 90% of real YAML use. Scope: `YAML.safe_load(str)`
and `YAML.load_file(path)` over scalars, maps, sequences, `nil`, booleans,
integers, and floats; `YAML.dump(obj)`; multi-document `load_stream`. Anchors,
aliases, and merge keys are a stretch goal; `!ruby/object` tags and
`unsafe_load` are explicitly out of scope (no runtime class table). Because
Ruxen is typed, decide up front whether `safe_load` returns a `Json`-style
opaque node (recommended, mirroring `library/std/json`) or a typed struct via
the B33 derive. Files: a new `library/std/yaml` package. Status: **READY-NOW**
for the node-returning design. Matrix rows: `stdlib.md` §12 (378-387).
Pins: 1420-1429.

### B44 — `json` option gaps plus `to_json`/`from_json` derives
Two halves. The cheap half is `.rx` today: `JSON.dump`/`JSON.load` aliases,
`JSON.load_file`/`dump_file`, `JSON[…]` round-trip sugar, and a configurable
`max_nesting`. The valuable half is the serializer derive: `include Serializable`
synthesizing `to_json -> Json` from a struct's declared fields, and
`include Deserializable` synthesizing `from_json(&Json) -> Result[Self,
JsonError]`, recursing into fields that are themselves serializable. That half
rides B33's field-walk substrate and is what makes both the ORM and the web JSON
API stop hand-building nodes. Files: `library/std/json/src/lib.rx` plus
`mir/lower/derive.rs`. Status: option gaps **READY-NOW**; derives
**BLOCKED-ON-B33**. Matrix rows: `stdlib.md` §1 (36-56); `gems-web.md` §6
(234-242). Pins: 1430-1439.

### B45 — `net/http` full client
The current `library/std/http` is a single blocking `Http.get(host, port, path)`
with a hand-rolled parser, no headers map, no TLS. Build the real client: a
connection object (`Net::HTTP.new(host, port)`, `start`/`finish`/`started?`),
verb methods (`get`/`post`/`put`/`patch`/`delete`/`head`/`options`), request
builders with a headers map, body, `set_form_data` (needs B38's
`encode_www_form`) and `basic_auth` (needs `base64`), a response object with a
parsed headers map, `code`/`message`/`body`/`read_body`, a status-class
taxonomy, the one-shot class methods (`get`, `get_response`, `post`,
`post_form`), timeouts plumbed through to `net.TcpStream`, opt-in redirect
following, and chunked transfer-encoding support (the current parser assumes
`Content-Length` plus `Connection: close`). HTTPS is deliberately excluded here;
it waits on B46. Files: `library/std/http`. Status: **READY-NOW** for plain
HTTP, **BLOCKED-ON-B38** for form encoding, **BLOCKED-ON-B46** for HTTPS.
Matrix rows: `stdlib.md` §9 (291-328). Pins: 1440-1449.

### B46 — `openssl`: TLS, Cipher, KDF
The load-bearing subset, in priority order: `SSLContext` and `SSLSocket`
wrapping a `net.TcpStream` (this is what gates HTTPS serving, an HTTPS client,
SMTPS, and WSS); `Cipher` for AES-256-GCM (encrypted cookies and sessions);
`PKCS5.pbkdf2_hmac` and `KDF.hkdf`/`scrypt` for password hashing; then
asymmetric `PKey::RSA`/`EC` for JWT RS256; X.509 parsing last. Bind to a real
backend by FFI (rustls plus ring, or system OpenSSL) and make that choice an
explicit ADR since it is a new external dependency for the toolchain. Files: a
new `library/std/openssl` package plus `runtime/`. Status: **NEEDS-RUNTIME-C**.
Matrix rows: `stdlib.md` §8 (266-277). Pins: 1450-1459.

### B47 — `bigdecimal`
Arbitrary-precision decimal, required whenever money is involved (float is
simply wrong there). Scope: `BigDecimal(str, precision)` construction from an
exact string, `+`/`-`/`*`/`/` with scale tracking, `div(x, n)`, `%`/`modulo`,
`power`/`**`/`sqrt(prec)`, `round(n, mode)` with the rounding modes (including
banker's), `truncate`/`ceil`/`floor`, `to_s(fmt)`/`to_f`/`to_i`, and the
introspection predicates (`precision`, `exponent`, `sign`, `nan?`, `infinite?`,
`finite?`, `zero?`). `BigMath` transcendentals are a follow-up. Rank this up if
a financial app becomes the target. Files: a new `library/std/bigdecimal`
package. Status: **READY-NOW** (`.rx` over an `Array[Int]` limb representation)
or **NEEDS-RUNTIME-C** if a C bignum core is preferred; decide in the batch and
record why. Matrix rows: `stdlib.md` §33 (813-821). Pins: 1460-1469.

### B48 — `pathname`, `fileutils` gaps, `tempfile`
Three related filesystem batches that share one session. `Pathname` is an object
wrapper over the existing `library/std/path` string ops plus `library/std/fs`
IO: `Pathname.new`, `+`/`join`, `parent`, `basename(suffix)`, `sub_ext`,
`split`, `relative?`, `cleanpath`/`expand_path`, `relative_path_from`,
`each_filename`/`ascend`/`descend`, `root?`, the `exist?`/`file?`/`directory?`
predicates, `read`/`write`/`open`/`each_line`, `children`/`entries`/`glob`,
`mkpath`/`rmtree`/`unlink`, and `to_s`. The `fileutils` gaps are `cp_r`,
`touch`, `mv` with a cross-filesystem fallback, `rm_f`, `ln`, `ln_sf`,
`chmod`/`chown`, `install`, `compare_file`, `copy_stream`, `cd`, plus
**`Dir.glob`** and a `Find.find` recursive walk (both commonly needed and
currently absent). `tempfile` is `Tempfile.new`/`create` with `path`, `close`,
`unlink`, and `Dir.mktmpdir`, built on `fs` plus `rand` plus `env`. Files:
`library/std/path`, `library/std/fs`, plus a new `library/std/tempfile`.
Status: **READY-NOW**; the `mtime`/`atime` rows are BLOCKED-ON-B35. Matrix rows:
`stdlib.md` §18 (501-522), §19 (533-555), §20 (564-571). Pins: 1470-1479.

### B49 — Thread, Queue, Monitor ergonomics
`library/std/sync` has real threads, a typed `Mutex[T]`, atomics, and channels,
but the Ruby-facing shape is missing. Scope: an ergonomic typed
`Thread.new { }`/`Thread.start` (today `Thread.spawn` takes a raw function-pointer
Int), `#join(limit)` and a typed `#value`, `#alive?`/`#status`, `#name`/`#name=`,
`Thread.current`/`.list`/`.pass`, thread-local storage, a first-class
`Queue`/`SizedQueue` over the existing channel (`push`/`pop`/`size`/`empty?`/
`clear`/`close`/`closed?`/`num_waiting`), a standard `ConditionVariable`
(`wait`/`signal`/`broadcast`) over `Signal`, `Mutex#synchronize`/`try_lock`/
`locked?`, and a reentrant `Monitor` with `MonitorMixin`. Files:
`library/std/sync/src/lib.rx` plus a new `library/std/monitor`. Status:
**READY-NOW** for the wrappers; the typed-closure `Thread.new` may need a
compiler assist, so probe before committing. Matrix rows: `stdlib.md` §27
(698-716), §29 (746-752). Pins: 1480-1489.

### B50 — `observer`, `singleton`, `delegate`, `forwardable`
Four small mixin-shaped libraries. `Observable` is plain runtime state (a list
of observer closures, `add_observer`/`delete_observer`/`count_observers`/
`changed`/`changed?`/`notify_observers`) and is buildable in pure `.rx` today.
`Singleton`, `Forwardable` (`def_delegator`/`def_delegators`/`delegate`), and
`DelegateClass`/`SimpleDelegator` all want B33's generated-method substrate, so
they ship after it. Files: new `library/std/{observer,singleton,forwardable}`
packages. Status: observer **READY-NOW**; the other three
**BLOCKED-ON-B33**. Matrix rows: `stdlib.md` §15 (449-456), §16 (466-471),
§22 (605-610), §23 (620-627). Pins: 1490-1499.

### B51 — `prime` and number theory
`Prime.prime?`, `Prime.each(ubound)`, `Prime.take`/`first`, `prime_division`
and `int_from_prime_division`, `Integer#prime?`, and `Integer.sqrt` (isqrt).
Overlaps B13; if B13 has landed `gcd`/`lcm`/`pow(b, mod)`/`digits`/`bit_length`,
this batch is just the sieve plus factorization. Files: a new
`library/std/prime` package. Status: **READY-NOW**. Matrix rows: `stdlib.md`
§31 (779-788). Pins: 1500-1509.

### B52 — Misc P3 cluster: `pp`, `shellwords`, `benchmark`, `open3`, `timeout`
One session for five small, low-frequency items: a `PrettyPrint` engine plus
`PP.pp`/`pretty_inspect` over the `inspect` surface from B19;
`Shellwords.escape`/`split` for safe `process`/`Command` argument quoting;
`Benchmark.realtime`/`measure`/`bm`/`bmbm` and the `Tms` CPU-time breakdown
(needs `getrusage` via C); `Open3.popen3`/`popen2`/`pipeline` streaming pipes
over `library/std/process`; and `Timeout.timeout` scoped to the cases Ruxen can
honestly serve (async cancellation and socket timeouts, not forcible
interruption of arbitrary blocking code, which needs a thread-raise Ruxen will
not grow). Files: `library/std/{bench,process}` plus new small packages.
Status: **READY-NOW** except the `Tms` row (**NEEDS-RUNTIME-C**). Matrix rows:
`stdlib.md` §26 (679-686), §30 (764-769), §34 (827-835), §35 (846-855).
Pins: 1510-1519.

### B53 — `matrix`
Linear algebra: `Matrix` construction, special matrices, arithmetic,
determinant/inverse/transpose/rank/trace, element and row/column access, and a
companion `Vector` with dot/cross/norm/normalize. Rare in web and API work, so
this is the last stdlib batch; do it only when a graphics or ML consumer asks.
Files: a new `library/std/matrix` package. Status: **READY-NOW**. Matrix rows:
`stdlib.md` §32 (796-804). Pins: 1520-1529.

---

## 7. ORM batches (the `folio` package over `ledger`)

Nothing in `gems-db.md` is built; the package does not exist. The whole design
factors into one compiler feature (the `Model` field-walk synthesis) plus a
`.rx` library. Two batches ship **before** that feature and are therefore the
right place to start.

### B54 — folio Phase 0: the repository core (was "prove the stack")

**Designed. Read [`design/B54-folio-orm.md`](design/B54-folio-orm.md) before starting any folio batch.** It re-derives the architecture from probes and **overturns `gems-db.md`'s Phase 2**: a `mixin Model` carrying generic CRUD cannot be built, because a mixin default calling a static on `Self` crashes the Cranelift verifier and a generic class calling `T.static` finds no symbol (**B77**). folio is therefore a **repository** (`Repo` plus `Relation[T]` over a `ModelDesc[T]` **value**), not active-record, with a thin per-model facade of one-line forwarders. The spec also revises the B54-B61 phasing (§12), adds a small **B54b** for ledger gaps (`last_insert_rowid` is absent, and every bind is text), and answers N+1 structurally by **not having lazy association accessors at all** (§9).

Probes confirm the core is buildable today: a phantom-typed `Relation[T]` chains correctly in pure `.rx`, a decoder passed as a closure value works, and the ledger stack runs end to end (`ruxen run` in `examples/smoke` returns typed JSON rows).


Write the `Relation` (a lazy, chainable bound-parameter SQL builder producing a
`?`-placeholder string plus a parallel `Array[String]`, so it is injection-safe
by construction) and the row-decode helpers over `ledger`, plus one
**hand-written** model where `table_name`, `columns`, `to_row`, and `from_row`
are typed out by hand. That proves `find`/`all`/`create`/`where`/`transaction`
end to end with zero compiler support, at the cost of per-model boilerplate.
Everything written here survives into the real ORM; only the hand-written glue
gets deleted. Files: a new `folio` package (sibling repo layout mirroring
`ledger`). Status: **READY-NOW**. Matrix rows: `gems-db.md` §2 (124-167) and §3
(171-214), plus the Build-order Phase 0 note (467-473). Pins: 1530-1539.

### B55 — folio Phase 1: the `Model` field-walk synthesis
The single non-negotiable compiler ask for the ORM. `include Model` on a class
generates, from the declared fields: `self.table_name`, `self.columns`,
`self.column_types`, `to_row`, `self.from_row(&Json)`, `self.build(...)`, and
`to_json`. It extends the existing derive walker rather than inventing a
subsystem, and it should be built as the first consumer of B33's substrate, not
as a bespoke pass. Register it as a new `Q##` in
`docs/dev/gui-stack-v1-issues.md` and in `docs/TASKS.md`. Files:
`compiler/ruxen_core/src/implicit_includes/mod.rs`,
`compiler/ruxen_core/src/mir/lower/derive.rs`. Status:
**NEEDS-COMPILER-FEATURE (E6), BLOCKED-ON-B33**. Matrix rows: `gems-db.md`
"The one new compiler feature" (55-99) and §1 (102-120). Pins: 1540-1549.

### B56 — folio Phase 2: generic CRUD plus the full `Relation`
On the synthesized surface: `save`/`save!`, `create`, `update`, `destroy`,
`reload`, `find`, `find_by`/`find_by!`, `all`, `first`, `last`, `exists?`,
`count`, `find_each`/`in_batches`, `insert_all`/`upsert_all`; and the full lazy
`Relation` (`where` fragment and hash and IN forms, `where_not`, `or`, `order`/
`order_desc`, `limit`, `offset`, `group`/`having`, `joins`, `pluck`, `select`,
`distinct`, `to_sql`). Return shapes follow the documented decision: plain forms
return `Option`/`Result`, `!` forms return `Err(NotFound)`. Files: `folio`.
Status: **BLOCKED-ON-B55**. Matrix rows: `gems-db.md` §2 (124-167), §3
(171-214). Pins: 1550-1559.

### B57 — folio Phase 3: migrations plus transactions
The lowest-compiler-risk, highest-early-value slice, and it runs in parallel
with everything else: a `create_table(db, name) do |t| … end` DDL builder with
`t.string`/`integer`/`boolean`/`datetime`/`primary_key`/`timestamps`/
`references`, `add_column`/`remove_column`/`rename_column`, `add_index` with
`unique:`, reversible `change` plus explicit `up`/`down` (global rule 12), a
`schema_migrations` version table and a runner that skips applied migrations,
and a `transaction(db) do … end` helper that COMMITs on `Ok` and ROLLBACKs on
`Err`, with `SAVEPOINT` nesting. Files: `folio`. Status: **READY-NOW**.
Matrix rows: `gems-db.md` §7 (298-333), §8 (336-357). Pins: 1560-1569.

### B58 — folio Phase 4: associations plus eager loading
Read `belongs_to`/`has_many`/`has_one`/`has_many :through`/polymorphic
declarations in the synthesis and emit the accessor methods (each expands to a
one-line `Relation` call, so the codegen target is small), plus `dependent:
:destroy` and `counter_cache`. The N+1 fix, `includes(:posts)`, stays in `.rx`
as a batched two-query preload. Files: `folio` plus `mir/lower/derive.rs`.
Status: **BLOCKED-ON-B55**. Matrix rows: `gems-db.md` §4 (218-247).
Pins: 1570-1579.

### B59 — folio Phase 5: validations plus callbacks
Generate `valid?`/`errors` from `validates` declarations (presence, uniqueness,
length, format, numericality, custom `validate :method`) and emit the callback
hooks inside the generated `save`/`create`/`update`/`destroy`
(`before_save`/`after_save`, the create and update variants,
`before_destroy`/`after_destroy`, `before_validation`, `after_commit`/
`after_rollback`). Pair every `uniqueness` validation with an emitted `UNIQUE`
index and treat the constraint violation as the authoritative error, because
the validation is a check and the index is the actual lock. Files: `folio` plus
the synthesis. Status: **BLOCKED-ON-B55**. Matrix rows: `gems-db.md` §5
(251-273), §6 (276-295). Pins: 1580-1589.

### B60 — folio Phase 6: scopes, enums, timestamps, dirty tracking, serialization
Scopes as plain class methods returning a `Relation`, enum fields mapped through
`to_row`/`from_row` with generated predicates, automatic `created_at`/
`updated_at` (needs B35), `touch`, whole-record dirty tracking via the existing
`Clone`/`PartialEq` derives with per-field `name_was` as a synthesis extension,
`to_json`/`as_json(only:, except:)`, and typed column predicates
(`User.age_col.gt(18)`) as the query-builder endgame. Files: `folio` plus the
synthesis. Status: **BLOCKED-ON-B55**, timestamps **BLOCKED-ON-B35**.
Matrix rows: `gems-db.md` §11 (406-422), §12 (425-441). Pins: 1590-1599.

### B61 — folio Phase 7: multi-adapter plus connection pool
An `Adapter` mixin carrying every dialect difference (placeholder style,
`RETURNING`, identifier quoting, upsert syntax, last-insert-id) so the `.rx`
layers above never change per backend, then Postgres (libpq) and MySQL
(libmysqlclient) FFI drivers behind it, a connection-URL parser, and a
connection `Pool` with checkout/checkin (rondo is thread-per-core, so each
request needs its own handle; SQLite handles are not concurrently shareable).
Check what `SharedSync` already gives before adding a `std` mutex. Files:
`folio` plus new FFI packages. Status: **NEEDS-RUNTIME-C**. Matrix rows:
`gems-db.md` §9 (360-379), §10 (382-403), §13 (444-458). Pins: 1600-1609.

---

## 8. Web batches (rondo)

rondo is close to Sinatra-complete. The Rails surface that is missing is missing
mostly because of stdlib crypto gaps, not framework design. These batches live
in `~/.projects/rondo` (note: **rondo forbids the `Co-Authored-By: Claude`
commit trailer**, unlike this repo).

### B62 — rondo Stage 0: close the routing gaps
Pure `rondo .rx`, no compiler or stdlib work: splat/wildcard segments
(`/files/*path`), full-pattern regex routes with named groups,
`resources`/`resource`/nested resources and `namespace`/`scope` as ordinary
startup-registration methods that loop over the existing `get`/`post`
registrars, the `_method` verb-override before-hook, `send_file`/`send_data`
with `Content-Disposition`, `request.ip` threaded from the accepted peer address
with an `X-Forwarded-For` trusted-proxy flag, `req.params_all` merging form over
query and route params, and `request.xhr?`. Outcome: Sinatra-complete plus REST
route tables. Files: `rondo/src/{route,router,rondo,request,response}.rx`.
Status: **READY-NOW**. Matrix rows: `gems-web.md` §1 (109-127), §3 (161-179).
Pins: 1610-1619.

### B63 — rondo Stage 1: controllers, strong params, filters
A `Controller` mixin where each action is a method and the route binds a
specific method reference (no `send`, no name lookup), per-controller
`before_action`/`after_action`/`around_action` with compile-time `only:`/
`except:` lists and a compile-time `skip_before_action`, and `rescue_from` over
typed error enums. Strong params become a typed per-action struct with an
`include FromParams` derive synthesizing `from(req) -> Result[T, ParamError]`:
`permit` is the field set, `require` is a non-`Option` nested struct,
unpermitted keys are dropped by construction, and there is deliberately no
`permit!` escape hatch (global rule 4). Files: `rondo/src` plus the `FromParams`
derive in the compiler. Status: **BLOCKED-ON-B33** for the derive; the
`Controller` mixin half is READY-NOW. Matrix rows: `gems-web.md` §2 (138-153).
Pins: 1620-1629.

### B64 — rondo Stage 2: serialization and the JSON API
Land the `Serialize`/`Deserialize` derives from B44 in rondo's consumers: model
to JSON, JSON body to typed struct, nested and `has_many` embedding, and a typed
`ApiError` envelope wired to `on_error`/`rescue_from`. Then API versioning via
route prefixes, pagination as a query object over folio's `Relation` with
`Link`/`X-Total-Count` headers, and opaque bearer-token auth (a before-hook
looking the token up, no crypto needed). Files: `rondo/src`. Status:
**BLOCKED-ON-B44** (derives) and cross-referencing B56. Matrix rows:
`gems-web.md` §6 (234-242), §9 (286-298). Pins: 1630-1639.

### B65 — rondo Stage 3: views and templates
Two paths, in order. First an `Html` builder DSL in pure `rondo .rx`
(`tag.div(class: "card") { tag.span(user.name) }`) that escapes by construction
with no `raw`/`html_safe` bypass (global rules 13 and 27). Then the Rails
ergonomic path: a **toolchain** template compiler (enabler E9) that preprocesses
`app/views/**/*.rhtml` into render functions, where `<%= %>` is an escaped
append, `<%== %>` is an explicit and greppable raw append, `<% %>` is control
flow, and the locals are typed parameters so a typo is a compile error rather
than a runtime `NoMethodError`. Plus layouts (the inner content as a block),
partials as ordinary render functions with typed locals, `content_for` slots,
and the view helpers (`link_to`, `form_with` emitting the CSRF field). Files:
`rondo/src` plus `src/ruxen_cli`. Status: builder DSL **READY-NOW**; template
compiler **NEEDS-COMPILER-FEATURE (E9)**. Matrix rows: `gems-web.md` §4
(187-197); `stdlib.md` §13 (399-407). Pins: 1640-1649.

### B66 — rondo Stage 4: the security weave (sessions, flash, CSRF, JWT, ETag)
This is the batch B36 exists to unblock, and it delivers most of the Rails
security surface in one stroke. Signed cookie sessions (a typed struct
serialized to JSON, HMAC-signed, base64'd, written through the existing
`with_secure_cookie`, verified and deserialized in a before-hook, re-signed in
an after-hook), flash as a one-shot session sub-map, CSRF (a per-session HMAC
token, a before-hook rejecting unsafe verbs whose `authenticity_token` field or
`X-CSRF-Token` header does not match, and a `csrf_token` view helper), JWT
HS256 verification, Basic auth, HMAC request signing with a constant-time
compare, and ETag/conditional-GET (a body hash compared against
`If-None-Match`). Encrypted (rather than signed) sessions wait on B46's
AES-GCM. Files: `rondo/src`. Status: **BLOCKED-ON-B36**. Matrix rows:
`gems-web.md` §3 (167-178), §5 (216-217), §9 (294-297). Pins: 1650-1659.

### B67 — rondo Stage 5: background jobs
A `Job` mixin with `perform`, `enqueue`/`perform_later` serializing arguments
through the Serialize derive, an in-process worker pool draining the queue on
the existing thread-per-core reactor, then a durable queue over folio (enqueue
is an insert; the worker drains with `SELECT … FOR UPDATE SKIP LOCKED`),
retries with backoff on an attempt counter, scheduled jobs via a `run_at`
column and the timer future path, dedup keys and priorities, and
`after_perform`-style callbacks mirroring the controller filters. Coordinate
with the reactor-waker work in rondo's `remaining-tasks.md`. Files: `rondo/src`.
Status: **BLOCKED-ON-B44** (Serialize) and partly on B05 (the reactor bug).
Matrix rows: `gems-web.md` §8 (271-280). Pins: 1660-1669.

### B68 — rondo Stage 6: streaming, SSE, WebSocket
rondo buffers the whole response into one `String` today. Add a streaming
`Response.stream { |out| … }` writing chunks with `Transfer-Encoding: chunked`
over the async writer, then **SSE** (`text/event-stream`, no handshake, the
right first target), then the WebSocket layer: the `Upgrade` handshake
(`Sec-WebSocket-Accept = base64(SHA1(key + GUID))`, so it needs B36), RFC-6455
frame parse and encode on the kept-alive socket, and an ActionCable-style
channel registry (a `Sync` map from channel to connections) fanning broadcasts
to subscribers. Files: `rondo/src` plus the async writer surface. Status:
**BLOCKED-ON-B36** (handshake) and on the reactor work; the SSE half can start
earlier. Matrix rows: `gems-web.md` §10 (304-310), §3 row 175. Pins: 1670-1679.

### B69 — rondo Stage 7: mailers plus SMTP
A `UserMailer`-shaped class where each method builds a typed `Email { to,
subject, html_body, text_body }`, HTML and text bodies from the compiled
templates of B65, layouts and helpers shared with the web views, multipart
assembly, base64 attachments, `deliver_later` through B67's job queue, and an
`Smtp` client speaking the SMTP dialog over `net.TcpStream`. Plain SMTP works
over TCP today; real providers need STARTTLS or SMTPS, so delivery is gated on
B46. Files: a new `mailer` package or `rondo/src`. Status:
**BLOCKED-ON-B46** and B65. Matrix rows: `gems-web.md` §7 (252-260).
Pins: 1680-1689.

---

## 9. Audit gap

### B70 — write `gems-frontend.md`
`README.md` lists six matrices; only five exist. `gems-frontend.md` (the
frontend framework surface, components, state, forms, routing, data binding,
mapped onto quiver, plus the asset and build story) was never written, so that
whole area has no backlog. Audit quiver (`~/.projects/quiver`) the way the other
five were audited and produce the matrix. Files:
`docs/ruby-parity/gems-frontend.md`. Status: **READY-NOW** (documentation).
Matrix rows: creates a new file; add its row to `README.md` (60-64).
Pins: none.

---

## 10. Summary

| Blocker status | Batches | Ids |
|---|---|---|
| **READY-NOW** | **32** | B06, B07, B08, B09, B10, B11, B12, B13, B14, B15, B16, B17, B18, B19, **B22**, **B35a**, B38, B39, B40, B41, B42, B43, B47, B48, B49, B51, B52, B53, B54, B57, B62, B70 (note: B37, B44, B45, B50, B63, B65 are partly ready and counted under their blocker) |
| **BLOCKED-ON-PER-T-MONO (E1)** | 2 | B20, B21 |
| ~~BLOCKED-ON-Q58-ALIAS (E2)~~ | 0 | E2 landed (`7b1c1907`); **B22 is now READY-NOW** |
| **BLOCKED-ON-B01 (Float ABI)** | 1 | B23 |
| **BLOCKED-ON-B33 (codegen substrate)** | 5 | B34, B44 (derive half), B50 (3 of 4), B55, B63 (derive half) |
| **BLOCKED-ON-B55 (Model synth)** | 4 | B56, B58, B59, B60 |
| **BLOCKED-ON-B36 (digest/HMAC)** | 3 | B37 (hex half), B66, B68 |
| **BLOCKED-ON-B46 (TLS)** | 2 | B45 (HTTPS half), B69 |
| **BLOCKED-ON-B44 (Serialize)** | 2 | B64, B67 |
| **NEEDS-RUNTIME-C** | 7 | B03, **B35c** (one function; the rest of B35 is `.rx`), B36, B46, B52 (Tms row), B61, **B73** |
| **BLOCKED within B35** | 5 | B35b, B35c, B35d, B35e, B35f (all on B35a or B35b; see §6) |
| **NEEDS-COMPILER-FEATURE** | 26 | B01, B02, B04, B05, B24, B25, B26, B27, B28, B29, B30, B31, B32, B33, B34, B55, B65 (template compiler), **B71**, **B74**, **B75** (toolchain), **B76**, **B77** (enabler), **B78**, **B79**, **B80**, **B82** — **B72** landed |

Counts overlap where a batch has both a ready half and a blocked half; the
per-batch status line is authoritative.

**Recommended first wave:** B07 (Enumerable), B08 (String query), B12 (Integer
predicates), B15 (Array slots), B16 (Hash), B17 (Set). Each owns a different
`library/std` file, so six subagents can run at once. B01 and B02 (the two
compiler bugs) should run alongside them since they gate B23 and every
block-using pin.

**B35a is a seventh parallel-safe candidate.** It owns `library/std/time`, which
no other first-wave batch touches, it needs no OS call and no C, and it is the
foundation the rest of the P0 date work rests on. Its correctness is also fully
checkable against fixed inputs, so it is unusually cheap to verify.

**B06 must NOT run in this wave.** It is the only batch whose target files are
the matrices themselves (`core.md`, `language.md`, `stdlib.md`), and every other
batch has to update matrix rows in those same files on completion. Running B06
concurrently conflicts with all of them, and because it rewrites rows wholesale
it invalidates the line numbers every other batch cites. Run B06 alone, either
before the wave or after it has merged.

**Matrix-row conflicts are the one shared-file hazard.** Batches own disjoint
`library/std` files, but their definition-of-done step 3 sends many of them into
`core.md`. Either serialize the matrix edits (each agent updates rows only at
merge time, not in its worktree), or accept that step 3 is where merge conflicts
will land and resolve them by hand.

---

*Created 2026-07-29 from `HANDOFF.md` plus the five matrices. Probe findings in
§0 (scalar `self` reboxing, the Float receiver ABI, the Char literal-receiver
quirk) were established by compiling against the workspace stdlib with
`RUXEN_STDLIB_PATH` on that date, and are not recorded anywhere else yet.*
