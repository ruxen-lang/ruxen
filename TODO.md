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
- [ ] **Batch 2: checkpoint, to finish on the Mac.** It was stopped at a checkpoint (2026-10-17), amended into the one commit and pushed at the owner's request. The combined-tree gate was stopped before it finished.
  - The last full gate passed on the tree without the CI-fix agent's link and runner changes: e2e 433/433, core lib 675, `it` 474, all stdlib packages, fmt and clippy clean.
  - The link and runner changes were checked separately: `it` 474 with `--as-needed` forced, all stdlib packages, the `codegen::object` tests, fmt and clippy.
  - So run the full gate once on the Mac first.
  1. Fix the macOS `sync` double free (CI section below). Then make CI green: Ubuntu, macOS and Lint on the push.
  2. Fix the `Mutex#lock` 8-byte overrun (CI section below).
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
  - F-19 (decision): operators as method calls. Rewriting `a == b` to a call made by-value operator params move their operand. Choose between auto-borrowing operator operands and `&` operator params in the stdlib. `&String <=> &String` is still E0707.
  - F-20: `MirInst::Call` structured callee (about 154 sites), so callee strings are still re-parsed in MIR and codegen. Extension methods on builtin heads are registered as free functions, so their parenless overloads are still wrong; register them as methods of the shell class. Not recorded for mixin defaults and `.new`.
  - F-21: generic-calling-generic across classes (`Outer[T]` holding `Holder[T]`) is not monomorphised; it needs a worklist. `unique_bound_impl` still guesses.
  - F-22 (decision): Option and Result declared in `.rx` need stdlib enums to export bare variants. E0722 cannot be re-tightened while Array `get`/`get_mut` share one C symbol with different surface types.
  - F-23: `def var` on a top-level lib function is silently ignored. Mutating through `&String` runs on a copy.
  - F-24 (decision): the `Int_fmt` fallback for Array, Option, Hash and type params (needs Display formatters). Primitive fmt via `.rx` `include Display` (needs `Formatter#precision` in `.rx`). `to_s` as a Kernel default. IO_TEXT_BUILTINS. Displayable, Display and Debug as one protocol. User classes have no `inspect`.
  - F-25: the Mutex payload leak (`get` returns an alias and `into_inner` does not consume). Ownership markers in the lib grammar.
  - F-27 (decision): Ord/PartialOrd/PartialEq into Comparable. A derived `eq(other: &Self)` is treated as a move by the borrow checker. Enums have no derived bodies. Bool, Char, Float and the collections have no `hash_code`.
  - F-28: the `__state`/`__sub_` checks need async lowering to mark generated classes. Four codegen sites still test `name == "main"`. `include Into[X]` loses its generic argument in HIR.
  - F-41: TypeParam against a concrete type is still lenient (`Array#sum`'s `var total: T = 0`, user generic statics, `Self::Output` as a TypeParam).
  - Leaks (valgrind on the scratch program: 778 bytes definitely lost before, 0 after). Fixed: class elements in Array and Hash values (per-type glue descriptor `RUXEN_ELEM_OBJ`), interpolation temporaries, and String rebinding in loops and mono bodies. Still open:
    - (decision) A class's own String or container fields are never freed. It needs a per-field drop rule, and fields alias freely today.
    - `Set[Class]` elements and `Hash[Class, _]` keys (needs key copying through the glue in `hash.c`). Generic classes and enums as elements.
    - A value returned by a user function, and a by-value argument to one (the F-25 ownership-ABI gap).
    - The element-escape guard is conservative: a container passed to a closure-taking `.rx` method (`map`, `select`) is made shallow, so its elements leak.
    - `Array#drop(n)` and `+` copy class elements with `<T>_clone`, so a class's `def drop` also runs on the copies.
  - Pre-existing, found while proving: `if let Some(r) = v.find { }` on a class array segfaults; `v.count { }` has no block overload. fmt still drops grouping parens, alignment padding and `"""` heredocs.
- [ ] **Batch 3 (rest of G):** F-29 backend lowering policy, F-30 one front-end driver, F-34 parser desugaring, F-35 IDE definition resolution, F-36 REPL replay, F-45 remaining stale docs.
  - F-30 includes a bug: `ruxen check` never runs the borrow checker, so it passes code that `ruxen build` rejects.
  - F-34 includes bugs: `case` re-evaluates its subject per branch; `x ||= y` evaluates `x` twice.
  - F-36: if the slot redesign is too big for one pass, do the interim (rewrite the REPL walkers on the AST `Visit`; handle `MultiAssign`) and list the rest here.
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
- [ ] **High:** a guard from `Mutex#lock!` taken in a helper or as a temporary is never released, so the next lock deadlocks.
- [ ] **High:** reading a `&var String` (interpolation, `.size`, passing it to a `&String` parameter) treats the slot address as the string. Reads through `MutexGuard#get` are fine.
- [ ] **High:** a scalar `&var` (Int, Float, Bool) is passed by value, so a write through it never reaches the caller. This includes `MutexGuard#deref_mut` on `Mutex[Int]`.
- [ ] Interpolating a `&Float`, `&var Float` or `&var Bool` prints the raw machine word, not the value.
- [ ] A local named `args` is shadowed by the global `args` function.
- [ ] `Mutex.new` and `MutexGuard#set` take ownership of the payload and never free it (needs typed drop).
- [ ] `String#[]` builds the char array on every call, aborts on a negative index and returns `Char`, not a `String`.
- [ ] E1017 calls a type alias or newtype a `class`, and rejects an alias of a mixin.
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
- [ ] Stdlib top-level helpers share the user's global namespace, and `private` is not enforced: a user `def connect_state_to_result(x: Int)` replaces the stdlib helper inside stdlib bodies. Resolve stdlib bodies in their own package scope first, or enforce `private`; add a negative fixture.
- [ ] Async I/O now classifies errno through the shared sync table: EBADF gives `Other("Bad file descriptor")` where async used to give `InvalidInput`.
- [ ] `cargo fmt --all -- --check` failed on committed files before batch 1 (the CI fmt gate); batch 1 ran `cargo fmt --all`.
- [ ] **High:** `BufWriter.File#into_inner` and `BufWriter.Tcp#into_inner` are declared `-> File`/`-> TcpStream`, but `ruxen_bufwriter_into_inner` (bufio.c) returns a boxed `Result[W, IoError]`, so the returned handle points at fd 0 and later writes go nowhere. Declare `-> Result[File, IoError]` (a flush can fail) or add a C entry point that returns the bare inner; this changes a public signature (decision).
- [ ] `ruxen fmt --stdin` prints diagnostics but exits 0 on a parse error or an unplaceable comment, so editors cannot tell formatting was refused. Exit 1 and keep echoing the input.
- [ ] `compiler/ruxen_core/build.rs` reruns only on edits in existing packages' `src/`; a new `library/std/<pkg>/src/lib.rx` is left out of the generated bootstrap tables until something else reruns it. Add a test that compares the package set on disk with `BOOTSTRAP_FILES`.
- [ ] No `CONTEXT.md` yet for lexer, typeck, mir/lower, borrow_check, async_lowering, diagnostics and resolve, or for the io, net and async_net runtime dirs and the `src/` crates.
- [ ] A class body's `include Send`/`include !Send` always overrides the same declaration in an `extension` of that class, whatever the source order (body `include !Send` plus `extension ... include Send` stays not Send). Decide the rule (last wins, or E1014 on a conflict) and pin it with a test.
