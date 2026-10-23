# TODO: debt cleanup and the Ruby-only migration

This is the working plan for cleaning up Ruxen and moving it to Ruby syntax. A future agent can read this file and continue the work. Tick a box only when its check passes, and record new decisions in the Decisions section in the same commit.

Started 2026-10-14 on branch `feat/concurrency-adoption` (worktree `../ruxen-concurrency-adoption`). Last updated 2026-10-17.

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
- **Memory:** Ruxen code must never leak. A fix that trades a double free or use after free for a leak is rejected, even as a stopgap (decided 2026-10-17).
- **Blocks, procs and lambdas** (decided 2026-10-17, settles OQ-5 in favour of `Proc`). They are Ruby's three kinds, and each has its own ownership rule:
  - A block (`xs.each { ... }`, `do ... end`, received as `&block: Fn[...]`) lives only for the call and borrows its captures. The enclosing scope frees them once. Storing a block is a compile error.
  - A block's body may read its captures but not move them out (return, bind, by-value argument, store, owning proc or lambda): E1019, fixed with `.clone`. A closure nested in another closure borrows what it takes from it, under the same rule.
  - A proc (`proc { }`, `Proc.new { }`, or a block received as `&block: Proc[...]`) and a lambda (`->(x) { }`, `lambda { }`) own their captures. Capturing a non-Copy variable moves it, so reusing it is E1001 and the fix is `.clone`. `Proc[(A) -> R]` replaces `any Fn[...]` for stored closures.
  - The old `let f = { || ... }` form is a lambda, and `ruxen fmt` writes it as `->() { ... }`.
  - Later (card "Ruby lambdas", after batch 3): counted shared captures so procs and lambdas need no `.clone`, with a cycle rule so they still cannot leak.
- **Operators** (F-19, decided 2026-10-17): operator methods declare borrowed operands (`def ==(other: &Money)`), and a call borrows an owned argument for a `&T` parameter as it already does, so `a == b` leaves `b` usable. An operator on a borrowed receiver (`&String <=> &String`) dereferences the receiver under the general method-lookup rule, with no operator special case. A by-value parameter on a comparison operator (`==`, `!=`, `<=>`, `<`, `<=`, `>`, `>=`, `eql?`) is a compile error with a fix-it to `&`. Arithmetic operators may take by-value operands. The stdlib, e2e, quiver and rondo operator declarations move to `&` params.
- **Option and Result in `.rx`** (F-22, decided 2026-10-17): no bare-variant export. Do the rest of F-22 now (delete the static-constructor tables, turn synonym FFI declarations into `alias`, re-tighten E0722). Move Option and Result to `.rx` only after the `T?` and exceptions migration, when user code no longer names their variants.
- **Printing** (F-24, decided 2026-10-17): Ruby's two methods. `puts` and `"#{x}"` call `to_s`; `p` calls `inspect`. Both are Kernel defaults in `.rx`, so no type prints a raw word: the default `inspect` is built from the fields (`#<User name="bob">`), and an object's default `to_s` is its `inspect`. Display, Debug and Displayable merge into this; `Formatter` stays internal for precision; the `Int_fmt` fallback is deleted.
- **Comparison** (F-27, decided 2026-10-17): Ruby's model only. Define `<=>` and `include Comparable` to get `<`, `<=`, `==`, `>`, `>=`, `between?` and `clamp`; `sort` and `min`/`max` require Comparable. PartialEq, PartialOrd and Ord leave user code. Equality without ordering is a plain `def ==(other: &T)`, value types (`Struct.new`, `Data.define`) get a structural `==`, and hash keys use Ruby's `hash` and `eql?` (proposed with this decision; the owner may still override these three).
- **Fields** (decided 2026-10-17): reading a field borrows it (`x = u.name` is a `&String`); keeping it beyond the object, or storing it elsewhere, needs `.clone`. Storing into a field moves the value in (already true for constructor arguments). An object frees its fields when it dies, after its own `def drop`, recursively. This closes the "class fields are never freed" leak (6999 leaks for 1000 `User` objects with a String and an Array field, measured with `leaks --atExit`). Tuples follow the same rule (decided 2026-10-23): a tuple owns its elements and frees them when it dies; reading `.0` or a destructured binding borrows a non-Copy element (`&T`), so storing it elsewhere needs `.clone` (E0744).
- **`case/when`** (F-34, decided 2026-10-17): Ruby's rule. `case` evaluates its subject once and calls `pattern === subject`. `===` is a `.rx` method with a Kernel default of `==`; Range, Regexp and Class override it. No compiler special case.
- **Calls without parentheses** (F-45, decided 2026-10-17): Ruby's general rule. An identifier followed by an argument on the same line is a call, with Ruby's spacing rules for ambiguity (`foo -1` versus `foo - 1`). The parser's bare-call allowlist is deleted. Batch 3 parser work.
- **Auto-borrow** (OQ-13, decided 2026-10-17): a `&T` parameter borrows an owned argument automatically; callers never write `&` at the call site.
- **Batch 1 leftovers** (decided 2026-10-17): byte buffers are `Array[UInt8]` (F-39); `autoload :Const, "path"` joins the grammar and the module loader stops scanning tokens by line (F-32); a `[runtime]` table in `Ruxen.toml` replaces `RUXEN_MACOS_FRAMEWORKS` and the hardcoded PCRE2 and Security lists (F-37); `include Send`/`exclude Send` inside a struct or enum body sets its flags and reports E1014 like classes (F-31); the non-Send check moves to `Ty::any` (F-17).
- **C headers across packages** (decided 2026-10-17): a package's C includes another package's headers by package name, never by relative path.
  - A package declares its public headers in `Ruxen.toml` under `[native] include = "runtime/include"` (the default when that folder exists).
  - The toolchain builds one include folder per build, `target/include/<pkg>/` (stdlib packages as `target/include/std/<name>/`), and passes `-I target/include` to every C compile, so code writes `#include <json/json.h>` or `#include <std/core/runtime.h>` wherever the package came from.
  - Only the package's dependencies (direct and transitive) are in that folder; including an undeclared package fails with "add `<pkg>` to `[dependencies]`".
  - Headers only declare: a dependency's C is compiled and linked once from its own `lib` declarations. Exported C symbols keep the `ruxen_<pkg>_` prefix.
  - A `#include "../..."` that leaves the package root is a build error with a hint to use `<pkg/header.h>`.
- **Borrow of a temporary** (default taken 2026-10-17; the owner may override): a borrow of a temporary's field or result (`n = U.new("t").name`, `mk(4).name`) keeps the temporary alive until the borrowing binding's scope ends, then frees it. Ruby-like, no new error, leak-free. The alternative was Rust's E0716 rejection. Mutating or moving the borrowed root while the borrow is live (`x = u.name; u.name = "new"; puts x`) is E1009 with a `.clone` hint.
- **Open:** OQ-1 to OQ-16 in spec section 20, except OQ-5 (Proc) and OQ-13 (auto-borrow), decided above. Each has a default that applies until it is decided.

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

Run as three build batches (2026-10-16). Each family's leftovers are listed under it; leftovers that need an owner decision are marked (decision).

- [x] **Batch 1 (D plus independent G leaves), built, proven and reviewed:**
  - F-16: `hir/visit.rs` (exhaustive `Visit`/`VisitMut`), used by no_std (struct and enum method bodies now get E1400), typeck type resolution, break/next collection, MIR captures and closure inlining, trait defaults, monomorphization the borrow checker and the 10 IDE walkers. Item walking added to `parser/visit.rs`, and the REPL walkers use the AST `Visit`.
  - F-17: canonical `Ty` helpers (`subst_params`, `any`, `callable_signature`, `builtin_head`, `method_home`, `is_scalar_primitive`); the three substitution copies, the callable and method-home copies and about 30 hand peels are gone.
  - F-18: `SymbolTable` name, kind and parent index (`named`, `nominal`, `type_def`, `methods_of`, overload-aware). Methods bind only their own `self`. Fixes a false E1001 on module function calls and the overload pick for parenless calls in typeck.
  - F-26: one `codegen/residual_abi.rs` table for both backends; the REPL JIT symbol table is gone.
  - F-31: one FFI registration path, one impl-item lowering with `AutoTraitFlags`, `DefKind::signature()`/`generic_params()`.
  - F-32 and F-33: shared parser body loop and signature parsing, one keyword/operator table, comments from lexer trivia. `ruxen fmt` no longer deletes comments on statements, fields, mixin signatures or module items; a comment it cannot place is an error, not silent loss.
  - F-37: `ruxen.toml` lookup fixed to `Ruxen.toml`; one `find_project_root`; `LinkInputs::for_project`.
  - F-38: shared C helpers in `runtime.h` and `future/runtime/future_rt.h`; `fmt` writes non-ASCII characters instead of `?`.
  - F-39 and F-40: shared C for BufReader/BufWriter, atomics, Set/Hash and stdio; `step_future.rx`; string and scalar helpers.
  - F-42: async lowering uses the real primitive names (`Int32`, `Char`, `String` outputs lower; e2e 2100), one diagnostic walker, one future-class synthesiser.
  - F-43 and F-44: bootstrap tables generated in `build.rs`; borrow-check titles from REGISTRY; IDE and CLI classify by code; unused E1004/E1005/E1008 removed.
  - Test temp dirs no longer leak into `/tmp` (a shared toolchain copy leaked 137 MB per `ruxenc` test run).
- [ ] Batch 1 leftovers:
  - F-17: `self` in builtin shell classes should get the structural head type once at assignment; the scalar name-to-`Ty` tables in resolve; `hir/context.rs` Fn-signature matches; `typeck/infer/ops.rs` non-Send walk (moving it to `Ty::any` widens Send errors, decision).
  - F-18: remaining `symbols.iter()` scans in `codegen/layout.rs`, `implicit_includes`, `resolve/funcs.rs`, `resolve/items.rs` and the IDE; resolve needs rename and set-kind setters first. MIR still picks the wrong overload for a parenless call of an overloaded method (F-20).
  - F-31: `resolve/` has no `CONTEXT.md`; three generic-param scope pushes remain in `resolve/items.rs`; `const_predicates` per-kind match. Struct and enum in-body `include Send`/`exclude Send` never set their flags and skip E1014 (decision).
  - F-32 and F-33: `module_loader` still scans `require`/`autoload` by token line because the parser rejects `autoload :Const, "path"` (grammar decision); IDE E1006 quick fix still uses `rfind("let")`; REPL `format_fn_signature`; fmt cannot yet place comments inside method chains or argument lists (it errors instead); partial `# fmt: off` ranges.
  - F-37: shared source gathering (needs `ModuleTree` below `ruxen_cli`); PCRE2 and macOS Security knowledge still hardcoded, and `RUXEN_MACOS_FRAMEWORKS` still read (needs a `[runtime]` manifest table, decision); workspace members' cache dir vs `ruxen clean`.
  - F-39 and F-40: StepFuture is shared functions, not a mixin base (mixin-default `poll` over `Self.Output` does not monomorphise); OpenOptions setters take `Int` (pinned by `runtime_abi_derivation`); byte buffers are `Array[Int]`, not `Array[UInt8]` (API decision); `TcpStream` timeouts take `Int` (net does not depend on time); `IoError_message`/`IoError_kind` free-function workaround; operator and Enumerable synonyms stay `def`; magic codepoints in `tr_set_expand`, `to_i`, `to_f`, `succ`; BufReader/BufWriter constructors stay per inner type.
- [ ] **HANDOFF (2026-10-21, checkpoint by the owner): start here. Card: pipeline unification (F-30 driver, then F-29 backend policy).**
  - Plan files (read first, they are the plan): `~/.agents/tmp/plans/ruxen-feat-concurrency-adoption-f29-backend-policy.md` (confirmed intent, approved combined plan, owner decisions, F-29 research) and `~/.agents/tmp/plans/ruxen-feat-concurrency-adoption-mir-borrow-check.md` (MIR card log).
  - Committed (local, not pushed; one unpushed commit, amend per step): the batch's round 4 work; the MIR loan checker `mir/loan_check.rs` in shadow mode (Phase 0-1: agreed with the HIR checker on all 932 matrix programs and fixtures, catches the I-30 shapes, `it` `mir_loan_check::loan_check_families` pins 18 cases); `ruxen_core::driver` (F-30 A0).
  - A1 done and amended (2026-10-21): ruxenc compile/cross/no_std/`--emit`/bench, the release-e2e harness and `tests/it/llvm_backend.rs` run `ruxen_core::driver`, so real compiles now run the MIR loan check (the I-30 shape is E1002 from `ruxen compile`); the check-only seam is gone (`ruxen check` will run full lowering); a derived struct `clone` is a fresh owner like a class clone (`MirFnRole::DerivedClone`), which fixed e2e 203's false E1009 and reduced its leak from 112 to 48 bytes. Checks: workspace build, clippy, fmt, `--lib` 681, `it` 568, ruxenc 10, release-e2e 390/390, LLVM lane 21, shadow 932/0, pins. Unverified: warnings-on-success (no warning code is registered yet to show one).
  - A2 done (2026-10-21, amended): `ruxen check` and `ruxen build` (binary, library and dependency pieces) run `ruxenc::compile::front_end` over the driver with `Options::for_target`, so `check` runs the borrow, lowering and MIR loan checks against the target's stdlib and prints the same errors as `build` (entry path on each line); `typeck::type_check_wasm` deleted (no caller). Pinned by `ruxen_cli` `it` `check_front_end`. Backend selection in build.rs untouched (F-29).
  - A3 done (2026-10-21, amended): the REPL (eval paths at `Stage::Mir`, the let probe, `:type` and `:type <def>` at `Stage::Typeck`) and the IDE `analyze` (`Stage::Mir`, typed HIR kept on errors) run `ruxen_core::driver`, so the I-30 shape is E1002 in both; REPL errors use the driver's format. No debounce added: the LSP analyses on didOpen/didSave only, never per keystroke. The IDE drops a MIR lowering error (no span to place it). Pinned by `ruxen_repl` `tests::loan_check` and `ruxen_ide` `analysis::tests::analyze_reports_mir_loan_errors`.
  - A4 done (2026-10-21, amended): the MIR loan check owns E1002/E1009 and the HIR loan code is deleted (`borrow_check/borrows.rs`, `places.rs`, `regions.rs`, operand holds, `LastUses`, the mutating-method name list, the driver's duplicate filter); E1003 is no longer emitted, kept registered (`docs/errors/E1003.md`). The `it` borrow helpers run `driver::check`. Expectation changes: `push_str` on a borrowed field chain is E1009 (was E1002); a `&var` field beside the object's handle (`hr(&var u.name, &u)`, `v.bump(&var v.name)`, `v.bump(v.name)`, v9) is accepted, Guard Malloc clean. Loan matrix in the repo (`tests/fixtures/loan_matrix/`, 485 programs + runner), checked by the ignored `it` `mir_loan_check::loan_matrix` with 22 accepted conflict rows listed. MIR borrow-check card Phase 2-3 done.
  - F-29 P0 done (2026-10-21, amended): parity list `compiler/ruxen_core/tests/common/parity_cases.rs` read by `it` `backend_parity` (Cranelift vs LLVM) and `ruxen_repl` `it` `jit_parity` (Cranelift vs JIT, child process); both in pr-ready's LLVM job. Ignored drifts: `neg_int32_arg_widened` (LLVM zext, P1), `ffi_int_param_takes_float` and `unit_self_kernel_default` (JIT, P2). Not reproduced on aarch64, so not added: Float->Int NaN/out-of-range (fcvtzs saturates in hardware; LLVM still emits plain `fptosi`, llvm/emit/mod.rs:534, so x86_64 would differ) and Bool/Char narrow FFI returns in the JIT. `RUXEN_E2E_BACKEND=llvm` release-e2e: 389/390, failing `2212_var_scalar_param_writes` (LLVM panic: pointer where an IntValue was expected), P1+ input.
  - F-29 P1a done (2026-10-21, amended): `codegen/abi.rs` owns AbiClass, FnAbi (entry by `is_entry`), CalleeIndex, B01 predicates and every coercion decision; Cranelift/LLVM keep a class -> type map; `runtime_sigs.rs`/`runtime_decl.rs` deleted. Fixed: LLVM negative Int32 widening, LLVM non-saturating Float->Int, Float into a declared Int param converted unsigned, LLVM 2212 panic. JIT keeps its own index (no FFI entries) until P2. Root cause left for later: MIR types a `&var` scalar param's deref temporaries as `RefMut(T)` (2212's `flip`), which only works because every backend treats a pointer-class value as a word.
  - F-29 P1b done (2026-10-21, amended): coercions are explicit MIR. `mir/abi.rs` (moved from `codegen/abi.rs`), `mir/residual_abi.rs`, `mir/lang_intrinsics.rs` (with `extract_method_name`; `codegen/runtime/` deleted); `mir::coercion::make_coercions_explicit` is the last step of `Lowerer::lower_program`, builds `MirProgram::callees` (both backends declare from it) and inserts `MirInst::Convert` for Assign, call args, returns, binop/compare operands. Backends keep `coerce` only for fixed-mode crossings (op result into dest, call result, Copy/Move/Ref/slot words, the dead `yield` passthrough); making those explicit too is a follow-up. JIT FFI imports now use the derived ABI (the one P2 item P1b forced); `unit_self_kernel_default` stays ignored for P2.
  - F-29 P1c done (2026-10-21, amended): result crossings are explicit MIR too (`mir::abi::produced`; op/call/address results retargeted to a temp + `Convert`; `Copy`/`Ref` across classes and `FloatToBits`/`BitsToFloat` become `Convert`; the passthrough pseudo-call becomes an `Assign`). Both backends' `coerce*` helpers deleted. Kept in a backend: Cranelift's `&var` slot word (`emit::slot_word`, storage), LLVM's string-compare `i64` -> `Bool` narrowing. Open: `yield` with no block in scope (inside a lambda) compiles and silently returns its argument (`lang_intrinsics::NOOP_PASSTHROUGH`); it should be a compile error (no silent no-op rule), its own card.
  - F-29 P2 done (2026-10-21): `CodeGen<M: Module>`; the JIT is `CodeGen<JITModule>` + its module construction (jit.rs 723 -> 257 lines); `compile_program` incremental/idempotent (declare_program, define_function, define_data); `isa(target, is_pic)` shared; `unit_self_kernel_default` passes in the JIT lane, the parity ignore mechanism deleted; `cranelift_share_parity`'s CLIF equality deleted (true by construction), its structure pin and JIT run kept.
  - F-29 P3 done (2026-10-21): `codegen/backend.rs` `Backend::select` (the one selection rule; ruxenc, `ruxen build` and the e2e harness call it; `--release`/wasm without the `llvm` feature is an error naming it; unknown `--backend`/`--opt-level` values are errors; `ruxen build` honours `[profile.*] opt-level`) and `Backend::emit_object` (the one object dispatch). `codegen::link_host` over a `LinkPlan` is the one host link (ruxenc's copy deleted), `aggregate_link_flags` and `object::compile_c_sources` one copy each; `compile_runtime_sources` deleted.
  - F-29 P4a done (2026-10-21): `codegen/ir_builder.rs` (`IrBuilder` trait) + `codegen/lower.rs` (`lower_function`, the one MIR walk, every instruction decision); Cranelift (batch + JIT) lowers through it as `cranelift::emit::ClifBuilder`; its instruction/terminator translator, `allocate_refmut_slots` and `def_local`/`use_local` free functions deleted.
  - F-29 P4b done (2026-10-21): LLVM lowers through the walk as `llvm::emit::builder::LlvmBuilder`; `llvm/emit/instructions.rs`, `control_flow.rs`, `calls.rs` and `types::ty_to_llvm` deleted. LLVM deltas: a float bitwise `BinOp` is a compile error (was the left operand, silently); a `Unit` parameter's alloca holds an `i64` 0.
  - Next (2026-10-21, checkpoint): Stage A, F-29 P0-P4b and the Stage B gate are done and amended. The final review (record `~/.agents/review-ledger/ruxen/branch-feat%2Fconcurrency-adoption/claude.md`) BLOCKED on I-41 (merged aliases), I-42 (direct closure calls) and medium I-43 (IDE lowering errors); fix round 1 (of 3) is in progress, plan in `~/.agents/tmp/plans/ruxen-feat-concurrency-adoption-mir-borrow-check.md`, section "Fix round 1 after final review". Done and amended: step 1 (place sets with depth cap 8, CallIndirect `modes`/`pair`, strict `unknown_param_mode`, IDE lowering errors, 7 pins) and step 4 (loan matrix `gen.py` + 176 programs over merge/multi-arg/closure-call axes, `memrun.py` Guard Malloc/ASan, pr-ready job `loan-matrix-memory`). UNCOMMITTED: steps 2-3 stopped midway (lowering records explicit borrow facts, the checker reads them and drops its reconstruction heuristics; 10 files, builds; backup `tmp/test-cache/fix-r1-facts/partial-steps-2-3.patch`; its last note: "update the drops default pass for Scalar and params"). Resume: review that diff, finish step 2, then step 3 (strict default for unrecorded facts; a false reject is fixed by recording the fact, never by loosening), then the full gate + `memrun.py` + apps + timing, amend, and the recheck review. Open notes: `memrun.py` copies `codegen::find_runtime_sources`' source list (duplicated rule; a compiler sanitize switch would remove it); 17 review lows parked for the owner.
  - Steps 2-3 (2026-10-22, uncommitted): the loan check reads `MirFunction::views` (`View`), recorded param modes and `ResidualFn::modes`; its type-based guesses are deleted and unrecorded facts are strict. The gate found false rejects, fixed by recording facts: `ruxen_*_ensure_key_kind` writes only an empty table's header (`Read`); the rebind frees elision deletes are skipped (`drops::elided_rebind_frees`, shared with `elide_returns_self_realloc`). A fresh checker plus HEAD comparison found four shapes HEAD accepts that crash under Guard Malloc (partial returns-self rebind, a view a callee stores in a field, a view moved into a returned object, a closure joined from two branches); all now rejected and pinned (`RECORDED_FACT_CASES`, `joined_closures`). OPEN, owner decision: stdlib `sync` test `t.expect(g.get)` is a false reject (a by-value arg's loans go to every by-reference arg); proposed (a) a per-function "param p may be stored into param q" summary, (b) keep strict and clone in the test, (c) rethink now. Open notes: the closure captures box (typed `Int`) is recorded `View::Scalar` (false but harmless); `residual_abi` gained `ruxen_vec_get_or_panic`, `ruxen_vec_skip_elem`, `ruxen_vec_chain_elem` for their modes (a grown C-symbol table; their home is a `.rx` lib declaration); pending: full gate, apps, timing, amend, recheck review.
  - Tuple ownership (2026-10-23/24, uncommitted; plan `~/.agents/tmp/plans/ruxen-feat-concurrency-adoption-tuple-ownership.md`): tuples own their elements and are freed (field and element glue, derived clone, Clone when the elements are); a tuple read borrows like a field (`typeck/infer/helpers.rs::slot_read_ty`, E0744 through tuple slots); hash entries are `(&K, &V)` views; `ruxen_hash_from_iter_keyed` copies keys and values; element glue is built from a recorded type (`MirFunction::elem_glue_types`, glue-name parsing deleted); boxed Copy values get one box per owner (`mir/lower/drops/copies.rs`, fixed a Copy struct double free). `MutexGuard#get`/`SharedSync#get` are `.rx` `def get -> T where T: Clone { self.deref.clone }`: a method `where` bound is visible in its body (`InferenceEngine::method_bounds`/`receiver_bounds`), every generic class with `lib` methods is a shell whose T-dispatching `.rx` methods are specialized per instantiation (`collect_builtin_head_mono_instances` over `lib_shell_classes`, discovery also follows generic-class instantiations; a `where` bound on the class generic makes the method `specialized_only`, its opaque body is not emitted), scalars define `clone -> Self { self }` (`library/std/scalar/src/lib.rx`), and `prune_unmonomorphised_bodies` also drops an unreachable opaque generic body (it linked against a pruned callee before, `Box2_peek`). quiver uses `SharedSync[Mutex[...]]` (271 tests), rondo `cycle.0.clone` and the accept pair moved whole (100 tests). Pins: `it` `field_ownership::a_tuple_element_read_into_an_owned_slot_is_e0744`, release-e2e 2240-2243. Pending: full gate, timing, amend, recheck review.
  - Review round 2 (2026-10-24, uncommitted; plan `~/.agents/tmp/plans/ruxen-feat-concurrency-adoption-review-round-2.md`): the round-1 recheck findings I-53 to I-62 are fixed; I-63 stays parked.
    - I-53: a method's `where` bound on its owner's generic is checked at the call site (`typeck/infer/ops.rs::check_owner_where_bounds`; `resolve/funcs.rs` registers the merged bounds). e2e 2196 declares `Cellish[T: Send + Clone]`.
    - I-54: a user generic class's where-bound method is `specialized_only`; the lib-shell pass walks every concrete instance body (generic classes, free fns, methods).
    - I-55: a boxed Copy struct is owned by its slot (`drops::glue::field_owns_value`); a derived clone box-copies it (`emit_box_copy`).
    - I-56: `pair_value_desc` is deleted; `Array#to_h` declares `pairs: ElemDesc[T]` (std/core `layout elem_desc`), filled by `key_desc::append_declared_elem_descs`.
    - I-57: the C-handle derive rule reads each mixin's own methods (`resolve/symbols/derive_support.rs::is_c_handle`), for every mixin.
    - I-58: `mir/loan_check/closures.rs` and `resolve/symbols/derive_support.rs` split out (979 and 813 lines).
    - I-59, I-60: `memrun.py` fails on unexplained compile failures, nothing run, rejected dead rows and unlisted accepted conflicts; the job has `timeout-minutes: 60`.
    - I-61: `ruxen_hash_from_iter_keyed` reads pairs from the last and skips a present key, so the last pair wins and a duplicate leaks nothing.
    - I-62: the doc nits are fixed.
    - Known remaining: a tuple built per loop iteration (`t = (p, i)` in a `while`) leaks with its owned elements (the loop body-local class below, owner decision).
    - Pending: full gate, timing, amend, recheck round 2.
  - Review round 3 (2026-10-24, uncommitted; plan `~/.agents/tmp/plans/ruxen-feat-concurrency-adoption-review-round-3.md`): the round-2 recheck findings I-64, I-54, I-57, I-65, I-66 and I-69 are fixed.
    - I-64: a generic class instance derives only what its substituted fields support (`synthesize_derives_as(.., substituted)`, `field_supports_derive`).
    - I-54: `close_class_instances` instantiates a generic class reached only through a generic body.
    - I-57: `DERIVED_METHODS` is deleted; `SymbolTable::derived_method` and `declares_derived_method` read the mixin's `.rx` declaration; `mixin Debug` declares `def to_debug -> String`.
    - I-65: `to_h` copies the key by key kind and moves the value, so an object key no longer leaks.
    - I-66, I-69: `ElemDesc[T]` parameters are left out of typeck arity (`source_param_count`); a descriptor still naming a type parameter becomes an abstract placeholder.
    - Parked lows for the owner: I-63 (tuple glue name collision, fixed 2026-10-24 by the injective instance key), I-67 (duplicate E1015 at one span), I-68 (doc splices: `check_owner_where_bounds` placement in `typeck/infer/ops.rs`, the `memrun.py:43` comment, the symbols line count), plus the earlier parked lows.
    - Pre-existing bugs the round-2 review recorded (P-13 to P-16) are open items below.
    - Pending: full gate, new commit (a0b982c8 is on origin), recheck round 3.
  - Instance engine (2026-10-24, rule-44 rethink after round 3, uncommitted; plan `~/.agents/tmp/plans/ruxen-feat-concurrency-adoption-instance-engine.md`): fixes I-54, I-63, I-64, I-65, I-69, I-70, I-71, I-72, I-73, I-75, P-13, P-14, P-16, P-17.
    - Typeck: constructor args bind class params through nested types (P-13), a method generic shadowing its type's is E0745 (P-14), a generic method call keeps its inferred result (P-16).
    - `mir/lower/instances/` is the one worklist for every generic body (monomorphize.rs deleted): one specialization predicate, injective full-type keys (`symbol_token`), `self` pinned, opaque bodies always lowered and pruned, E0746 at 32 levels (I-72). Pin: e2e 2247.
    - Derives: `ty_has_derive_trait` substitutes an instance's args (nested included); typeck gates derived methods per instance (P-17); a derive needs its mixin (`--no-std`, I-71); `field_supports_derive` deleted.
    - `KeyDesc[K]` (`layout key_desc`): tables hash, compare, copy and free keys by a compiler-filled key descriptor; `KEYED_TABLE_CTORS`/`OPS`, `ensure_key_kind`, the `_keyed` forms, `RUXEN_KEY_*` and `ELEM_DERIVE_HELPERS` deleted; object glue gains HASH/EQ; `lib` decls take own generics (`to_h[K, V]`); supplied params never reach typeck (`source_param_count` deleted); E0747 (I-69).
    - Known gap: no program is known to reach a live opaque body any more, so the prune diagnostic has no pin.
    - Pending: full gate, timing, amend, fresh review round 1.
  - Owner decisions (2026-10-21): rethink after three review rounds; F-30 + F-29 as one card, commit per phase by amending; all three backends stay; Float->Int saturates; `--release` without llvm is an error; MIR emits explicit coercions; parity test in the pr-ready lane; IDE runs the loan check debounced; stdlib diagnostics always fatal; warnings shown on success; `ruxen check` runs full lowering so check and build agree; build steps run in background subagents.
  - Pre-existing bug found in A1 (both builds): in script mode (top-level statements) diagnostics report the line after the real one (`u.name = "new"` on line 9 reported at 10:1); likely the implicit-main synthesis shifting spans. Fix with the driver work (A2) or its own card.
  - Open findings from the MIR card: collection mutators (`push`, `[]=`) are declared without `var`, so MIR cannot see `v.push` invalidating an element view; per-compile stdlib typeck (~35 ms) + lowering (~33 ms) for a tiny program (perf card, owner goal: Go-level speed); review round 4 lows I-35 to I-40 parked; quiver/rondo/canvas edits uncommitted in their repos.
  - Gate commands: `cargo test -p ruxen_core --test it`; `cargo test -p ruxen_core --lib`; `cargo test -p ruxen_repl`; `cargo build --release --bins`; `scripts/test-stdlib.sh`; `cargo test -p ruxen_core --test release_e2e_smoke -- --ignored`; `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `LIBRARY_PATH=/opt/homebrew/lib LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18 cargo test -p ruxen_core --features llvm --test it llvm_backend`; then `target/release/ruxen test` in quiver, rondo and canvas after clearing each `target/test`. Loan matrix: `python3 compiler/ruxen_core/tests/fixtures/loan_matrix/memrun.py` (memory run of every accepted program; `gen.py --check` keeps the programs in sync) and `cargo test -q --release -p ruxen_core --test it loan_matrix -- --ignored`. Memory proofs on macOS: `DYLD_INSERT_LIBRARIES=/usr/lib/libgmalloc.dylib <bin>` and `leaks --atExit -- <bin>`.
  - Way of working: one tree; narrow tests per card, full gate once per batch, one review; three fix rounds, then rethink; no new code debt; new `.rx` in Ruby forms; never trade a crash for a leak.
  - After the MIR borrow-check card: **pipeline unification next, F-30 then F-29** (owner, 2026-10-21: one card, one front-end driver for every binary, then one backend policy with explicit MIR coercions and the Cranelift/LLVM lowerers merged last; each phase amended in once green; intent and research in `~/.agents/tmp/plans/ruxen-feat-concurrency-adoption-f29-backend-policy.md`). Then phase 2 continues in this order: checkpoint item 3 (real-CLI proof), the leak sweep (Hash#each tuples, closure literal in a loop, F-25 returns and by-value args incl. Proc/String params, BufWriter/Mutex locals P-5/P-7, temp to `&String` FFI param P-6, loop-conditional moves P-9, borrowed temps in loops P-10, Option/Result boxes), rondo's borrowed `&Rondo` in Thread.spawn (close the method-call Send gap; rondo moves to an owned shared app handle), the batch 2 leftovers, then the decided items (F-19 operators, rest of F-22, F-24 to_s/inspect, F-27 Comparable). Then "F-29 first", batch 3, Ruby parity.
- [ ] **Batch 2: checkpoint, to finish on the Mac.** It was stopped at a checkpoint (2026-10-17), amended into the one commit and pushed at the owner's request. The combined-tree gate was stopped before it finished.
  - The last full gate passed on the tree without the CI-fix agent's link and runner changes: e2e 371/371 (the earlier "433" was a miscount: `git ls-tree` shows 371 cases at 9cee205; re-run on the Mac 2026-10-17, 371/371), core lib 675, `it` 474, all stdlib packages, fmt and clippy clean.
  - The link and runner changes were checked separately: `it` 474 with `--as-needed` forced, all stdlib packages, the `codegen::object` tests, fmt and clippy.
  - So run the full gate once on the Mac first.
  1. Fix the macOS `sync` double free (CI section below). Then make CI green: Ubuntu, macOS and Lint on the push.
     - Decided 2026-10-17: fix it with the blocks, procs and lambdas split (Decisions above). The double free is wider than two closure values: two blocks passed to a user `each` that capture one local both free it.
     - Reproduce on the Mac with `DYLD_INSERT_LIBRARIES=/usr/lib/libgmalloc.dylib` (add `MALLOC_STRICT_SIZE=1` for overruns); the plain macOS allocator does not crash on it.
  2. [x] Fix the `Mutex#lock` 8-byte overrun. `alloc_size` now reserves the payload slot of every `Option`/`Result` (`mir/lower/emit.rs`); pinned by `it` `tagged_union_alloc_size`. The sync tests that lock pass under `MALLOC_STRICT_SIZE=1` Guard Malloc.
  3. Rerun the real-CLI proof on a scratch `ruxen new` project. It must exercise:
     - safe navigation;
     - parenless and operator calls on a class;
     - user `each` and `push_str`;
     - a mixin-bound generic;
     - `Some(3) == Some(3)`;
     - interpolation of each scalar and a class;
     - `p` on each scalar;
     - `Hash[String, Array[String]]`;
     - a sorted class array.

     Then run `ruxen fmt` twice (nothing changes the second time, no line lost). Use `leaks --atExit` or ASan on macOS for the leak and double-free check.
  4. Run the end review (`general-review`, normal tier) over the whole batch, `git diff e3e7157`, and fix its blocker, high and medium findings.
- [x] **Ruby forms in this batch's new code** (owner, 2026-10-17). The 22 `.rx` files added by the blocks/procs/lambdas, fields and `&var` work (fixtures `compiler/ruxen_core/tests/fixtures/ruxen/*`, e2e `2195`-`2216`) and the inline programs in the new `it` modules use deprecated forms (`def main`, `let`, `var`). Rewrite them in the spec's Ruby forms: top-level statements, `x = 1` for both `let x =` and `var x =` (Ruby locals are always reassignable; spec section 2), parenless calls where implemented, `@ivar`. Keep a deprecated form only where the spec marks the Ruby form [planned], with a comment: today that is a typed local (`let bytes: Array[UInt8] = []` until `bytes: Array[UInt8] = []` lands) and `const NAME = v` until `NAME = v` lands. Rule for all new code from now on. Done 2026-10-17: the 8 fixtures, e2e `2195`-`2216`, the batch's `library/std/*/tests` additions and the inline programs of the 6 new `it` modules and `drop_fixtures.rs` use top-level statements, `x = 1`, `@ivar`, parenless `puts` and `->`; typed locals stay `let` inside a helper `def` (top-level `let` does not parse). Compiler gaps the rewrite exposed are fixed (top-level `while`/`if`/literal statements, `p = v` shadowing `Kernel#p`, a declaring assignment as a one-expression closure body, the last top-level expression typed against `main`'s `()`; e2e `2217`).
- [ ] **Batch 2 (E and F), built and checked card by card:**
  - F-19: the HIR keeps call shape (`safe_nav`, `parens` from the parser). `&.` is the only safe navigation; a plain `.` on `T?` no longer reaches inside. The MIR safe-nav guessing and the copied method-call pipeline are gone. String `+`/`<=>` and Set `==` are `.rx` declarations.
  - F-20: typeck records the resolved method. MIR picks the right overload for a parenless call. The IDE `resolve_struct_method` is implemented.
  - F-21: user methods named `each`, `map`, `find` and so on run their own bodies. Mixin-bound generics are monomorphised. A `?` callee is a hard error, and the three backend guessing heuristics are gone.
  - F-22: builtin statics come from `.rx` declarations, not Rust tables. `Some(3) == Some(3)` is true. `.unwrap` on `Option[Class]` from stdlib methods type-checks. Parenless `e.message`/`e.kind` work.
  - F-23: one String rebinding mechanism. A user method named `push_str` no longer overwrites the local.
  - F-24: interpolating `&Float`, `&var Float` or `&var Bool` no longer prints the raw word. `p`/`pp` are one generic each over `mixin Inspect`; scalars get `inspect`, and String's is quoted. `p 7` works without parens.
  - F-25: one ownership ABI table, and a recursive element drop. Nested containers (`Hash[String, Array[_]]`, `Array[Array[String]]`) no longer leak.
  - F-27: derive signatures are `Self`-typed `.rx` mixins. Classes get Ord, PartialOrd and Default. A user `hash_code` works as a Hash key. `hash_code` through a `Hashable` bound links for Int and String.
  - F-28: `MirLocal` kind (a user `_total` is freed like any local), `MirFunction` linkage and entry, drop classes by DefId, no LLVM-to-Cranelift module edge, one `impl_blocks_with_target`.
  - F-41: `&T` no longer unifies with `T`. Distinct type params do not unify. An unknown method on a known class is E0740. `Array.from(123)` fails at typeck, and `Array.with_capacity("x")` is E0741.
  - Fixed while proving, all of them older than this work:
    - `p(2.5)` called the String overload; a call matching no overload is now E0742.
    - `sort`/`sort!` on a class array double-freed. A `let` that only reads `v[i]` was freed every loop pass.
    - `ruxen fmt` collapsed multi-line `def`s, dropped blank lines and added parens. It now keeps parens as written, `@x`, `T?`, `where` placement, `do`/`{}` blocks and blank lines. A sweep of every e2e case and stdlib file loses nothing and is idempotent.
    - The `each` block parameter on class arrays is typed from Enumerable's signature.
- [ ] **CI failure on the pushed e3e7157** (run 38851223477; MSRV passed, the other three jobs failed):
  - **Build + test (ubuntu-latest):** about 43 integration tests fail to link with `string.c:(.text.ruxen_float_pow+0x5): undefined reference to 'pow'`. Examples: `std_use_resolution::*`, `stdlib_io::*`, `stdlib_process::*`, `task_spawn_ownership::*`, `user_runtime_c::user_runtime_c_is_compiled_and_linked`.
    - Cause: Ubuntu's gcc links with `--as-needed` and resolves archives left to right. `-lm` (from the per-package `[system_libs]`) comes before `libruxenrt.a` and the runtime objects on the link line, so it is dropped before `pow` is needed.
    - Fedora does not default to `--as-needed`, so it passes locally.
    - Fix: put system libs after every archive and object in every link path (`ruxenc`, `ruxen build`, the test harness `tests/common/mod.rs`).
  - **Build + test (macos-14):** `scripts/test-stdlib.sh` reports `FAIL sync`, 7 passed and 1 failed.
    - The Tester runner does not print which test failed, so the log cannot name it.
    - Likely cause: a timing or pthread difference in `library/std/sync` (timed waits or `clock_gettime`; macOS lacks `pthread_condattr_setclock`). This needs confirming on the Mac.
    - Also, the script's duration column prints an epoch (`1792125809.00s`) because BSD `date` has no `%N`.
  - **Lint (advisory):** `cargo fmt --check` diffs at e3e7157 in `tests/it/main.rs`, `stdlib_json.rs`, `stdlib_array_negatives.rs`, `mir_debug_spans.rs`, `parser/tests.rs` and `src/ruxenc/src/compile.rs`.
  - Status at the checkpoint (amended commit):
    - **Ubuntu link: fixed.** One `link_command_args` in `codegen/object.rs` puts the program object, runtime objects and archive first, then every `-l` flag. It is shared by `compile_host`, `compile_no_std` and `ruxenc`. Guard test: `link_command_puts_library_flags_after_every_input`. Checked locally with a `cc` wrapper that forces `--as-needed` (474 `it` tests pass). Gap: `ruxen build` still passes dependency `.o` paths inside `extra_link_flags`.
    - **Runner: fixed.** It names each failed case and how it died, and the durations are portable.
    - **Lint: passes** on the current tree.
    - **macOS sync: NOT fixed.** It is memory corruption, not timing.
      - The failing case is `SharedSync / "round-trips a Mutex[String] between closures"`.
      - Two closures (`reader`, `writer`) both capture `cell` by value, and both env-drop functions free it. The second `ruxen_sharedsync_drop` decrements freed memory.
      - glibc survives this; the macOS allocator keeps its free list in that word and crashes.
      - Fix in `mir/lower/expr/closure.rs` (`synthesize_closure_drop` / `capture_slot_still_owned`): two closures must not both own one non-Copy capture. Capture by reference, or make the second move E1001.
      - Confirm on Linux: `RUXEN_TEST_FORMAT=documentation valgrind --trace-children=yes library/std/sync/target/debug/test/sync_test`. On the Mac, `scripts/test-stdlib.sh sync` should now name the case.
    - **Also from valgrind: `Mutex#lock` (`.rx`, returning `Result`)** allocates 8 bytes and reads and writes 8 bytes past them, on every `m.lock`. The `Result[MutexGuard, _]` allocation is probably too small. Not fixed.
    - Not run before the checkpoint: MSRV `cargo +1.91 build` and `cargo check --workspace --benches`. CI on the push will run them.
- [ ] Batch 2 leftovers:
  - **High, not in the audit: ordinary call arguments are never type-checked.** Every call site in typeck does `let _ = unify(arg, param)`, so `baz("s")` compiles for `def baz(x: Int)`. It is blocked by the Int-erased concurrency surface (`Race.any_of`, `spawn_raw(future_ptr: Int)`), by unify lacking `any Fn + Send` vs `any Fn`, and by a block returning into a `-> ()` slot.
  - F-19 (decided 2026-10-17, see Decisions: `&` operator params): operators as method calls. Rewriting `a == b` to a call made by-value operator params move their operand. Choose between auto-borrowing operator operands and `&` operator params in the stdlib. `&String <=> &String` is still E0707.
  - F-20: `MirInst::Call` structured callee (about 154 sites), so callee strings are still re-parsed in MIR and codegen. Extension methods on builtin heads are registered as free functions, so their parenless overloads are still wrong; register them as methods of the shell class. Not recorded for mixin defaults and `.new`.
  - F-21: generic-calling-generic across classes (`Outer[T]` holding `Holder[T]`) is not monomorphised; it needs a worklist. `unique_bound_impl` still guesses.
  - F-22 (decided 2026-10-17, see Decisions): Option and Result declared in `.rx` need stdlib enums to export bare variants. E0722 cannot be re-tightened while Array `get`/`get_mut` share one C symbol with different surface types.
  - F-23: `def var` on a top-level lib function is silently ignored. Mutating through `&String` runs on a copy.
  - F-24 (decided 2026-10-17, see Decisions: Ruby `to_s`/`inspect`): the `Int_fmt` fallback for Array, Option, Hash and type params (needs Display formatters). Primitive fmt via `.rx` `include Display` (needs `Formatter#precision` in `.rx`). `to_s` as a Kernel default. IO_TEXT_BUILTINS. Displayable, Display and Debug as one protocol. User classes have no `inspect`.
  - F-25: the Mutex payload leak (`get` returns an alias and `into_inner` does not consume). Ownership markers in the lib grammar.
  - F-27 (decided 2026-10-17, see Decisions: Comparable only): Ord/PartialOrd/PartialEq into Comparable. A derived `eq(other: &Self)` is treated as a move by the borrow checker. Enums have no derived bodies. Bool, Char, Float and the collections have no `hash_code`.
  - F-28: the `__state`/`__sub_` checks need async lowering to mark generated classes. Four codegen sites still test `name == "main"`. `include Into[X]` loses its generic argument in HIR.
  - F-41: TypeParam against a concrete type is still lenient (`Array#sum`'s `var total: T = 0`, user generic statics, `Self::Output` as a TypeParam).
  - Leaks (valgrind on the scratch program: 778 bytes definitely lost before, 0 after). Fixed: class elements in Array and Hash values (per-type glue descriptor `RUXEN_ELEM_OBJ`), interpolation temporaries, and String rebinding in loops and mono bodies. Still open:
    - A class's own String or container fields are never freed (decided 2026-10-17, see Decisions "Fields": reads borrow, the object frees its fields). Next card after the checkpoint.
    - `Set[Class]` elements and `Hash[Class, _]` keys (needs key copying through the glue in `hash.c`). Generic classes and enums as elements.
    - A value returned by a user function, and a by-value argument to one (the F-25 ownership-ABI gap). The same gap leaks a by-value argument passed to a closure call (`f.(7, "s")`) and a `proc` passed by value.
    - A closure nested in an owning lambda that escapes the lambda's body (returned or stored) carries a borrowed capture out; the lambda then gives that capture up instead of freeing it, so it leaks (`mir/lower/expr/closure.rs` `NestedRecapture`). Needs counted shared captures (card "Ruby lambdas").
    - The element-escape guard is conservative: a container passed to a closure-taking `.rx` method (`map`, `select`) is made shallow, so its elements leak. Narrowed by cards f4/f5: a call through a closure now lends its arguments unless a body that can be the target stores them (`drop.spec.md` B13), so a user `each(&block)` over its own Array keeps a deep drop; `select`, whose inner block stores, still demotes.
    - Closures held by fields and collections are now freed (cards f4/f5, `drop.spec.md` B12). Still open there: `Hash#each` / `each_key` / `each_value` leak one tuple per entry (`ruxen_hash_entries`) and make the iterated Hash's drop shallow; a shared `Proc` passed by value to a callee that does not store it keeps one owner too many (F-25).
    - `Array#drop(n)` and `+` copy class elements with `<T>_clone`, so a class's `def drop` also runs on the copies.
    - An `Option` / `Result` local or temporary is never freed: its box leaks (`let o = Some(s)` with no match, the `nil` arm of `if let Some(v) = o`, `if let Some(v) = mk(1)`). Only an arm that moves the payload out of an owned local frees the box (`MirPayloadMove`, drop spec B14).
    - A block-local pushed into an array the block captures from its enclosing scope leaks 16 bytes (`run do let q = "a-#{k}"; arr.push(q) end`).
  - Pre-existing, found while proving: `if let Some(r) = v.find { }` on a class array segfaults; `v.count { }` has no block overload. fmt still drops grouping parens, alignment padding and `"""` heredocs.
  - Found with the one-rule E1019 change (2026-10-17):
    - The borrow checker does not see a move repeat across loop iterations: `while c; eat(s); end` compiles. Walk loop bodies so a move of an outer variable inside a loop is E1001. (A move before `break` / `next` now reaches the code after the loop, I-16.)
    - A by-value binding out of a call result or element (`if let Some(v) = arr.get(0)`, a getter returning a field's Option) owns the payload though the container or object still does (same root as the `v.find` segfault). A binding out of a field read or reference now only views it.
    - I-13 remainder: a method call skips the `Send` capture check (`check_send_required_closure` runs on free calls only), so `Thread.spawn(-> { r.size })` with `r = &s` compiles and reads freed memory (review probes x4/x4h). Closing it rejects rondo's `Thread.spawn({ || async_worker_shared(app, shared_fd) })` in `src/async_server.rx`, which sends a `&Rondo` to worker threads; rondo needs an owned, shared app handle first (owner decision).
- [ ] **F-29 first** (owner, 2026-10-17: Cranelift, LLVM and the REPL JIT must share one place and one set of rules). Do it right after the batch 2 merge and commit, before the rest of batch 3 and before any further backend fix.
  - One backend-neutral `codegen::abi` (machine class per type, every coercion and signedness rule, parameter word slots, callee and FFI lookup); `CodeGen` generic over `M: Module` so the JIT reuses the batch driver (done, P2); one `Backend` interface (`emit_object`, `aggregate_link_flags`, `compile_c_sources`); backend dispatch written once (today `codegen/mod.rs` x3 and `ruxenc`).
  - Guard: a parity test that compiles and runs the same fixture set through Cranelift, LLVM and the JIT and compares output; an LLVM lane in CI.
  - Owner decisions (2026-10-21): Float->Int saturates on every backend; `--release` without the llvm feature is a clear error on every driver; MIR emits explicit coercion instructions as part of F-29 (no backend decides coercions); the parity test runs in the slow pr-ready lane. Research and phases: `~/.agents/tmp/plans/ruxen-feat-concurrency-adoption-f29-backend-policy.md` (drifts: call-arg signedness, Float->Int, four JIT FFI/entry drifts, backend selection).
- [ ] **Batch 3 (rest of G):** F-29 backend lowering policy, F-30 one front-end driver, F-34 parser desugaring, F-35 IDE definition resolution, F-36 REPL replay, F-45 remaining stale docs.
  - F-30 includes a bug: `ruxen check` never runs the borrow checker, so it passes code that `ruxen build` rejects.
  - F-34 includes bugs: `case` re-evaluates its subject per branch; `x ||= y` evaluates `x` twice.
  - F-45 includes a bug: a bare call with a brace block and no block parameters (`run { puts("z") }`) does not parse as a call (E0726 on `run`); `run() { ... }`, `run do ... end` and `run { |x| ... }` do. (The release-e2e harness and `drop_fixtures` now run the borrow checker, I-20.)
  - F-36: if the slot redesign is too big for one pass, do the interim (rewrite the REPL walkers on the AST `Visit`; handle `MultiAssign`) and list the rest here.
- [ ] **Batch 3 addition: C headers by package name** (owner decision 2026-10-17, see Decisions). Build it with F-37 (`LinkInputs`, the `[runtime]` table) and F-30 (one front-end driver), so `ruxen build`, `ruxen test`, `ruxenc` and the test harness share one native-input path.
  - Manifest: `[native] include`, defaulting to `runtime/include` when present.
  - Build: `target/include/<pkg>/` for the dependency closure (stdlib as `std/<name>`), `-I target/include` on every C compile. Delete the PCRE2 `-I` special case in `codegen/object.rs` if the vendor tree can use the same mechanism.
  - Errors: an include of an undeclared package names the missing dependency; a relative include that leaves the package root fails with a `<pkg/header.h>` hint. Both get registry codes and `docs/errors` pages.
  - Migrate: move `library/std/core/runtime/runtime.h` to `library/std/core/runtime/include/`, and the 33 stdlib files from `#include "../../core/runtime/runtime.h"` to `#include <std/core/runtime.h>`. Check canvas, quiver and rondo for the same pattern.
  - Tests: a two-package fixture where package `b` includes `<a/a.h>` from a path dependency outside the workspace; an undeclared include fails; a `../` include fails.
- [ ] **Batch 3 addition: one generic over a mixin bound instead of per-type copies** (owner rule, 2026-10-16). Inventory, at HEAD 3892d60:
  - `.rx` only:
    - AtomicI64/AtomicUsize share one lib block (`mixin AtomicWord`).
    - Stdout/Stderr share `mixin StdStream`; `eprint`/`eprintln` become `alias`.
    - bufio `drop` moves into `mixin Reader`/`Writer`. Check the drop-class detection, which is by DefId since F-28.
    - MapStream, FilterStream and TakeStream `drop` move into `mixin StreamAdapter`.
    - Set and Hash share one `each`.
    - Array `min`/`max` share one private `extremum`.
  - Compiler:
    - Scalars and String `include Comparable, Hashable, PartialEq, Clone, Display, Default` in `.rx`.
    - Then delete the per-type Rust arms:
      - `derive.rs`: `synthesize_primitive_fmt_displays`, `format_field_for_debug`, `build_eq`/`build_hash_code`/`build_cmp`, `synthesize_clone_field`, `synthesize_default_value`
      - `interpolation.rs`: the `fmt_callee` ladder
      - `implicit_includes`: `ty_is_effectively_clone` / `ty_satisfies_named_trait`
      - both backends: the String `Compare` special case
    - Needs: `Float#hash_code`, Set/Hash `clone` declarations, conditional container impls (`Array[T]: PartialEq where T: PartialEq`), and `Formatter#precision` in `.rx`.
    - Fixes a latent bug: Array/Hash/Set fields of a derived class are compared and hashed by pointer.
  - Array-only `join`, `min`, `max`, `uniq`, `include?`, `sum` become `Enumerable` defaults with where-bounds, so Set and Hash get them.
  - Int `clamp`/`between?` come from `Comparable`; Char gains them.
  - The 11 async step futures: `mixin StepFuture` (needs associated types in mixin defaults).
  - Check whether `ArrayMatcher`, `StringMatcher`, `BoolMatcher` and `OptionMatcher` in std/test are unreachable (`Tester.expect` returns only `Matcher[T]`); delete them if so.
- **Running batch 3 on macOS (Apple Silicon):**
  - valgrind does not run there. Use `leaks --atExit -- <binary>`, or build with `-fsanitize=address` for the leak and double-free checks.
  - The stale-toolchain-copy sweep in `src/ruxenc/tests/support/installed_binary.rs` is Linux-only. Check that test temp dirs do not pile up in `CARGO_TARGET_TMPDIR`.
  - e2e still needs `cargo build --release --bins` and then `RUXEN_WORKSPACE=$PWD tests/release-e2e/run.sh`.
  - Do a first full run on a clean checkout to catch platform-specific failures, especially C runtime (`library/std/*/runtime/*.c`), linker flags and fixed-port fixtures, before starting the cards.

- [ ] **Ruby lambdas** (after batch 3; decision "Blocks, procs and lambdas"; ADR `docs/decisions/blocks-procs-lambdas.md`):
  - Counted shared captures, so procs and lambdas share a variable without `.clone`, plus a cycle rule (compile error or weak capture) so they still cannot leak.
  - A proc's `return` leaves the enclosing method; a proc called with fewer arguments than it has parameters (the missing ones are `nil`; taking fewer parameters than the `Fn` type is done); `.call`, `[]` and `lambda?`.
  - `lambda`/`proc` followed by `{` or `do` should call a user method of that name when one is defined.

### Debt added or found by the blocks/fields/`&var` merge (2026-10-17)

- [x] `.clone` of a Proc was recognised by name (`method_name == "clone"` in typeck, I-19). It is now the `.rx` method `Closure#clone` (`library/std/core/src/lib.rx`, `class Closure`), a lib method over the C `ruxen_closure_share` (`core/runtime/alloc.c`), declared `-> owned Self`: the generic owned-result marker of the lib grammar (`FfiFunction::returns_owned`, read by `runtime_abi::result_ownership` before the table), so its `FRESH_ALLOC_CALLEES` row is gone (review I-19). The other rows move to the marker in F-25's own card. `Ty::method_home` of a Proc is the class the core library declares `layout closure` (`SymbolTable::closure_shell`, review round 3 I-19: no `"Closure"` spelling in Rust); typeck selects it like any method and maps the shell's `Self` back to the receiver (`collect.rs` `replace_shell_class`); MIR calls it because typeck resolved a method of that declared class (`method_call.rs`, `closure_shell_method`). `Ty::proc_of` is the one constructor of `Proc[...]` (I-32). `HirExprKind::ProcClone`, `lower_proc_clone`, the MIR `__ruxen_closure_share` helper and `MirFnRole::ClosureShare` are deleted.
- [x] Synthesized-name protocols matched by string (I-19): closure bodies carry `MirFnRole::ClosureBody`; field glue carries `MirFnRole::FieldGlue` and every glue call is recorded where `heap_free_call` emits it (`MirFunction::field_glue_calls`, callee → object type), which `is_field_glue_call` and `synthesize_field_drop_glue` read instead of rebuilding the name; the closure drop helper carries `MirFnRole::ClosureDrop`; borrow-check diagnostics read `SymbolTable::display_name` (`__block` shows as `block`) instead of comparing the name.
- [ ] Still by name, pre-existing (found while doing I-19): element glue recovered its type by slicing its own name (fixed 2026-10-24: `MirFunction::elem_glue_types`, written by `drops::glue::record_elem_glue`), and `demote_escaping_element_drops` finds a closure's body from its drop function by `strip_suffix("_drop")` (`drops/flow.rs`). Record the type and the body when they are synthesized, as `field_glue_calls` does.
- [ ] `SymbolTable::class_owns_fields` excludes async state machines by the field name `"__state"`; record it as a flag on `ClassInfo` when async lowering synthesizes the class. (Pre-existing.)
- [x] Two sources for "a `Fn[...]` parameter is a borrow": `runtime_abi::user_callee_param_is_ref` now takes the callee's lowered block slots (`runtime_abi.rs:149`, `CallSummaries::block_params`) and is the one reader; it falls back to the declared signature only for callers that run before the callee is lowered (`fn_call.rs`, the escape walks). `CallSummaries::takes_block` is deleted.
- [x] Two sources for "an owned value passed to C moves": the `arg_transfer_mask` rows and `ArgMask` are deleted; every reader (drop elaboration, the escape walks, element flow, the loop-local frees in `emit.rs`) asks `drops::FfiByValue` (`drops.rs:924`), built from the `.rx` declarations by `runtime_abi::ffi_param_takes_value` (`runtime_abi.rs:513`). `runtime_abi_parity::oracle_transfers_are_by_value_in_the_rx_declarations` proves every old row is derivable.
- [ ] `has_runtime_prefix` / `collection_family` classify callees by name prefix (`Array[`, `String_`), which marked `Array[Int]_tap` (a `.rx` body) as a borrowing runtime helper. Classify by whether the callee is an FFI declaration. (Pre-existing.)
- [ ] `FRESH_ALLOC_CALLEES` still lists the `Command` builders (`ruxen_command_arg`/`args`/`env`/`current_dir`), which return their receiver, with `COMMAND_BUILDER` compensating. Make them `.rx` bodies returning `self`, as done for `OpenOptions`, and delete both rows. (Pre-existing.)
- [x] A derived `clone` was recognised by its mangled name (`is_derived_clone_call`, deleted). A derived class `clone` now carries `MirFnRole::DerivedClone` (`derive.rs`), and `CallSummaries::record_roles` puts it in `fresh` and `reads_argument` beside the closure share (`drops.rs:980`, read at `drops.rs:3810`).
- [x] `Lowerer::mutates_slot_receiver` (`method_call.rs:1000`) applies the `&var` slot rule through `refmut_is_slot_address` alone; `emit_rebinding_self_call` handles a scalar receiver (a plain copy into the slot). No stdlib type declares a scalar `def var` lib method today and a user cannot (E0727), so only the String path is exercised.
- [x] `Proc[(A) -> R]` was special-cased by name in the parser, formatter and resolver. `Proc` is now `type Proc[F] = any Fn[F]` in `library/std/core/src/lib.rx:64`, through a new generic type-alias mechanism (`resolve/helpers.rs:49` `resolve_alias_target`, `resolve/types.rs:65` `instantiate_alias`); the bare signature `(A) -> R` is a generic argument of any named type (`parser/types.rs:340` `at_signature`, `FnTypeSpelling::Signature`, printed as written by `formatter/format_type.rs:81`). Pins: e2e 2218, `blocks_procs_lambdas::syntax_signature_generic_arg_works_for_any_alias`.
- [ ] Floor, documented: `Proc.new { }` is recognised by name in the parser (`parser/expr/calls.rs:185`), like `proc { }` and `lambda { }`: each is closure-literal syntax whose kind (proc or lambda arity) no `.rx` method can produce. `ClosureSpelling` keeps it lossless; no later stage reads the name.
- [ ] Floor, documented: the borrow checker reads the method name `call` (`borrow_check/checks.rs:413`, `:579`) because resolve lowers every callable-value call (`f.(x)`, `f.call(x)`, `yield`) to a `MethodCall` named `call`, which typeck and MIR also read (pre-existing protocol). Retire it with a distinct HIR node for calling a callable value.
- [x] Place loans (I-15, F-1, `borrow_check/places.rs`) are one provenance rule: loans carry a field path (writing `u.age` while `x = u.name` is live is fine), follow every value shape (`if`/`case` arms, block tails, casts, multi-assign values, pattern bindings, `for` over a field, a closure holding a borrow lives as long as the closure), and every write, `var self` receiver, `&var` argument (I-24: also exclusive for its call) and move checks them. A never-used borrowing binding holds nothing (I-25). A `&var` String/scalar argument with no address is E1021 (I-27). Adversarial matrix: `tmp/test-cache/loan-matrix/gen.py` (242 programs).
- [x] Review round 3 structural fixes (2026-10-17): one provenance query over every sub-expression whose type holds a reference (aggregates, constructor arguments, closures reaching their borrowed captures; I-29); evaluated operands of calls, operators and literals held while the later ones run (I-30); free-function trailing blocks checked like method blocks (I-31); a `var self` receiver excludes its place from the other operands; every `&var` place is the place's own storage (`MirInst::FieldAddr`, block captures by address, owned capture slots; I-24, P-11), the writeback paths deleted; `op=` on an Array element through its slot, an error elsewhere (P-12). Matrix: `gen.py` + `gen2.py`, 485 programs, results `tmp/test-cache/loan-matrix/classify-r3.txt`.
- [ ] Leaks the round-3 matrix still shows, all pre-existing families (no crash, no wrong output): a tuple box was never freed (fixed 2026-10-24, tuple ownership: `drops::heap_free_callee` has a `Tuple` arm and tuples get field glue, so a tuple frees its box and the elements it owns); an `Option` box built by `Some(..)` and never matched leaks (see the Option/Result item under Known issues); a lambda holding a borrow of a field plus a by-value move of the object (`y = u.name; f = -> { puts(y) }; f.(); eat(u)`, 8 blocks, also in the round-2 matrix). A lambda or proc that assigns a captured Int still shares a heap cell with the enclosing frame that nobody frees (16 bytes).
- [ ] Leak-sweep round (owner, 2026-10-24: its own round, after tuple ownership). Five pre-existing leak classes, each hitting Strings and now also tuples:
  - A loop body-local is freed only at scope exit, so only the last iteration's value is freed (also a tuple built per iteration, `t = (p, i)` in a `while`).
  - A by-value parameter is never freed by the callee.
  - A non-fresh Array result leaks (`zip`, the `Array#clone` spine).
  - An escape-demoted container drop is shallow (a hash after `to_a` views it).
  - A hash literal drops shallow.
- [x] (fixed 2026-10-24: the call keeps the inferred type when the declared return names a type parameter; it/method_call_inference, e2e 2244) A generic METHOD call is mistyped at the call site, even a single one: `r.ident("only-s")` (`def ident[T](x: T) -> T`) prints an address, and `r.ident(42)` then `r.ident("word")` types the second call `Int`. The per-instance bodies are right (`Reader_ident__mono__String`); the call's result type is wrong. Pre-existing (reproduced on 9cee2056, 2026-10-24; P-16 corrected the scope); e2e 2244 calls its generic method with one type only until this is fixed.
- [x] (fixed 2026-10-24: constructor arguments bind class parameters through nested and compound types; it/method_call_inference) A generic class constructor whose argument is itself generic or compound (`Outer.new(Wrap.new(3))`, `Table.new([("a", 6)])`) leaves the class parameter unresolved; a later use binds it (`puts` makes it `&String`), so an `Outer[&String]` holding an Int crashes. Pre-existing (base 9cee2056, a0b982c8; review P-13, `typeck/infer/expr.rs`).
- [x] (fixed 2026-10-24: the shadowing is error E0745; it/method_call_inference) A method generic that shadows its class generic (`class Box[T]; def show[T: Clone](x: &T) -> T`) is typed as the class `T`, so `b.show(&s)` on `Box[NoClone]` reads a String as `NoClone`. Pre-existing (base 9cee2056, a0b982c8; review P-14, `typeck/infer/expr.rs`).
- [ ] `"#{h:?}"` of a class with a C-handle field (`class H; m: Mutex[Int]`, or `Box[Mutex[Int]]`) prints the object's address instead of `H { m: <...> }`: since I-57 a C handle supports no derived mixin as a field, so the container gets no Debug body, although the Debug builder renders a field it has no body for as `<...>`. The installed pre-round-3 toolchain prints `H { m: <...> }`. Needs an owner decision: a per-derive field rule in the derive table (grows `SUPPORTED_DERIVES`) or the F-24 `inspect` default (Decisions, Printing). Found 2026-10-24 (probe e03).
- [ ] `h = Hash.new` then `h[K.new(1)] = v` leaves the key type an inference variable (typeck), so the table gets no key descriptor and the insert panics "no key descriptor for this table (B73)". The installed pre-batch toolchain panics the same way; `{K.new(1) => v}` or a typed parameter works. Found 2026-10-24.
- [ ] A subclass without its own `init` (`class Kid < Base`, `Kid.new(P.new(1))`) fails to link: `_Kid_init` is undefined. Pre-existing (base 9cee2056, a0b982c8; review P-15, `mir/lower`).
- [ ] A `Mutex` local leaks its C object (`m = Mutex.new(1); g = m.lock_raw; puts(g.get)`: 1 leak, 96 bytes, `ruxen_mutex_new`). Pre-existing (reproduced on 9cee2056, 2026-10-24); belongs with the leak-sweep round.
- [ ] Audit the other FFI `-> T` getters that return an alias without a copy (tuple ownership fixed `ruxen_hash_from_iter_keyed`; `SharedSync#get`/`MutexGuard#get` move to `.rx` clone).
- [ ] The closure captures box (typed `Int`) is recorded as `View::Scalar` in `MirFunction::views` (false but harmless today).
- [ ] The three residual C names added in steps 2-3 (`ruxen_vec_get_or_panic`, `ruxen_vec_skip_elem`, `ruxen_vec_chain_elem` in `mir/residual_abi.rs`) are a grown C-symbol table; their home is a `.rx` lib declaration.
- [x] Debt added by tuple ownership: `pair_value_desc` (`mir/lower/key_desc.rs`) checked the callee name `ruxen_hash_from_iter`. Deleted in review round 2: `Array#to_h` declares `pairs: ElemDesc[T]` and `key_desc::append_declared_elem_descs` fills any `lib` parameter of that type.
- [ ] The `to_h` resolver arm in `typeck/method_resolvers/collections.rs` and the key-descriptor name table `KEYED_TABLE_CTORS` (`mir/lower/key_desc.rs`) remain (pre-existing debt).
- [ ] A method returning `&T` borrows its whole receiver (no field path; MIR: a call result points `INSIDE` its argument). (The HIR `LastUses` loop false positive went with the HIR loan code: MIR liveness accepts `x = u.name; puts x; u.name = ..` rebound each iteration of a loop.)
- [ ] `u.items[0]` (an index right after a field or method name) parses as a method call with generic argument `0`, so it fails typeck ("no field or method `items`"); `its = u.items; its[0]` works. Pre-existing (HEAD too). The parser should take `[` after a member name as an index unless the method declares type parameters.
- [ ] A local moved into an owning lambda and then reassigned (`f = -> { puts(u.name) }; u = U.new(..)`) never frees its new value: the local is tainted as a whole by the move (2 leaks, 48 bytes). Needs per-definition ownership in drop elaboration. Pre-existing.
- [ ] A function returning an `Array` (or `Hash`) of heap elements is never caller-owned (`returns_fresh` accepts a container only when its elements own no heap), so `a = names(3)` and `names(3)[0]` leak the result (review probe g7, 3 blocks; HEAD too). Needs element provenance: a spine whose every element was a fresh owned value moved in.
- [ ] e2e 616 (`Box` holding `Array[Proc]` built through `&var *self`) leaks 5 blocks under `leaks --atExit`; not triaged.
- [ ] Observed while pinning e2e 2218, not yet triaged against the pre-batch compiler: a closure passed BY VALUE to a user `Proc[...]` parameter leaks its pair (48 bytes), and an owned `String` passed by value to a user `String` parameter leaks it (16 bytes). Repro: `def fire(cb: Proc[(String) -> nil], word: String); cb.(word); end` then `fire(->(s: String) { puts s }, "hi")` under `leaks --atExit`. A callee never drops its own parameters, so a by-value parameter needs the callee to free it.

## Then: Ruby parity (priority 2)

Order from [the Rust-isms audit](docs/audits/2026-10-rust-isms.md), section 6. Rust forms go: add the Ruby form, make `ruxen fmt` print it, rewrite the code, then warn, then error.

- [ ] **Ruby project layout** (decision, owner raised 2026-10-17: the package folder structure is not Ruby-like). Today a package is `Ruxen.toml`, `src/lib.rx` or `src/main.rx`, and `tests/*.rx` (`src/ruxen_cli/src/scaffold.rs`). Proposal, modelled on a Ruby gem:
  - `lib/<name>.rx` is the package entry (replaces `src/lib.rx`); sub-files go in `lib/<name>/` and load with `require`/`require_relative`.
  - `bin/<name>` holds executables as top-level scripts (replaces `src/main.rx`); a package may have several.
  - `spec/*_spec.rx` holds tests, because the stdlib test DSL is RSpec-style `describe`/`it` (`test/*_test.rx` would fit a Minitest-style DSL instead).
  - `Ruxen.toml` stays as the manifest (one file playing the role of the gemspec plus Gemfile).
  - Migration: `ruxen new`, `ruxen build`/`test`/`run` discovery, `module_discovery.rs`, the stdlib packages under `library/std/*`, quiver, rondo and canvas, and the docs and tutorial. Old layouts keep working with a deprecation warning for one release.
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

- [ ] Left from the `&var`/temporaries fix (2026-10-17): a closure parameter typed `&var Int`/`&var String` and `Array#get_mut`'s `Option[&var T]` payload still carry a copy, so writes through them are lost; a function returning a `&var` of a scalar or String is refused at MIR lowering; a `&var` handle (class, array) stays the handle, and rebinding such a parameter is now E1020; enum temporaries and temporaries of multi-block statements keep the function-exit drop; results of functions returning a container of heap elements, a field or a parameter are never freed (pre-existing).
- [ ] `Array#sort_by` sorts in place and returns `nil`. Ruby returns a new array, so make it match and add `sort_by!`.
- [ ] Inside a stdlib method, `self.clone` assigned to an `Array[T]` variable fails typeck: "expected `Array[T]`, found `Array[T]`".
- [ ] The installed toolchain (`~/.ruxen/bin/ruxen`) is from June and cannot parse newer fixtures. Run `ruxen upgrade --from-source ~/Documents/ruxen-lang/ruxen` once this branch lands.
- [ ] Unconfirmed: on the first run, 4 `ruxen test` dependency-linking tests failed (`consumer_test_runs_dep_generic_over_two_implementors`, `library_test_sees_dependency_symbol_and_runs`, `non_ffi_dep_test_still_links`, `test_links_flat_merged_ffi_dep_runtime`). They may have been environmental, so recheck them.
- [ ] `docs/specs/syntax/ruby-naming.spec.md` appendix A lists things not yet verified on a fresh binary: the `super` lookup order, `.call` on a stored block, hex/binary/octal literals, and `protected`.
- [ ] `typeck::type_check_with_bootstrap` (the non-package bootstrap path) is used only by 4 integration tests (`bootstrap_class_bodies`, `bootstrap_prelude_merge`, `no_std_e1400`, `wasm_codegen`). Port them to the package-aware path, then delete it. It cannot go behind `cfg(test)` because integration tests do not see `cfg(test)` items.
- [ ] The `ruxen_driver` crate is deleted on this branch. Drop its mentions in `../CLAUDE.md` and `ruxen/CLAUDE.md` (both git-ignored, outside this worktree) when the branch merges.
- [x] **High:** a guard from `Mutex#lock!` taken in a helper or as a temporary is never released, so the next lock deadlocks. Fixed 2026-10-17: statement temporaries drop at statement end and fresh call results are caller-owned (e2e 2210, 2215; `drop_fixtures::drop_class_statement_temporary_drops_at_statement_end`).
- [x] **High:** reading a `&var String` (interpolation, `.size`, passing it to a `&String` parameter) treats the slot address as the string. Reads through `MutexGuard#get` are fine. Fixed 2026-10-17: every read loads through the address (e2e 2211; `drop_fixtures::var_string_param_writes_neither_leak_nor_double_free`).
- [x] **High:** a scalar `&var` (Int, Float, Bool) is passed by value, so a write through it never reaches the caller. This includes `MutexGuard#deref_mut` on `Mutex[Int]`. Fixed 2026-10-17: a scalar `&var` is the caller's address (e2e 2212, 2213).
- [x] (fixed in 9cee205, re-checked 2026-10-17 on a clean HEAD build) Interpolating a `&Float`, `&var Float` or `&var Bool` prints the raw machine word, not the value.
- [ ] A local named `args` is shadowed by the global `args` function.
- [ ] A receiver passed to a mixin default or `Kernel` method (`Bag.new.walk { }`, `Point.new(1).then { }`) is never freed when it is a temporary: `runtime_abi::user_callee_param_is_ref` finds no candidate for the mangled `Class_method` callee (a mixin default is a parentless `DefKind::Function`), so the receiver slot is tainted. The closure pair passed to such a method is now freed (`drops::CallSummaries::block_params` reads the block parameters off the lowered body, 2026-10-17); the receiver needs the callee's `self` mode recorded the same way (a typed field on `MirFunction`, not a name lookup). Repro: `mixin Walk; def walk(f: Fn(Int) -> nil) -> nil; f.(1); end; end`, `class Bag include Walk`, `Bag.new.walk { |n| puts(n.to_s) }` under `leaks --atExit` (1 leak, the Bag).
- [ ] An `Option` / `Result` box is freed only when a by-value `match` binding empties it (`MirPayloadMove`): a local matched with `Some(_)`, an `Option` field, and the `Result` a `.expect!` consumes each leak their box (32 bytes). Repro: `drop_fixtures::match_on_a_place_keeps_its_payload` (2 leaks under `leaks --atExit`).
- [ ] The cell of a variable a closure captures by reference is never freed. Fixed for blocks (2026-10-17: a block borrows a String or scalar capture by the enclosing variable's address, no cell) and for a String a proc or lambda owns (its own slot); still open for a lambda or proc assigning a captured scalar (`n = 10; f = -> { n += 1 }`, 16 bytes) or a handle. The enclosing frame should free it at scope exit unless an owning closure that captured it escapes (`drops::closure_pair_escapes`).
- [ ] `Mutex.new` and `MutexGuard#set` take ownership of the payload and never free it (needs typed drop).
- [ ] `String#[]` builds the char array on every call, aborts on a negative index and returns `Char`, not a `String`.
- [ ] E1017 calls a type alias or newtype a `class`, and rejects an alias of a mixin.
- [ ] CHANGELOG [Unreleased] overstates a few things: Channel and "stdlib extensions", and "the formatter round-trips" (it still drops some comments). It also lacks Removed and Added entries.
- [ ] `Option.Some(f.await)` (an `.await` inside an enum-variant constructor argument) typechecks but fails to link: `Ready_await` and `Future_poll` are undefined. Covered only by the Rust test using `fixtures/ruxen/async_await_in_enum_variant.rx`; add an e2e case once it links.
- [x] (fixed in 9cee205, re-checked 2026-10-17 on a clean HEAD build) `hash_code` on `Int` and `String` through a `Hashable` generic bound typechecks but fails to link (`Int_hash_code`, `String_hash_code` undefined). No test covers this gap: the never-run `#[ignore]` pin was deleted. Repro: `def hash_it[T: Hashable](a: &T) -> Int` returning `a.hash_code`, called with `&42` and `&"hello"`.
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
- [x] (fixed in 9cee205, re-checked 2026-10-17 on a clean HEAD build) A parenless `e.message` or `e.kind` on an `IoError` value is rejected as "cannot access field `message` on enum `IoError`"; only `e.message()` works. Ruby parity wants the parenless call.
- [ ] The `std::net` blocking echo server with a SIGINT shutdown stays a Rust test (`stdlib_net.rs`) because a `.rx` test cannot signal its own blocked `accept`. Move it once `ruxen test` can drive a child process.
- [ ] A class whose implicit init (from its fields) is called only inside a block fails to link (`Point_init` undefined). `library/std/fmt/tests/fmt_test.rx` gives `Point` an explicit init to work around it.
- [x] (fixed in 9cee205, re-checked 2026-10-17 on a clean HEAD build) `.unwrap` or `.nil?` on an `Option[C]` returned by a stdlib method, where `C` is a class (`JSON.object_get`, `Regex#find`, `Mutex#try_lock`), fails typeck with "no field `unwrap` on type `Json`". `.unwrap_or` works.
- [x] (fixed in 9cee205, re-checked 2026-10-17 on a clean HEAD build) `Some(3) == Some(3)` is `false`: Option equality compares pointers, so `t.expect(opt).to_eq(Some(x))` always fails.
- [ ] `Formatter#write_str` and `#write_char` return a null pointer for success, so `.ok?` on their result is `false`.
- [ ] `.await` on a hand-written `include Future` class passed as an `async def` parameter fails to link (`CountDown_await`, `Future_poll` undefined).
- [ ] `"#{x:?}"` ignores a user `include Debug` `fmt` and prints the derived `Tag { name: x }` form. Check whether that is intended (`fmt.spec.md` B2).
- [ ] `t.expect(x).to_eq(y)` compares pointers when `x` is an `Array[String]`, an `Option`, or a `String` from a destructured tuple (`let (evens, odds) = v.partition { ... }`, then `evens.join(",")`), so equal values fail. The std package tests compare `join` strings or bind a typed `let s: String`.
- [x] A block that assigns a captured `var` without reading it (`last = x` inside `[1, 2].each do |x| ... end`) was rejected with E1009. It compiles and prints the last value (rechecked 2026-10-17).
- [x] (fixed in 9cee205, re-checked 2026-10-17 on a clean HEAD build) A class with its own `def hash_code` fails codegen once used as a `Hash` key: `DuplicateDefinition("Ticket_hash_code")`.
- [x] (fixed in 9cee205, re-checked 2026-10-17 on a clean HEAD build) A parenless `a.hash_code` on a class with derived `Hashable` is rejected as "no field `hash_code`"; only `a.hash_code()` works.
- [x] (fixed in 9cee205, re-checked 2026-10-17 on a clean HEAD build) `Array.from(123)` is accepted at typeck and fails only at link time (the old `vec_from_int_is_currently_accepted_at_typeck` pin), and `Array.with_capacity` does not check that its argument is an `Int`.
- [ ] `ruxen test` has no stdout capture, so Kernel `p` / `pp` (e2e 1180) and `warn` (e2e 1181) stay e2e fixtures instead of `library/std/core/tests`.
- [ ] `case opt in nil ... in x` on an `Option[Int]` binds `x` to the Option pointer, not the payload, so `puts "#{x}"` prints an address (the ruby-naming spec marks this form [implemented]). `in Some(x)` works; e2e 2079 uses it.
- [ ] `Task.join(h).await` inside `block_on` hangs: the deferred e2e cases `728_async_spawn_join_basic` and `728c_async_spawn_loop` (removed; never run) compiled but did not finish within 10 s. Add an e2e case once spawned tasks join.
- [ ] Stdlib top-level helpers share the user's global namespace, and `private` is not enforced: a user `def connect_state_to_result(x: Int)` replaces the stdlib helper inside stdlib bodies. Resolve stdlib bodies in their own package scope first, or enforce `private`; add a negative fixture.
- [ ] Async I/O now classifies errno through the shared sync table: EBADF gives `Other("Bad file descriptor")` where async used to give `InvalidInput`.
- [ ] `cargo fmt --all -- --check` failed on committed files before batch 1 (the CI fmt gate); batch 1 ran `cargo fmt --all`.
- [x] **High:** `BufWriter.File#into_inner` and `BufWriter.Tcp#into_inner` returned the boxed `Result` from `ruxen_bufwriter_into_inner`, so the handle pointed at fd 0. Fixed 2026-10-17 without changing the public signature: `ruxen_bufwriter_into_inner_or_panic` flushes, returns the bare handle and panics on a flush error (e2e 2214, bufio stdlib tests).
- [ ] `ruxen fmt --stdin` prints diagnostics but exits 0 on a parse error or an unplaceable comment, so editors cannot tell formatting was refused. Exit 1 and keep echoing the input.
- [ ] `compiler/ruxen_core/build.rs` reruns only on edits in existing packages' `src/`; a new `library/std/<pkg>/src/lib.rx` is left out of the generated bootstrap tables until something else reruns it. Add a test that compares the package set on disk with `BOOTSTRAP_FILES`.
- [ ] No `CONTEXT.md` yet for lexer, typeck, mir/lower, borrow_check, async_lowering, diagnostics and resolve, or for the io, net and async_net runtime dirs and the `src/` crates.
- [ ] A class body's `include Send`/`include !Send` always overrides the same declaration in an `extension` of that class, whatever the source order (body `include !Send` plus `extension ... include Send` stays not Send). Decide the rule (last wins, or E1014 on a conflict) and pin it with a test.
