# Ruby-parity push — session handoff

Read this first to resume the Ruby-parity work in a fresh session. It captures
the mission, the rules, what's done, the exact next steps, and the hard-won
gotchas so you don't rediscover them.

## Mission

Drive Ruxen to full Ruby (CRuby/MRI) parity — syntax, core classes, stdlib, and
eventually the DB/web gem surfaces — so a Ruby developer can build a full app in
Ruxen. User directive (2026-07-29): **"Add all. Fix the bugs and start on
features. Fully autonomous until everything is completed and approved."** and
**"the core methods can be expressed in ruxen as well ... you can just extend
those."**

Authoritative backlog: **`docs/ruby-parity/{README,language,core,stdlib,gems-db,gems-web}.md`**
— one matrix per area, every Ruby feature a row with status
`DONE | PARTIAL | MISSING | COMPILE-TIME | OUT-OF-SCOPE`. Update the row in the
same commit as the fix.

## The two hard rules (do not violate)

1. **`.rx`-first.** Implement every feature in `.rx` by EXTENDING the stdlib
   packages (`library/std/{string,array,map,set,scalar,core,...}/src/*.rx`). Only
   touch the Rust compiler for a genuinely inexpressible primitive or a missing
   *generic mechanism* (never a per-feature special-case). See `CLAUDE.md`
   "⚠️ `.rx`-FIRST" and `docs/specs/system/generic-compiler.spec.md`.
2. **No derive macros.** Ruxen's compile-time metaprogramming / class-feature
   surface is **mixins (`include`) + `extension`**, not derive/macros. The marker
   mixins (`Clone`/`Debug`/…) are compiler-synthesized but still written as a
   plain `include`.

## How to work (build / test / gate)

- Toolchain build: `cargo build --release --bin ruxen` (binary → `target/release/ruxen`).
- **Fast stdlib iteration WITHOUT re-embedding:** the CLI reads embedded stdlib
  by default, but `RUXEN_STDLIB_PATH="$PWD/library/std" ./target/release/ruxen
  compile f.rx -o f` loads `.rx` from the workspace. `include_str!` re-embeds on
  every `cargo build`, so a rebuilt binary already has your `.rx` edits embedded
  (no env var needed then).
- ⚠️ **That loop replays stale objects: `RUXEN_STDLIB_PATH` is NOT in the
  compile-cache key** (source hash + compiler version + target + project
  `runtime/*.c` + the binary's mtime/size, and nothing about the stdlib it was
  pointed at). Editing `library/std` and recompiling an UNCHANGED `.rx` reuses
  the old object, so the edit looks like a no-op or an old result poses as a
  fresh pass. **Wipe `<checkout>/target/ruxen/incremental` before any
  verification run that follows a stdlib edit, and treat an `Up to date` line as
  a red flag.** The embedded path is safe (`cargo build` changes the binary, so
  the toolchain fingerprint misses). A failure that vanishes on re-run with no
  edit is this, not a flake. Tracked as **B75** in `QUEUE.md`.
- **Narrow test loop:** `cargo test -p ruxen_core --lib` (≈650/0, ~0.5s). Additive
  `.rx` changes only need this (rule 42).
- **Where the harness lives (repo-root relative, NOT under `compiler/`):**
  `tests/release-e2e/` holds `run.sh`, `cases/`, `expected/`, `scripts/`, and
  `results/`. There is no `compiler/ruxen_core/tests/release-e2e/`;
  `compiler/ruxen_core/tests/` is ordinary Rust integration tests
  (`alias_keyword.rs`, `block_tier2.rs`, …), a different thing entirely.
- **Full gate (once per phase / after any compiler change):**
  `RUXEN_WORKSPACE="$PWD" PHASES=compile bash tests/release-e2e/run.sh` — compiles
  AND runs each fixture, diffs stdout vs `expected/`. Currently **~504 cases, 0
  fail**. Takes ~5-6 min; run in background, cache to `tmp/test-cache/`.
  - The fast-AR archive can go stale (missing async symbols → every case
    link-fails). Fix: `find target/release/build/ruxen_repl-*/ -name libruxenrt.a
    -delete; touch src/ruxen_repl/build.rs; cargo build --release -p ruxen_repl`.
- **DO NOT edit `library/std` or `cargo build` while a `RUXEN_WORKSPACE` e2e run
  is in flight** — it reads the workspace stdlib + the binary and will corrupt.
- **Every fix:** `.rx` method (or generic mechanism) + a pinned
  `tests/release-e2e/cases/NNN_*.rx` + matching `expected/NNN_*.out` + update the
  matrix row + `CHANGELOG.md` `[Unreleased]`, in one commit. Review every change
  under CLAUDE.md rule 44.
- **Pin numbers: do NOT use a single running counter.** Parallel batches would
  collide on it. Take your block from the allocation rule in
  [`QUEUE.md`](QUEUE.md) §0: batch `Bnn` owns pins `1000 + 10*(nn-1)` through
  `+9`. Highest pre-existing case is `989`; `990` to `999` are reserved for the
  in-flight per-`T` mono and Q58 work. Work with no batch id takes the next free
  block above every allocated one and records it in `QUEUE.md`.
- `.gitignore`s all `CLAUDE.md` (root + per-module) — local docs, not committed.

## What's DONE this push (branch `feat/concurrency-adoption`)

| Commit | What |
|---|---|
| `3fe826be` | Block-taking mixin defaults resolve on user classes (fixes Q57) |
| `b499fa7b` | `tap`/`then`/`yield_self` → `.rx` `mixin Kernel` (universal include); special-case removed |
| `439ea5cd` | Fixed `String *`, `Array +`, `Set \|&-` (were empty/garbage/segfault) + **builtin-head operator route** |
| `8c315461` | `Enumerable#find_all` / `none?` / `one?` |
| `83ce806a` | `Hash#has_key?`/`include?`/`member?`/`each_key`/`each_value`/`each_pair` |
| `43e5d199` `67db2891` | `String#reverse` / `capitalize` / `ljust` / `rjust` |
| `09580b95` | **Enabler:** default args now fill on builtin-head (`String`/`Array`/`Hash`/`Set`) method calls |
| `3b1709aa` | review nit (clippy) |

**Two generic mechanisms landed that unlock whole families:**
- **Builtin-head operator route** (typeck `infer/ops.rs::builtin_head_operator_method_available`
  + `infer/expr.rs` BinaryOp arm + MIR `binops.rs::builtin_head_has_op_method`):
  a non-comparison operator on a `String`/`Array`/`Hash`/`Set` receiver routes to
  the receiver's `.rx` `def OP` when one exists. So ANY builtin-head operator is
  now just a `.rx` method (delegating to a named method). Comparison/`??`/`~=`
  keep their dedicated routes.
- **Default-arg fill on builtin heads** (`infer/expr.rs`: `selected_method` now
  resolves for builtin heads too, mapping to the bare class name, WITHOUT pinning
  `*method`). So `.rx` methods with default args (`ljust(w, pad=" ")`) work.

## Probing rules (READ — a bad probe is worse than no probe)

- **NEVER establish a language fact from inside a string interpolation.** The
  interpolation re-lexer is permissive: invalid syntax inside `"#{…}"` compiles,
  then emits an **empty string** at runtime instead of failing. `language.md` §11
  files this under metaprogramming, but it applies to any probe. A blank line in
  probe output is the signature of this trap, not evidence of a working feature.
  Verified 2026-07-29: `puts "#{box(h: 3, w: 2)}"` compiles and prints blank,
  while the same call in a `let` is a clean parse error. This is very likely how
  `language.md` §4's false "Named-argument call ... DONE, probe PASS" row was
  produced.
- **A DONE row citing "probe PASS" rather than a case pin is suspect.** The
  process rule is that a row needs a `tests/release-e2e/` case; a probe-only
  claim has not been through the harness.
- **Compiles ≠ works.** Drive every probe to a real run and check the value, not
  just the exit status (this is `core.md`'s own methodology note, and it keeps
  paying).
- **Give every probe its own filename.** The compile cache does not key on
  stdlib identity (**B75**), so reusing a filename after a stdlib edit can replay
  a stale object.

## Hard-won `.rx`-authoring rules (READ — these cost hours to find)

- **`String` bodies MAY dispatch** (no generic `T`): build results char-by-char
  over `self.chars` (`Array[Char]`) + interpolation `result = result + "#{c}"`.
  - NEVER use `self` as a `String` value in `+` concat inside a `class String`
    body: `self` is the FFI-shell `&String`/`Class{"String"}`, which does NOT
    compose with the `String + String` concat route → `type mismatch: expected
    String, found String`. And `"#{self}"` interpolates the buffer POINTER
    (garbage). `self.clone`/`self.to_string` return the shell type too.
    Pattern that works: `var result = ""` then append each `"#{c}"` from
    `self.chars`.
- **Generic-collection combinators (`Array`/`Set`/`Hash`) must NOT dispatch on
  `T`** — they lower ONCE with `T` abstract (only `each`/`push`/closure-call).
  So `join` (needs `to_s(T)`), `sum`/`min`/`max` (need `T` ordering), `uniq`
  (needs `T` eq) canNOT be simple `.rx` bodies → they need per-`T` mono (below)
  or a runtime C helper. Delegating methods that just move slots (`find_all`→
  `select`, `has_key?`→`key?`) are fine.
- **`alias`-to-method on a GENERIC builtin head works** (Q58 FIXED, pin `990`).
  Prefer `alias new old` over a delegating body for a pure synonym. The one
  exception is an OPERATOR method (`def +`, `def |`): the builtin-head operator
  route is a name-only `DefKind::Method` existence check and an `alias` records
  no def, so an aliased operator falls through to the machine floor. Operators
  stay real delegating bodies.
- Operator methods (`def *`, `def +`) only need to EXIST in `.rx`; the builtin-
  head operator route dispatches them.

## Next steps (user chose "compiler enablers first")

1. **Per-`T` monomorphization for collection methods (BIG, highest leverage).**
   Unblocks `Array#min`/`max`/`uniq`/`flatten`/`join`, `Enumerable#sum`/`min_by`,
   etc. Plan: extend the generic-CLASS mono (`mir/lower/monomorphize.rs`
   "option 1", which already clones user generic-class method bodies per concrete
   receiver as `Box__mono__String_eq`) to emit `Array[Int]_join` / `Array[Int]_min`
   per concrete `T` at the call site, rewriting the method body's `T` → concrete
   (like `trait_default.rs::rewrite_self_in_func` but for the element param). Then
   these become normal `.rx` bodies that dispatch on `T`. Start fresh — it's
   multi-hour and touches lowering.
2. ~~**Q58 alias fix.**~~ **DONE.** Both `mir/lower/mod.rs::
   select_method_symbol_name` and `method_trailing_default_sentinels` now share
   `method_defs_on_class`, which falls back from the receiver's instantiated
   class name to its generic-free base. Hash's synonyms are one-line `alias`es
   again. Pin `990_alias_generic_builtin_head`.
3. **Cheap `.rx` wins (steady, low-risk):** more `String` (`swapcase`, `chomp`,
   `center` [default-arg now works], `sub`), more `Hash`/`Enumerable`. Follow the
   String char-by-char pattern.

## Larger feature gaps (after the enablers)

- **Civil `Time`/`Date`** — ranked P0; Ruxen only has monotonic `Instant`/
  `Duration`. Needs runtime C (calendar math) + `.rx` surface. Blocks records/
  APIs/logs. Self-contained large feature.
- **Missing stdlib packages:** `digest`+HMAC, `uri`, `openssl`/TLS, `csv`,
  `logger`, `optparse`, `stringio`, `bigdecimal`.
- **Runtime/compiler-backed method bugs still open:** `Array[Int]#join` (empty;
  needs per-`T` mono or C), `String[i]` indexing (returns null; needs a
  `ruxen_string_char_at` runtime + Index-path arm in `mir/lower/expr/index.rs`),
  `String#<<`/`Array#<<` mutation semantics.
- **Gems:** ActiveRecord/Sequel → a compile-time ORM; Rails/Sinatra → rondo. Both
  mostly `COMPILE-TIME`-planned, not built.

## Known PRE-EXISTING bugs (reproduce at baseline; not from this push)

- Float closure-argument passes as `0`: `[2.5].map { |v| v + 0.5 }` → 0 (affects
  `Array.map`, not just Kernel#then on Float).
- `arr.each { |x| puts(x) }` (do-end block + bare-var `puts`) segfaults;
  `arr.each { |n| puts "#{n}" }` (interpolation) works.
- `docs/dev/gui-stack-v1-issues.md` Q56 (async `block_on` client I/O no-ops) is
  the one live correctness bug tracked there. Q58 was added and FIXED this push.
- A non-scalar param default (`prefix: String = "v="`) reached through a
  PAREN-LESS call drops to the null sentinel and interpolates as empty, because
  `mir/lower/mod.rs::default_expr_to_sentinel` only maps `nil`/Int/Bool/Float.
  Reproduces on a plain NON-generic class at baseline, so it is not a Q58
  regression. Int/Bool/Float defaults land correctly.

## Key files

- Stdlib `.rx`: `library/std/{string/src/string.rx, array/src/lib.rx,
  map/src/lib.rx, set/src/lib.rx, scalar/src/lib.rx, core/src/lib.rx}` (core =
  `Comparable`/`Enumerable`/`Kernel` mixins).
- Operator route: `compiler/ruxen_core/src/typeck/infer/{ops.rs,expr.rs}`,
  `compiler/ruxen_core/src/mir/lower/expr/binops.rs`.
- Method-call typeck: `typeck/infer/{expr.rs,collect.rs,mixins.rs}`.
- MIR method dispatch/mangling: `mir/lower/expr/method_call.rs`, `mir/lower/mod.rs`.
- Mono: `mir/lower/{monomorphize.rs,trait_default.rs}`.
- Auto-memory captures the same state: `ruxen-ruby-parity-push`,
  `ruxen-features-in-rx-not-compiler`, `ruxen-no-derive-macros-use-mixins`,
  `ruxen-kernel-universal-mixin`.
