# TODO: debt cleanup and the Ruby-only migration

This is the working plan for cleaning up Ruxen and moving it to Ruby syntax. A future agent can read this file and continue the work. Tick a box only when its check passes, and record new decisions in the Decisions section in the same commit.

Started 2026-10-14 on branch `feat/concurrency-adoption` (worktree `../ruxen-concurrency-adoption`). Last updated 2026-10-16.

## Evidence

Every item below points at one of these audits. They list each finding with `path:line`, and line numbers may have moved since. They were run read-only on `65aab1c`.

- [docs/audits/2026-10-code-debt.md](docs/audits/2026-10-code-debt.md): 45 debt families, F-1 to F-45, grouped into fix phases A to G.
- [docs/audits/2026-10-rust-isms.md](docs/audits/2026-10-rust-isms.md): 43 Rust-isms with their Ruby replacements, the ownership features that stay, and the migration phases.
- [docs/audits/2026-10-test-suite.md](docs/audits/2026-10-test-suite.md): where test time goes, and the tests to delete, move and merge.

The canonical language spec is [docs/specs/syntax/ruby-naming.spec.md](docs/specs/syntax/ruby-naming.spec.md), rewritten 2026-10-15. Each construct carries a tag: [implemented], [planned] or [deprecated].

## Decisions (owner, binding)

Product direction:
- Ruxen is Ruby: full Ruby parity, but static and compiled.
- The only thing taken from Rust is the borrow checker and ownership model: moves, borrows, drop, no GC, `Send`/`Sync`. Rust syntax and Rust-style features that earlier agents added are debt to remove, not intent.
- Language and stdlib logic lives in the stdlib as `.rx`, with C shims only where needed. Rust name tables in the compiler that hard-code stdlib methods are debt; move them into `.rx` declarations.
- Prefer generic code (modules and mixins, extension blocks, derived tables) over special cases keyed by a method or type name.

Priority order (2026-10-15):
1. Remove debt code.
2. Ruby parity.

Docs may change on the same branch at any time.

Stdlib rules:
- Public stdlib methods must stay. A declared but unimplemented public method gets implemented, not deleted.
- Rust-named methods that have a Ruby twin (`to_string` next to `to_s`) stay as deprecated aliases until the codemod finishes.
- Keep `Hashable` and delete `mixin Hash`.

Language decisions:
- **Errors:** Ruby exceptions: `raise`, `begin/rescue/ensure`, `retry`. They are statically typed and lowered to `Result` inside the compiler. `Result`, `?` and `panic!` leave user code.
- **Absence:** `T?` and `nil` only. No `Some(...)` and no `Option[T]` in user code.
- **Value types:** `Struct.new` and `Data.define`. Enums are `Data` variants or symbols, matched with `case/in`. `class` is for heap objects. No `.0` tuple access.
- **Async:** Ruby's Fiber-scheduler model, but only after the syntax migration. `async`/`.await` keep working until then.
- **Defaults taken** (the owner may still override):
  - Keep `name: Type` and `-> T` annotations.
  - Bounds are RBS-style `[T < Bound]`, with no `where`.
  - `mixin`/`extension` become Ruby `module` plus reopening a class.
  - `match` is removed in favour of `case/in`.
  - Keep FFI `lib "…"` blocks.
  - `size`/`length` return `Int`, and `to_i8`-style methods replace `as`.
  - Ownership type names stay as they are for now.
  - `%` is Ruby's floored modulo for Int and Float (`-7 % 3` is 2), with `remainder` for the truncated result (decided 2026-10-16).
- **Open:** 16 questions, OQ-1 to OQ-16, in spec section 20. Each has a default that applies until it is decided.

Process rules:
- **Tests:** run only what a change needs. `scripts/affected-tests.sh --print` gives the minimal set. Never run `cargo test --workspace` or the full e2e sweep after each change; the full sweep runs once at the end of a batch. The target for the whole suite is 1 to 5 minutes.
- **Commits:** git-guard rule 54 allows one unpushed commit per branch. Later work is folded into it with `git commit --amend` until the owner pushes. Stage explicit paths and never `git add -A`. Never push; pushing is the owner's job.
- **Disk:** `/tmp` is a small tmpfs. Keep build output in the repo's `target/` or `tmp/`, never under `/tmp`.

## Done

- [x] Merged `origin/worktree-agent-aaf5e4a…` (B74 class derives) and `origin/worktree-agent-ac8d826…` (B86 and B02) into `feat/concurrency-adoption` (merge commits `233214f` and `65aab1c`, pushed).
- [x] Commit `1759633` (local; later work is amended into it), audit Phases A to C, part 1:
  - F-1: the guard tests check something now.
  - F-4: Array and Hash methods are generic `.rx` code. The Float-through-reference compare is fixed for every binary operator. Ruby `sort` returns a new array, and `sort!` sorts in place.
  - F-6: the async `read_line` waker and the Poll box leaks.
  - F-8: Regex drop.
  - F-9: test fixtures moved to `library/test_std`.
  - F-11: module-nested types.
  - F-12: one pattern binder.
- [x] Docs pass a1. Docs no longer teach Rust forms the compiler never had, and old plans and prompts carry a "historical" banner. `ruby-naming.spec.md` is rewritten.
- [x] Test-harness step 1:
  - `build.rs` rebuild triggers.
  - Parallel e2e by default, with a fast path and `ulimit`.
  - The REPL replay is out of the default phases.
  - `scripts/affected-tests.sh`.
  - Result: the full default e2e sweep takes about 2 minutes, down from 15 to 30 or more.

## In progress

Phases A to C, part 2. The build workflow was stopped on purpose on 2026-10-15 at 02:01, after wave 2 and before wave 3 started editing. Each card below that is ticked passed its own command, rechecked by a separate agent, and is committed. None of them has had the independent `general-review` yet. That review must still run over everything since `1759633`.

- [x] c2 (F-2): no silent stubs. Covers `?` and `unwrap_or_else`, `super`, `block_on`, the index catch-all and the REPL stubs. Guard: `compiler/ruxen_core/tests/no_silent_stubs.rs`.
- [x] c3 (F-3): input that is dropped today gets a coded diagnostic (E0734 to E0738, E1127, E1128). Covers bootstrap items, `ruxen compile` flags and `--emit`, and the Tester's `expected_substr`.
- [x] c7 (F-7): Cranelift/LLVM drift (`ruxen_env_init`, Float `%`, `RefMut`), plus the LLVM Float-to-`&Float` argument bug.
- [x] c15 (F-15): dead internal runtime C symbols removed, with the guard "every exported runtime symbol has a declaration or an emitter". Public stdlib surface is kept.
- [x] c17 (was failing before this work): one shared test link helper (`compiler/ruxen_core/tests/common/`) includes the regex runtime and `-lm`. `drop_fixtures`, `runtime_safety` and `stdlib_json` link again.
- [x] c18 (was failing before this work): e2e `416_scope_collect_reactor_child` no longer times out (`library/std/future/runtime/reactor.c`).
- [x] c10 (F-10): the concurrency methods become real `.rx` code, and `typeck/method_resolvers/concurrency.rs` is deleted.
- [x] c5 (F-5): the formatter round-trips. `#{…}` is parsed as an expression, every `format_*` destructures exhaustively, and retired syntax is no longer emitted. Also fixes the `1101_string_regex_bridge.rx` idempotence failure and the `syntax_parity` failures.
- [x] c13 (F-13): dead Rust code deleted (after c5, c10). Two leftovers are under Known issues.
- [x] c14 (F-14): retired syntax removed; `Hashable` only (after c13).
- [x] c16: CHANGELOG and `CONTEXT.md` for everything since `65aab1c` (after c14).
- [x] Targeted end check (passed 2026-10-16: 672/672 e2e, 655 core lib tests, affected tests green):
  - `cargo build --workspace --all-targets`
  - `cargo test -p ruxen_core --lib`
  - `scripts/affected-tests.sh 1759633`
  - the full default e2e sweep, which must show 0 failures
- [x] Independent `general-review` of everything since `1759633`, with fixes: 3 rounds; its remaining findings predate this work and are under Known issues.

To resume, use the doing-substantial-work skill's build workflow (`~/.claude1/skills/doing-substantial-work/build-workflow.js`) with:
- `base: "1759633"`
- only the cards still unticked above, with their `after` lists reduced to unticked cards
- the same test budget: only each card's own command, no workspace runs
- fixture ranges: c5 1930-1939, c10 1970-1979, c13/c14 2000-2029

Card details are in [the code-debt audit](docs/audits/2026-10-code-debt.md), families F-5, F-10, F-13 and F-14. The user decisions for F-10 are under "Stdlib rules" above: implement the methods, never delete public ones.

## Next: debt removal (priority 1)

### Test-suite cleanup

Steps 2 to 7 of [the test-suite audit](docs/audits/2026-10-test-suite.md).

- [x] Make `ruxen test` work for stdlib packages:
  - Fix `src/ruxenc/src/test_runner.rs:307` so it skips the `src/lib.rx` merge for std packages.
  - Wire `RUXEN_STDLIB_PATH`.
  - Add `scripts/test-stdlib.sh` and a CI step.
- [x] Add the e2e fixtures that replace Rust stdout tests:
  - Promote `fixtures/sample_program.rx`.
  - Fold the `do _1` case into 1780.
  - Add an `.out` for the duplicate `213_` case.
- [x] Delete about 130 duplicate or vacuous tests. The audit table names what still covers each one. Keep 422, 576, 577, 728d and the `stdlib_file` truncation test.
- [x] Move the 24 `stdlib_*.rs` files and about 225 stdlib e2e fixtures into `library/std/<pkg>/tests/*.rx`, one package at a time. Delete a source test only after its new `.rx` test is green.
- [x] Merge the 178 integration-test binaries into about 6 (`tests/it/main.rs` per crate, `autotests = false`). Fix the process-wide env vars first.
  - Decision (owner): nextest was not adopted. With one `it` binary per crate, libtest already runs each binary's tests in parallel, and shared process state is fixed in the tests (explicit paths, `OnceLock`) rather than by a process per test. nextest would add a tool to install without a further speedup.
- [x] CI lanes:
  - Fast lane on every push: unit tests, `it`, the filtered smoke and `test-stdlib`.
  - Slow lane when a PR is ready: full smoke on both OSes, LLVM, and the `run.sh` cli and lsp phases.
- [x] Cut the e2e parity phase to a one-file smoke, and limit the REPL replay to a curated list.

### Code-debt Phases D to G

[The code-debt audit](docs/audits/2026-10-code-debt.md) has instances, fixes and risks for each family. This is the core of the duplicate and non-generic cleanup.

- [ ] **D, shared infrastructure:**
  - F-16: a HIR visitor.
  - F-17: adopt the canonical `Ty` helpers (`peel_refs`, `map_inner`).
  - F-18: a name and parent index on `SymbolTable`.
- [ ] **E, the HIR keeps resolution results:**
  - F-19: call shape (safe navigation, calls without parentheses, operators).
  - F-20: typeck records the method a call resolved to.
- [ ] **F, replace name tables with declarations:**
  - F-21: mixin-bound generics are monomorphised, and the sample-program names are removed from the compiler.
  - F-22: builtin statics and methods leave the Rust name tables.
  - F-23: the String "returns a new buffer" contract.
  - F-24: display, printing and interpolation are no longer keyed by type and name.
  - F-25: the ownership ABI and element-aware drop are no longer hand-listed.
  - F-26: the triplicated residual runtime tables.
  - F-27: the derive machinery.
  - F-28: compiler protocols encoded in names.
- [ ] **G, consolidation:**
  - F-29: one lowering policy for Cranelift, LLVM and the REPL JIT.
  - F-30: one front-end driver for `ruxenc`, CLI, REPL and IDE.
  - F-31: resolver copy-paste.
  - F-32 and F-33: parser and formatter duplication.
  - F-34: the parser's surface desugaring.
  - F-35: IDE definition lookup.
  - F-36: REPL replay.
  - F-37: build and link config.
  - F-38: shared C runtime helpers.
  - F-39: per-type copies in the runtime and stdlib.
  - F-40: stdlib `.rx` duplication.
  - F-41: typeck leniency.
  - F-42: async lowering duplication.
  - F-43: bootstrap lists maintained by hand.
  - F-44: diagnostics plumbing.
  - F-45: stale documentation.

## Then: Ruby parity (priority 2)

Order from [the Rust-isms audit](docs/audits/2026-10-rust-isms.md), section 6. Rust forms go: add the Ruby form, make `ruxen fmt` print it, rewrite the code, then warn, then error.

- [ ] Owner decides OQ-1 to OQ-16 (spec section 20).
- [ ] b1: add the missing Ruby forms to the parser, each with an e2e fixture, a negative test, and parity across the compiler, REPL, LSP, IDE and fmt:
  - `begin`/`rescue`/`ensure`/`retry` and `raise`
  - `initialize`, `attr_reader`/`attr_writer`/`attr_accessor`
  - symbols, `Struct.new`, `Data.define`
  - `->(x) { }`, `loop do`, `next` in every loop
  - `{}` as an empty Hash
  - array and hash patterns
  - keyword arguments
  - `=~` (spelled `~=` today)
  - `||` and `&&` on `T?`
  - `exclude Send`
- [ ] d: `ruxen fmt` prints the canonical Ruby forms: `@x`, no `let`, `case/in`, `require`, endless `def`, no `'a`.
  - Calls keep the parentheses the author wrote: `puts "x"` stays `puts "x"` and `x.map { }` stays `x.map { }`, never `x.map() { }`. Today the formatter always prints `()` because the AST does not record whether a call had them. Record that in the parser's call nodes first; F-19 then carries it into the HIR. Add a `ruxen fix --ruby` codemod built on the same formatter.
- [ ] c: rewrite with the codemod, per module, with tests green after each:
  - the stdlib (90 files), the 595 fixtures and the examples
  - then canvas, quiver and rondo, which were not audited; read `rondo/docs/ruxen-issues.md` first
  - then rewrite the Rust-flavoured `##` comments
- [ ] b2 and b3: warn on Rust forms (new coded diagnostics with fix-its), lowest usage first, ending with `use`, `match` and `let`. Then make them errors.
- [ ] a2: the second docs pass. Rewrite the tutorial (33 chapters), README, error docs and requirement tiers to the spec.
- [ ] Update code comments that cite old `ruby-naming.spec.md` section numbers (§3.x, §10a).
- [ ] Make `%` floored for Int and Float on both backends and the REPL, add `remainder`, and update e2e 1950.
- [ ] Later: the Fiber scheduler replaces `async`/`await`.

## Known issues and loose ends

- [ ] `Array#sort_by` sorts in place and returns `nil`. Ruby returns a new array, so make it match and add `sort_by!`.
- [ ] Inside a stdlib method, `self.clone` assigned to an `Array[T]` variable fails typeck: "expected `Array[T]`, found `Array[T]`".
- [ ] The installed toolchain (`~/.ruxen/bin/ruxen`) is from June and cannot parse newer fixtures. Run `ruxen upgrade --from-source ~/Documents/ruxen-lang/ruxen` once this branch lands.
- [ ] Unconfirmed: on the first run, 4 `ruxen test` dependency-linking tests failed (`consumer_test_runs_dep_generic_over_two_implementors`, `library_test_sees_dependency_symbol_and_runs`, `non_ffi_dep_test_still_links`, `test_links_flat_merged_ffi_dep_runtime`). They may have been environmental, so recheck them.
- [ ] `docs/specs/syntax/ruby-naming.spec.md` appendix A lists things not yet verified on a fresh binary: the `super` lookup order, `.call` on a stored block, hex/binary/octal literals, and `protected`.
- [ ] `typeck::type_check_with_bootstrap` (the non-package bootstrap path) is used only by 4 integration tests (`bootstrap_class_bodies`, `bootstrap_prelude_merge`, `no_std_e1400`, `wasm_codegen`). Port them to the package-aware path, then delete it. It cannot go behind `cfg(test)` because integration tests do not see `cfg(test)` items.
- [ ] The `ruxen_driver` crate is deleted on this branch. Drop its mentions in `../CLAUDE.md` and `ruxen/CLAUDE.md` (both git-ignored, outside this worktree) when the branch merges.
- [ ] `ruxenc::compile::project_target_ruxen` looks for `ruxen.toml` or `Cargo.toml`, not `Ruxen.toml`, so a project's incremental cache can land in `./target/ruxen` of the current directory.
- [ ] **High:** `ruxen fmt` silently deletes comments on statements, fields, mixin signatures and module or extension items.
- [ ] **High:** a guard from `Mutex#lock!` taken in a helper or as a temporary is never released, so the next lock deadlocks.
- [ ] **High:** reading a `&var String` (interpolation, `.size`, passing it to a `&String` parameter) treats the slot address as the string. Reads through `MutexGuard#get` are fine.
- [ ] **High:** a scalar `&var` (Int, Float, Bool) is passed by value, so a write through it never reaches the caller. This includes `MutexGuard#deref_mut` on `Mutex[Int]`.
- [ ] Interpolating a `&Float`, `&var Float` or `&var Bool` prints the raw machine word, not the value.
- [ ] A local named `args` is shadowed by the global `args` function.
- [ ] `Mutex.new` and `MutexGuard#set` take ownership of the payload and never free it (needs typed drop).
- [ ] `String#[]` builds the char array on every call, aborts on a negative index and returns `Char`, not a `String`.
- [ ] E1017 calls a type alias or newtype a `class`, and rejects an alias of a mixin.
- [ ] REGISTRY rows and docs for E1004, E1005 and E1008 remain, but nothing emits them any more.
- [ ] CHANGELOG [Unreleased] overstates a few things: Channel and "stdlib extensions", and "the formatter round-trips" (it still drops some comments). It also lacks Removed and Added entries.
- [ ] `Option.Some(f.await)` (an `.await` inside an enum-variant constructor argument) typechecks but fails to link: `Ready_await` and `Future_poll` are undefined. Covered only by the Rust test using `fixtures/ruxen/async_await_in_enum_variant.rx`; add an e2e case once it links.
- [ ] `hash_code` on `Int` and `String` through a `Hashable` generic bound typechecks but fails to link (`Int_hash_code`, `String_hash_code` undefined). No test covers this gap: the never-run `#[ignore]` pin was deleted. Repro: `def hash_it[T: Hashable](a: &T) -> Int` returning `a.hash_code`, called with `&42` and `&"hello"`.
- [x] e2e 2057 (the promoted `sample_program.rx`) printed created todo ids #1 to #5 but listed them as #0 to #4. The fixture was wrong, not the compiler: it gave each `Todo` a 0-based id while `Repository#add` returns 1-based ids. The fixture now uses the repository's id.
- [ ] No test covers comparing an FFI handle with an `Int` (`Mutex.new(7) == 0`, the null-check route in `mir/lower/derive.rs`); e2e 703 and 710 were both deleted. Add an e2e case.
- [ ] Release: the owner pushes and merges `feat/concurrency-adoption` into master.
- [ ] Async spec gaps that had only empty `#[ignore]` placeholder tests (removed):
  - `async_lowering.spec.md` B11: `.await` inside `if` or `match` arms does not lower.
  - B13: a `&` or `&var` borrow held across `.await` is not rejected (should reuse E1010).
  - B14: dropping a state machine mid-run drops every field, not only the current state's.
  - `executor.spec.md` B9: no runtime check that `block_on` drops the future exactly once.
  - B10: no runtime check that repeated `block_on` does not leak (the `Context` still leaks 8 bytes per call).
  - `async.spec.md` B5: `include Future` without `type Output = T` is accepted.
  - B6: a `poll` with the wrong return type is accepted on an `include Future` class.
- [ ] Send/Sync spec gaps that had only empty `#[ignore]` placeholder tests (removed): B5, no `channel[T]()` constructor, so `Sender`/`Receiver` non-Send rejection is untested; B7, `Thread.spawn` does not tell move from by-reference captures, so "by-reference capture needs Sync" is untested.
- [ ] The `type_check_with_bootstrap` line above is stale: `bootstrap_class_bodies` is deleted, and `llvm_backend` and `release_e2e_smoke` also call it.
- [ ] `Result#expect!` and `IoError#message()` results lose their type when used without a typed binding. `t.expect(FS.read_to_string(p).expect!("r")).to_eq("x")` compares String pointers and fails, and `let f = File.open(p).expect!("o")` then `f.read_to_string()` fails codegen with "no runtime symbol for `?T::read_to_string`". The std package tests work around it with `let s: String = ...` and `let f: File = ...`.
- [ ] A parenless `e.message` or `e.kind` on an `IoError` value is rejected as "cannot access field `message` on enum `IoError`"; only `e.message()` works. Ruby parity wants the parenless call.
- [ ] The `std::net` blocking echo server with a SIGINT shutdown stays a Rust test (`stdlib_net.rs`) because a `.rx` test cannot signal its own blocked `accept`. Move it once `ruxen test` can drive a child process.
- [ ] A class whose implicit init (from its fields) is called only inside a block fails to link (`Point_init` undefined). `library/std/fmt/tests/fmt_test.rx` gives `Point` an explicit init to work around it.
- [ ] `.unwrap` or `.nil?` on an `Option[C]` returned by a stdlib method, where `C` is a class (`JSON.object_get`, `Regex#find`, `Mutex#try_lock`), fails typeck with "no field `unwrap` on type `Json`". `.unwrap_or` works.
- [ ] `Some(3) == Some(3)` is `false`: Option equality compares pointers, so `t.expect(opt).to_eq(Some(x))` always fails.
- [ ] `Formatter#write_str` and `#write_char` return a null pointer for success, so `.ok?` on their result is `false`.
- [ ] `.await` on a hand-written `include Future` class passed as an `async def` parameter fails to link (`CountDown_await`, `Future_poll` undefined).
- [ ] `"#{x:?}"` ignores a user `include Debug` `fmt` and prints the derived `Tag { name: x }` form. Check whether that is intended (`fmt.spec.md` B2).
- [ ] `t.expect(x).to_eq(y)` compares pointers when `x` is an `Array[String]`, an `Option`, or a `String` from a destructured tuple (`let (evens, odds) = v.partition { ... }`, then `evens.join(",")`), so equal values fail. The std package tests compare `join` strings or bind a typed `let s: String`.
- [ ] A block that assigns a captured `var` without reading it (`last = x` inside `[1, 2].each do |x| ... end`) is rejected with E1009 "cannot assign to `last` — currently borrowed". `last += x` and `last = last + x` compile.
- [ ] A class with its own `def hash_code` fails codegen once used as a `Hash` key: `DuplicateDefinition("Ticket_hash_code")`.
- [ ] A parenless `a.hash_code` on a class with derived `Hashable` is rejected as "no field `hash_code`"; only `a.hash_code()` works.
- [ ] `Array.from(123)` is accepted at typeck and fails only at link time (the old `vec_from_int_is_currently_accepted_at_typeck` pin), and `Array.with_capacity` does not check that its argument is an `Int`.
- [ ] `ruxen test` has no stdout capture, so Kernel `p` / `pp` (e2e 1180) and `warn` (e2e 1181) stay e2e fixtures instead of `library/std/core/tests`.
- [ ] `case opt in nil ... in x` on an `Option[Int]` binds `x` to the Option pointer, not the payload, so `puts "#{x}"` prints an address (the ruby-naming spec marks this form [implemented]). `in Some(x)` works; e2e 2079 uses it.
- [ ] `Task.join(h).await` inside `block_on` hangs: the deferred e2e cases `728_async_spawn_join_basic` and `728c_async_spawn_loop` (removed; never run) compiled but did not finish within 10 s. Add an e2e case once spawned tasks join.
