# Ruxen compiler code-debt audit: synthesis

Audited checkout: `/home/sheraz/Documents/ruxen-lang/ruxen-concurrency-adoption`. It is on branch `feat/concurrency-adoption` at `65aab1c`, not `master`, and the audit covers the whole repo in that checkout. I edited nothing and built nothing.

The installed `~/.ruxen/bin/ruxen` is older than this source: it fails to parse fixture 965. So I could not reproduce any runtime claim, and every "verified" below means verified by reading the code.

**Path shorthand:** `core/` is `compiler/ruxen_core/src/`, `std/` is `library/std/`, `bin/` is `src/`. **(V)** means I re-read that instance; unmarked instances are as the auditor reported them and not re-checked.

## Summary

The eight auditors sent about 150 candidate findings. After merging shared root causes they form **45 families**.

| Kind | Families |
|---|---|
| release-blocker | 9 |
| non-generic | 9 |
| duplicate | 19 |
| unneeded | 3 |
| other-debt | 5 |

| Severity | Families |
|---|---|
| high | 15 |
| medium | 28 |
| low | 2 |

Main cross-area root causes:
- **Missing shared infrastructure.** HIR has no visitor, `SymbolTable` has no name index, and the canonical `Ty` helpers (`peel_refs`, `map_inner`) exist but are mostly unused.
- **The HIR throws away resolution results.** Safe navigation (`x?.field`), calls written without parentheses, operators, and the method a call resolved to are all lost. MIR, the borrow checker and the IDE then re-derive them from names.
- **Builtin and ownership knowledge lives in Rust or C string tables** instead of the `.rx` `lib` declarations. That breaks contract 1 in ruxen/CLAUDE.md.
- **Code is copied between twins:** Cranelift and LLVM, batch and REPL JIT, `ruxenc`/`ruxen_cli`/REPL/IDE drivers, and the C runtime packages.

Every high and medium claim I re-checked (two or more instances per family) held up. No candidate was dropped as factually wrong. The drops below are scope or design decisions, plus one downgrade and one recount.

## Release blockers

| # | Problem | Family | Status |
|---|---|---|---|
| 1 | Silent stubs reach user code: `ruxen_result_try_op` and `unwrap_or_else` return 0, `super` lowers to a no-op, `block_on`'s `.rx` body returns its argument, the REPL links weak no-op `Future_dynamic_poll`/`Drop_dynamic_drop`, and index lowering emits nothing for String, dynamic FixedArray and Tuple | F-2 | V |
| 2 | `Array#include?`, `sort`, `dedup`, `sum` and `==`, and `Hash#==`, compare or add raw 64-bit words for any `T`. String arrays and `Array[Float]` give wrong answers silently | F-4 | V |
| 3 | `ruxen fmt` corrupts code: `#{a <=> b}` comes out as `#{aSpaceshipb}`, and it emits retired `@[repr]`, `@[link]` and `derive` syntax that no longer parses | F-5 | V |
| 4 | Typeck types about 14 concurrency methods that have no body or C symbol. Calls pass typeck and fail at link time | F-10 | V |
| 5 | A user class with a method named `each`/`map`/`find`/`filter`/`position`/`partition` is miscompiled: MIR reads field 0 as a Vec instead of calling the user's method body | F-21 | V. Interim fix: delete the user-class inlining path |
| 6 | Async: `read_line` registers the fd without attaching its waker (likely lost wakeup), and every C-side Poll block leaks ("accept the leak for now"), which grows without limit on a long-running server | F-6 | V |
| 7 | LLVM backend ignores `no_std` and always calls `ruxen_env_init`, so no_std LLVM builds should fail to link. Cranelift lowers Float `%` with integer `srem` | F-7 | V |
| 8 | Regex, Match and RegexError drop helpers are never declared, so every regex object leaks | F-8 | V |
| 9 | Test fixture packages and symbols (`bootstrap_smoke`, `_pin_zero_rust_stdlib`, `test_extern.c`, `foobar.c`, a `printf` in `stream.c`) ship in every user program's prelude and runtime | F-9 | V |
| 10 | Input is dropped without a diagnostic: interpolation parse failures, stdlib `use`/`type`/`newtype` items in bootstrap merge, unknown `ruxen compile` flags and `--emit` values (exit 0, no output), and `it_panics`'s `expected_substr` | F-3 | V (the main ones) |
| 11 | Likely, needs a reproducing fixture first: structs, enums and mixins nested in a module resolve to `UNRESOLVED_DEF` | F-11 | V by reading the code |
| 12 | Likely: `for (i, x) in …` with an outer `i` in scope captures the outer DefId | F-12 | V by reading the code |
| 13 | Likely: a user method named `push_str` called on a local variable gets that variable overwritten with the call's return value | F-23 | V |

## Families

Families are listed in the recommended fix order.
- **Phase A** repairs the guard tests.
- **Phase B** fixes the independent blockers.
- **Phase C** deletes dead code.
- **Phase D** adds shared infrastructure.
- **Phase E** makes the HIR keep resolution results.
- **Phase F** replaces name tables with declarations.
- **Phase G** consolidates the duplicated drivers and helpers.

### Phase A: guards

**F-1 Guard tests that check nothing.** Kind: other-debt. Severity: medium. Effort: S.
- Instances:
  - `compiler/ruxen_core/tests/error_code_registry.rs:19-24` scans a `crates/` folder that does not exist (also stated in ruxen/CLAUDE.md).
  - `core/typeck/method_resolvers/mod.rs:776-805`: `golden_covers_every_method` scans `mod.rs`, which no longer holds any arms. `:595-620` is a duplicate test.
  - `core/resolve/stdlib_embedded.rs:168` cites a pin test `multi_file_pkg_lib_present_when_siblings_listed` that does not exist (V).
  - `compiler/ruxen_core/tests/formatter_corpus_roundtrip.rs:20` checks only that output parses and is idempotent, never AST equality (V).
  - Empty `#[ignore]` placeholder tests: `core/diagnostics/mod.rs:115-143` and `core/resolve/symbols.rs:1018`.
  - No check that every exported runtime C symbol has a consumer.
- Root cause: guards were not updated through the migrations.
- Fix:
  - Point the registry scan at `compiler/` and `src/`.
  - Delete the vacuous and placeholder tests.
  - Add the sibling pin test.
  - Compare span-stripped ASTs in the round-trip test.
  - Add a "symbol has a lib declaration or a compiler emitter" check to `runtime_abi_derivation.rs`.
- Risk: the fixed scans will flag existing debt. That is intended.
- Dependencies: F-5, F-8, F-15 and F-43 rely on these guards.

### Phase B: independent blockers

**F-2 Silent stubs in runtime, codegen and lowering.** Kind: release-blocker. Severity: high. Effort: S (REPL part M).
- Instances:
  - `std/core/runtime/alloc.c:94` `ruxen_result_try_op` returns 0 on Err (V); `:82` `unwrap_or_else`; `:201` section header "No-op Stubs".
  - `core/codegen/lang_intrinsics.rs:63` maps `"super"` to `ruxen_noop` (V); `:291-293` map `try_op`/`unwrap_or_else` to the stubs.
  - `ruxen_noop_return_null` arms in `core/codegen/cranelift/emit.rs` and `core/codegen/llvm/emit/instructions.rs`. No producer exists outside the two backends and `jit.rs` (V).
  - `std/executor/src/lib.rx:34` `def block_on(future: Int) -> Int { future }` (V).
  - `bin/ruxen_repl/runtime_stubs.c:17,29` weak no-op stubs (V). The real helpers are synthesised at `core/mir/lower/mod.rs:1099` but exist only as JIT symbols (V).
  - `core/mir/lower/expr/index.rs:122` catch-all `Ok(None)` (V), while typeck accepts these shapes at `core/typeck/infer/collect.rs:449,457` (V).
- Root cause: v1 fallbacks left in place. This breaks contract 3 (no silent stubs).
- Fix:
  - Delete the C stubs and their `lang_intrinsics` arms, residual-table rows and `jit.rs` registrations.
  - Make `super` outside a subclass `init` a registered resolve or typeck error.
  - Make the `block_on` body `panic!`.
  - Make the index catch-all an error (better: a typeck coded error), peel with `peel_refs()`, and add String `def []` in `.rx`.
  - For the REPL, abort in the stubs now. Later, dispatch through the `class_info`/vtable pointer.
  - Keep only the sanctioned `ruxen_noop_passthrough`.
- Risk: `core/codegen/runtime/tests_resolve.rs:85` pins `super` to `ruxen_noop`. Needs negative tests with error codes. Guards are the `?`-operator e2e fixtures and the REPL suite.

**F-3 Input dropped without a diagnostic.** Kind: release-blocker. Severity: medium. Effort: S.
- Instances:
  - `core/resolve/helpers.rs:301-335`: a failed interpolation parse returns `HirExprKind::Error` with no diagnostic (V).
  - `core/resolve/bootstrap_merge.rs:517`: the filter silently skips Use, TypeAlias and Newtype items (V). This drops `std/bench/src/lib.rx:48` and `std/test/src/runner.rx:1` (V).
  - `core/resolve/funcs.rs:369` ignores associated-type bounds (TODO); `funcs.rs:759` use-group miss; `core/resolve/exprs.rs:1353` UnsafeBlock skips bare-assignment binding; `helpers.rs:372` const-eval `NotImplemented` ignored.
  - `bin/ruxenc/src/compile.rs:104` unknown flags dropped (V); `:809` unknown `--emit` value returns `Ok(())` (V).
  - `std/test/src/tester.rx:76` never reads `expected_substr` (V); `:30-36` `context` drops `hooks_after`.
  - `bin/ruxen_repl/src/eval.rs:779-790` defers a definition on message text, which also swallows real "type mismatch" errors.
- Fix:
  - Forward the interpolation parser's diagnostics (F-5 removes this path entirely).
  - Emit a coded error (for example E0725) for unsupported stdlib items, or support them.
  - Coded errors for associated-type bounds and use-group misses.
  - UnsafeBlock calls `resolve_block_as_expr`.
  - clap `ValueEnum` and typed fields for `Command::Compile`.
  - Shared `run_case` in Tester that implements the substring check.
  - REPL matches on `d.code`.
- Risk: new diagnostics may appear in rondo and quiver code. Run their test suites.

**F-4 Generic collection methods on Int-only C kernels.** Kind: release-blocker. Severity: high. Effort: M.
- Instances:
  - `std/array/src/lib.rx:80,83,92,103`: `include?`, `sort`, `dedup` and `sum` aliased to Int kernels (V).
  - `std/array/runtime/vec.c:445-455` `contains_int` bitwise compare (V); `:248` `sum` adds raw words (V); `:465` "typeck-rejected upstream", but no such rule exists (V: no `"sort"` in typeck); `:602` `vec_eq`; `:813` `dedup`.
  - `std/hash/runtime/hash.c:666` `hash_eq`.
  - `core/mir/lower/expr/binops.rs:580` routes every `Array == Array` to `ruxen_vec_eq`.
- Root cause: these stayed as FFI aliases when Array became generic.
- Fix: write `.rx` bodies over `T`'s `==`, `<=>` and `+`, as `min`, `max` and `uniq` already are (`array/src/lib.rx:176-267`), or pass `RUXEN_ELEM_*` kinds through `ruxen_elem_eq`/`ruxen_elem_cmp` next to `ruxen_elem_clone`. Then delete the C kernels.
- Risk: add String and Float fixtures first. Guards: e2e 142, 146, 601, 1734, `stdlib_array_negatives.rs`, the method-resolver golden test.

**F-5 `ruxen fmt` corrupts or drops code.** Kind: release-blocker. Severity: high. Effort: M.
- Instances:
  - `core/formatter/format_expr.rs:711` falls back to `{:?}` for unknown tokens; Spaceship is missing from `:600-625` (V). Fixture 965:43 hits it (V).
  - `core/formatter/format_items.rs:500` emits `@[repr]` (V), which `core/parser/items.rs:56` rejects as retired (V); `:520,580` emit `derive` (V); `:1041` emits `@[link]` (V).
  - Class and enum `layout` never emitted; type-level `where_clause` and mixin `async` dropped.
  - `core/formatter/format_type.rs:135` renders const-expr args with the debug printer `format_expr_short` (V).
  - `core/formatter/format_items.rs:216-222` emits inline `protected` while section markers also emit it (V).
  - Interpolation is stored as raw tokens: `core/parser/visit.rs:56`, `core/resolve/exprs.rs:57`.
  - Visibility uses `Private` to mean "unspecified": `core/parser/items.rs:63-77,187,269`.
- Root cause: interpolations are never parsed, and the formatter reads AST fields without exhaustive destructuring.
- Fix:
  - Parse `#{…}` in the parser into `StringPart::Expr { expr }` with `Parser::parse_expression`. Format it with `format_expr` and walk it with `walk_expr`. Delete `token_to_source`, `interp_token_is_spaced` and `resolve_interpolation_tokens`.
  - Destructure AST structs with no `..` in each `format_*`.
  - Add one shared `layout` emitter.
  - Make the visibility prefix `Option<Visibility>`.
- Dependencies: F-1 (AST-equality test).
- Guards: `formatter_corpus_roundtrip.rs`, `q23_fmt_nondestructive.rs`, `syntax_parity.rs`, e2e 965, 1840, 1170, 1083.

**F-6 Async runtime leaks and lost wakeup.** Kind: release-blocker. Severity: medium. Effort: M.
- Instances:
  - `std/async_io/src/async_read_line_future.rx:31-38` registers the fd but never calls `ruxen_reactor_set_waker` (V). `std/future/runtime/reactor.c:470` sets `task = NULL` (V) and `:1074` wakes only when `task` is set (V).
  - `std/future/runtime/scheduler.c:267` "accept the leak for now" (V); `:336,385,561,578,702`; `std/future/runtime/schedule.c:44,63`.
- Fix:
  - Interim: add the missing `set_waker` line.
  - Real fix: one shared step-future poll (F-39), plus `ruxen_poll_is_ready` that frees only the 16-byte Poll box. Free completed task futures after the payload moves out.
- Risk: the payload must not be freed. Check with the RSS-capped wrapper on a poll loop. Guards: e2e 740 and the 7xx async fixtures.

**F-7 LLVM and Cranelift correctness drift.** Kind: release-blocker. Severity: medium. Effort: S.
- Instances:
  - `core/codegen/llvm/emit/mod.rs:360-373` has no `no_std` check (V); compare `core/codegen/cranelift/mod.rs:101,565`.
  - `core/codegen/cranelift/emit.rs:783` Float `%` uses `srem` and says it "will fail verifier" (V); LLVM uses `frem`.
  - `RefMut` address-taking differs between `emit.rs` and `core/codegen/llvm/emit/instructions.rs:776`.
- Fix:
  - Emit `ruxen_env_init` from MIR into `main` (skipped for `no_std`) and delete both backend branches.
  - Cranelift Float `%` calls `fmod`.
  - The full deduplication is F-29.
- Risk: add a `--backend=llvm` no_std fixture.

**F-8 Regex drop helpers never wired.** Kind: release-blocker. Severity: medium. Effort: S.
- Instances: `std/regex/src/lib.rx` has no `def drop` (V); MIR has no emitter (V); `std/regex/runtime/regex.c:107,133,146`; rows in `core/codegen/cranelift/runtime_sigs.rs:176,197` and `core/codegen/llvm/runtime_decl.rs:219`.
- Fix: declare `def drop as "ruxen_regex_drop"` (and the Match and RegexError equivalents) in the lib blocks, then delete the residual rows.
- Risk: a double free if MIR also deallocates the object. Guards: `drop_frees_storage`, `drop_fixtures.rs`. Add a leak fixture.

**F-9 Test fixtures shipped in production.** Kind: release-blocker. Severity: medium. Effort: M.
- Instances:
  - `core/resolve/bootstrap.rs:91,173` (V) and `core/resolve/stdlib_embedded.rs:27,135`.
  - `std/bootstrap_smoke/src/lib.rx:11-28`, `std/_pin_zero_rust_stdlib/src/lib.rx:26-48`.
  - `std/core/runtime/test_extern.c:20-33`, `std/_pin_zero_rust_stdlib/runtime/foobar.c:24`.
  - `std/future/runtime/stream.c:69-91` (contains a `printf`), `std/future/runtime/executor.c:188,204`.
  - Public String helpers (low): `std/string/src/string.rx:217,261,488,506,1463`.
- Fix: a test-only bootstrap list and a test-only runtime directory, using the existing `RUXEN_WASM_BOOTSTRAP` override pattern.
- Risk: `trio_leak_pin.rs`, `bootstrap_prelude_merge.rs`, `ffi_c_symbol_wired.rs` and e2e 750 must opt in explicitly.

**F-10 Concurrency resolver advertises phantom methods.** Kind: release-blocker. Severity: high. Effort: M.
- Instances:
  - `core/typeck/method_resolvers/concurrency.rs:139,148,173,196,220` (V), plus `:17-26,43-126`.
  - No matching declarations in `std/sync/src/*.rx` (V: `thread_panic.rx` is an empty class; `join_handle.rx` has only `join_raw`; `mutex_guard.rx` has only get/set/drop).
  - `core/typeck/method_resolvers/resolver.rs:55`; `core/typeck/infer/ops.rs:49-62` (Arc owner codes, although Arc was deleted).
  - `core/borrow_check/checks.rs:384-431` duplicates the E1100 Send check.
- Fix: delete `concurrency.rs`. Declare the real surface (`join`, `try_lock`, a typed `Thread.spawn[T: Send]`) as `.rx` bodies over the `_raw` FFI, as `mutex.rx` already does for `lock`. Let the generic send-required-closure check handle `spawn`.
- Risk: re-record `method_resolver_golden.snapshot`. Guards: `concurrency_negative.rs` (E1100-E1102).

**F-11 Module-nested types resolve to `UNRESOLVED_DEF`.** Kind: other-debt. Severity: high. Effort: S.
- Instances: `core/resolve/items.rs:482` (V) and `:668,803,28,42`; `core/resolve/funcs.rs:889`. `core/resolve/ffi_registration.rs:577-584` registers nested types only under the qualified key (V). The qualified key is rebuilt by hand at `ffi_registration.rs:544,582,685,849`, `core/resolve/mod.rs:294` and `items.rs:120`.
- Fix: add `qualified_type_key()` and `own_type_def()` on Resolver, use them in every `resolve_*`, and build `self_ty` from the item's generic params.
- Risk: write the nested-struct fixture first (TDD). Rondo and quiver use nested modules.

**F-12 Pattern bindings registered twice; for-tuple shadowing bug.** Kind: duplicate. Severity: medium. Effort: S.
- Instances: `core/resolve/exprs.rs:860-880` (V); `core/resolve/patterns.rs:189` inserts only when the name is absent (V); `core/resolve/control_flow.rs:91,108,178,229`; `patterns.rs:22` has an unused `_expected_ty`.
- Fix: make `resolve_pattern` the only binder. Delete `register_pattern_bindings` and the duplicate `let` definition.
- Risk: write the shadowing fixture first.

### Phase C: dead code

**F-13 Dead Rust code and APIs.** Kind: unneeded. Severity: medium. Effort: S each (M in total).
- Instances:
  - `core/typeck/coerce.rs:8-135` `try_coerce`/`is_subtype_class` (V: only a doc reference in `tests/variance.rs`).
  - `core/borrow_check/lifetimes.rs` (V: no callers) plus the uncalled region, ownership and move APIs. E1004, E1005 and E1008 are never constructed (V).
  - `compiler/ruxen_driver` (V: no dependents) and `docs/architecture/repo-layout.md:75`.
  - `core/mir/lower/monomorphize.rs:1023` `fn_mono_needs_opaque`.
  - `core/resolve/mod.rs:253` `extern_symbols`; `core/resolve/scope.rs:185-214`; `TypeInfo`.
  - `resolve_with_bootstrap`/`type_check_with_bootstrap` (test-only) and the `_legacy` path probes.
  - `core/formatter/mod.rs:48,131` `format_range`; `core/formatter/format_imports.rs`.
  - `core/codegen/mod.rs:331` `find_runtime_c`; `core/codegen/runtime/mod.rs:16` `RUNTIME_C_SOURCE`.
  - `ruxenc` cache graph and rayon path: production always passes `parallel: false` (V, `compile.rs:215`). `bin/ruxenc/src/cache/graph.rs`.
  - rlib exports metadata (`bin/ruxen_cli/src/build.rs:954-1080`).
  - `bin/ruxen_cli/src/build.rs:300` `clean()` has no caller (V), while `ruxen clean` clears only the incremental cache and error hints promise more.
  - `warning_with_code`; the blanket `#![allow(unused_imports)]` on 9 resolve files.
- Fix: delete, or `cfg(test)`. Make one `clean` that removes the project target root.
- Risk: none functional. `cargo build`/`clippy` catch stragglers.

**F-14 Retired-syntax leftovers.** Kind: unneeded. Severity: medium. Effort: M.
- Instances:
  - `derive_traits` is always `Vec::new()`: `core/parser/classes.rs:34,334,932` (V). The field is carried through AST, HIR, `*Info` structs and MIR (91 references).
  - `TopLevelItem::Extern`/`ExternBlock` are never produced (12 match arms); `TokenKind::Lifetime`/`Mut`; `Attribute`/`AttrArg`; `link_attrs` is always empty.
  - Retired macros: `core/parser/macros.rs:37`, `core/mir/lower/expr/misc.rs:120` (an uncoded MIR error).
  - The Hash→Hashable alias can never fire: `core/resolve/bootstrap_merge.rs:488-495` only inserts when `Hash` is absent, and `std/core/src/lib.rx:580` declares `mixin Hash` (V). The dual-name checks repeat at 6 sites.
  - Legacy `Vec`/`vec` spellings.
- Fix: delete the fields, variants and readers. Retire the macro names into the existing E1126 path (or a coded error). Pick Hashable and delete `mixin Hash` along with the dual checks.
- Dependencies: F-5 (the formatter currently emits `derive` and `@[link]`).

**F-15 Dead runtime C symbols and dead stdlib surface.** Kind: unneeded. Severity: medium. Effort: S–M.
- Runtime instances:
  - `std/net/runtime/tcp.c:66-212`: the flat TCP API has no callers (V). The class API copies its bodies (`:376,541`).
  - `std/array/runtime/vec.c:829` `vec_join` (V); `std/string/runtime/string.c:851` `str_split` is referenced only by residual tables and the JIT (V).
  - `vec.c:198,221,687`; `alloc.c:62,175`; `executor.c:204`; `scheduler.c:225,245,325`; `test.c:70-82,157`; `env.c:51,55`.
- Stdlib instances: `thread_spawn_raw`, `parse_json`/`stringify_json`, `Sender`/`Receiver` plus `std/sync/runtime/channel.c` (it has no constructor and duplicates `future/runtime/queue.c`), `mixin Iterable`, `TestCase`, unused async_net accessors, and the Phase C free-function shims in fs, env, path, rand, time, signal, process and json.
- Fix: delete bodies, residual rows and `reg!` entries, and port fixtures to the class form.
- Dependencies: F-1's symbol guard. Update `runtime_abi_parity.rs` and `runtime_safety.rs` in the same change.

### Phase D: shared infrastructure

**F-16 No HIR visitor.** Kind: duplicate. Severity: medium. Effort: L.
- Instances:
  - mir: `core/mir/lower/trait_default.rs:149,300`; `core/mir/lower/captures.rs:21,263,365`; `core/mir/lower/closure_inline/mod.rs:19` (no Index, UnsafeBlock or Tuple arms, so E1125 can miss); `core/mir/lower/monomorphize.rs:1427,1504,1757,1919`.
  - `core/borrow_check/walk.rs`; `core/typeck/infer/helpers.rs:419-665` (`collect_break_types`/`collect_next_types` are twins); `core/typeck/mod.rs:366-506` `resolve_expr_types` ends in `_ => {}`.
  - `core/no_std.rs:46-88`: `validate_item` ends in `_ => {}` (V), so struct and enum method bodies escape E1400.
  - Ten IDE walkers (`bin/ruxen_ide/src/{node_finder,use_index,highlight,rename,references,signature_help,completion,inlay_hints,folding,semantic_tokens}.rs`).
  - REPL AST walkers at `bin/ruxen_repl/src/eval.rs:198,287,435`, where `MultiAssign` is never handled (V).
  - `core/parser/visit.rs` has no item-level walker.
- Root cause: `core/hir/` has only `context`, `mod`, `nodes`, `tests` and `types` (V); there is no `visit.rs`.
- Fix: add `core/hir/visit.rs` (`walk_expr`, `walk_expr_mut`, `walk_item`), exhaustive with no `_` arm, mirroring `parser/visit.rs`. Port `no_std` first. Add item walking to `parser/visit.rs` and use the AST `Visit` in the REPL.
- Risk: coverage widening is the point. Guards: `no_std_e1400.rs` (add a struct-method case), the capture tests, the q17 fixtures.
- Status (batch 1): done. `core/hir/visit.rs` exists; the compiler passes and all ten IDE walkers use it, and the REPL uses the AST `Visit`. `highlight` and `rename` share `node_finder::method_call_at`, and `use_index` sorts each def's uses into source order after the walk.

**F-17 Canonical `Ty` helpers not adopted.** Kind: duplicate. Severity: medium. Effort: M.
- Instances:
  - `core/hir/types.rs:1257` `peel_refs` documents itself as "the single canonical peeler", yet there are 38 hand-written peels in mir and 26 in typeck. Examples: `monomorphize.rs:123,1344`, `method_call.rs:93`, `index.rs:94`, `helpers.rs:268`.
  - Three substitution implementations: `core/typeck/infer/collect.rs:1201`, `core/typeck/infer/helpers.rs:204` (misses mixin bounds), `core/mir/lower/trait_default.rs:66,111` (misses Fn, Alias and generic args).
  - Fn-signature extraction written 9 times (`collect.rs:688`, `core/typeck/mixins.rs:231,250`, `core/typeck/infer/expr.rs:27,58`, `core/typeck/unify.rs:340`, …).
  - The method-home mapping written 5 times: `mixins.rs:1318`, `collect.rs:1096`, `expr.rs:654` (omits Option and Result), `core/borrow_check/checks.rs:461` (parses Display text), `core/mir/lower/expr/field_access.rs:199`.
  - Builtin heads have two spellings, `Ty::Array` and `Ty::Class{"Array"}`: `core/mir/lower/util.rs:48,72`, `index.rs:12`, `core/mir/lower/match_arms.rs:687`, `monomorphize.rs:86`, `collect.rs:428`.
  - Scalar-primitive lists: `core/resolve/stdlib/primitives.rs:16`, `core/resolve/types.rs:44`, `ffi_registration.rs:1462`, `core/hir/types.rs:425,489,572`.
- Fix:
  - Use `peel_refs()` everywhere.
  - Make `map_inner` recurse into mixin bounds, then add `Ty::subst_params`, `Ty::callable_signature`, `Ty::method_home`, `Ty::is_scalar_primitive` and `Ty::any`.
  - Give `self` in builtin shell classes the structural head type once, at assignment, instead of reconciling names at every consumer.
- Risk: substitution fixes may change monomorphization output, for the better. Guards: the `map_inner` tests, q17, the trait-default fixtures.

**F-18 `SymbolTable` has no name or parent index.** Kind: duplicate. Severity: medium. Effort: M.
- Instances:
  - About 60 crate-wide `symbols.iter()` scans plus 32 in mir.
  - `core/mir/lower/function.rs:72` maps every SelfValue in the whole program to the current method's self local (V).
  - `core/borrow_check/checks.rs:310` takes the first definition by name.
  - `core/hir/types.rs:993` `nominal_definition` and `core/resolve/const_helpers.rs:168` are mutable and immutable twins.
  - `collect.rs:568` `lookup_class_method_return` cannot see overloaded methods.
- Fix: index by name, kind and parent in `define`. Expose `nominal`, `type_def` and `methods_of` (overload-aware). Bind only the method's own SelfValue.
- Risk: where duplicate names exist, "first defined wins" changes. Guards: send/sync, derive and overload tests.

### Phase E: keep resolution results in the HIR

**F-19 HIR loses call shape (safe navigation, calls without parentheses, operators).** Kind: non-generic. Severity: high. Effort: L.
- Instances:
  - `core/resolve/exprs.rs:1088` lowers SafeNav to a plain FieldAccess (V).
  - `core/mir/lower/expr/field_access.rs:18` guesses safe navigation from an exclusion list of Option method names (V); `core/mir/lower/util.rs:10` uses `starts_with("Option")` (V).
  - `field_access.rs:166` copies the `.new` constructor path (V) and `:310-480` copies the rest of the method-call pipeline (comments say "parity with method_call.rs").
  - `core/typeck/infer/expr.rs:163-335` has four copies of member-as-method resolution.
  - Operators become synthetic MethodCalls at five MIR sites: `binops.rs:361,418`, `index.rs:108`, `unaryops.rs:37`, `assign.rs:365`. Typeck resolves them at `expr.rs:1591` without rewriting the node. Builtin `==`/`<=>` fallbacks are hardcoded at `binops.rs:529-627`.
- Fix:
  - Add a `safe_nav` flag.
  - Typeck rewrites a FieldAccess that names no field into a `MethodCall` with empty args.
  - Typeck rewrites resolved operators into `MethodCall`s.
  - Declare `def ==`, `def <=>` and `def +` in the string, array, map and set `.rx` files.
  - Delete the MIR copies.
- Dependencies: F-16.
- Risk: formatter and IDE hover read the same HIR. `&&`, `||` and `!=` stay special. Guards: e2e safe-navigation, `.new`/`.default` and operator fixtures.

**F-20 Typeck does not record the method it resolved.** Kind: non-generic. Severity: high. Effort: L.
- Instances:
  - `core/typeck/infer/expr.rs:640-674` ("We do NOT set `*method` for a builtin head").
  - `core/borrow_check/checks.rs:554-592` guesses mutation from method names (V); `:446-465` builds the receiver's home from Display text; `:304-325`.
  - IDE has 6 method-lookup strategies, and `bin/ruxen_ide/src/use_index.rs:743` `resolve_struct_method` is a stub returning `None`.
  - MIR callees are strings that get re-parsed: `core/mir/lower/mod.rs:2075,2301`, `core/mir/lower/runtime_abi.rs:367,395`, `monomorphize.rs:231`, `core/mir/lower/type_helpers.rs:596`, `core/codegen/runtime/mod.rs:23`.
- Fix:
  - Record the DefId and FnSignature on every MethodCall, in a side table if MIR name mangling must not change.
  - Give `MirInst::Call` a structured callee (`symbol`, `def`, `owner`, `mono`).
  - Quick win (S): make `lookup_ffi_alias` call `strip_generic_suffix` instead of its inline copy.
- Dependencies: F-18.
- Guards: `borrow_check/tests.rs`, q29, e2e 649 and 965, `ffi_alias_single_entry.rs`.

### Phase F: replace name tables with declarations

**F-21 Mixin-bound generics not monomorphised; sample-program names baked into the compiler.** Kind: non-generic. Severity: high (blocker 5). Effort: L, with an interim M step.
- Instances:
  - `core/codegen/lang_intrinsics.rs:290-324`: method table for `?T` receivers plus a list of fixture method names (V).
  - `core/codegen/cranelift/translation_env.rs:83-119`: "shortest suffix wins" symbol guess (V). The same in `core/codegen/llvm/emit/calls.rs:272,399`.
  - `T_method` heuristic (prefix of 1-2 uppercase letters) in three places: `lang_intrinsics.rs:328` (V), `translation_env.rs:121`, `calls.rs:281`.
  - `core/mir/lower/util.rs:29` `is_collection_method` (V) and `core/mir/lower/closure_inline/mod.rs:174` reads field 0 of a user class as a Vec (V); `:108,133,219,260`; `core/mir/lower/expr/method_call.rs:548`.
  - `display`/`display_all` defaults: `core/resolve/funcs.rs:550` (V), `core/typeck/infer/mod.rs:272` (V).
  - Closure-inline scaffolding repeated in 9 files (low).
- Root cause: `Repository[T: Showable].display_all` needs the suffix fallback (comment at `translation_env.rs:91`) because mixin-bound generics are not monomorphised.
- Fix:
  - Interim: delete the `is_collection_method` user-class path so user methods run their own bodies. Remove the `display` defaults and annotate the sample instead.
  - Real fix: monomorphise mixin-bound generics, or route them through the existing mixin vtables and `<Mixin>_dynamic_<m>` helpers. Then make any `?` callee a hard error, delete the three heuristics and the fixture list, and delete the closure-inline modules that become unreachable.
- Dependencies: F-20.
- Guards: e2e 1860, 1868; `codegen_unknown_method_rejected.rs`; `compiler/ruxen_core/tests/fixtures/sample_program.rx`.

**F-22 Builtin static and method surface kept in Rust name tables.** Kind: non-generic. Severity: high. Effort: L.
- Instances:
  - MIR: `core/mir/lower/expr/method_call.rs:182` five `is_*_static_ctor` locals (V), plus `:278,338,362,437,918`; `core/mir/lower/runtime_abi.rs:430,448` `STATIC_CTORS` (V); `util.rs:84`; `field_access.rs:179`. Every entry is already declared with `def self.X as "sym"`, for example `std/time/src/lib.rx:34` and `std/net/src/lib.rx:95` (V).
  - Resolve: `core/resolve/stdlib/type_constructors.rs:36` `SIMPLE_CLASS_CTORS` (its own doc calls it debt, V); `option.rs`, `result.rs`, `modules.rs:32`; `core/resolve/types.rs:455,528`; `exprs.rs:93` (Regex literal).
  - Typeck residual resolvers: `core/typeck/method_resolvers/collections.rs:12-117`, `io.rs:42-203` (plus E0714), `helpers.rs:258`; name hooks at `expr.rs:371,465,589,620` (V); `collect.rs:164-290`.
  - Codegen: `lang_intrinsics.rs:75-118,199-282` scalar `to_s` and Option `expect!`/`unwrap!`.
  - Stdlib: synonyms declared as second FFI declarations instead of `alias` (`std/string/src/string.rx:19-62`, `std/scalar/src/lib.rx:48,630-790`, time, thread, file); E0722 was relaxed to permit this.
- Fix:
  - Static-ness comes from `is_user_static_method` and the callee from `lookup_ffi_alias`. Delete the cascade and both tables.
  - The class arm of `register_top_level_type_with_ffi_in` binds the value scope too.
  - Declare Option and Result in `.rx` with a tag-stability test.
  - Allow `!` in lib-declaration names and scalar `extension` lib shells.
  - Make bare `BufReader` a typeck error that points to `BufReader.File`/`.Tcp`.
  - Turn synonyms into `alias` and re-tighten E0722.
- Risk: `runtime_abi_parity.rs` and `ffi_alias_single_entry.rs` pin the tables. File, net, bufio and time fixtures; rondo and canvas.

**F-23 String "returns a new buffer" contract handled per method name.** Kind: non-generic. Severity: medium. Effort: M.
- Instances: `core/mir/lower/expr/method_call.rs:1092` (V: no `resolved_class == "String"` guard on `push_str`), `:1150,1217,1266,1322` (V); `core/mir/lower/expr/assign.rs:416`; `core/typeck/method_resolvers/strings.rs:56-82` reports return types that contradict the `.rx` declarations.
- Fix: a lib-declaration marker for self-reallocating methods, or an in-place `RuxenString*` slot ABI, plus one `emit_rebinding_self_call` helper. Then delete the name blocks and `strings.rs`.
- Guards: e2e 311, 313, 315, 630, 631, 48; the drop leak backstop.

**F-24 Display, printing and interpolation keyed by type and name.** Kind: non-generic. Severity: medium. Effort: M.
- Instances:
  - `core/mir/lower/interpolation.rs:277,286,296` fall back to `Int_fmt`, which prints the raw word (V).
  - `core/mir/lower/derive.rs:67` synthesises primitive `fmt` in Rust.
  - `method_call.rs:102` intercepts `to_s` only in the parenthesised form.
  - `core/hir/mod.rs:34` `IO_TEXT_BUILTINS`, consumed at `core/typeck/infer/expr.rs:775` and `core/mir/lower/expr/fn_call.rs:14` (V).
  - `std/core/src/lib.rx:708-749`: `p`/`pp` written as one overload per type.
  - Overlapping protocols: Displayable, Display and Debug.
- Fix: `.rx` `extension Int include Display`; the interpolation fallback becomes an error; `to_s` becomes a Kernel default; insert `to_s` coercion keyed on the parameter type, not the callee name; one generic `p[T]`.
- Dependencies: F-21 (generic free functions over mixin bounds).
- Guards: byte-identical fmt and interpolation e2e output.

**F-25 Ownership ABI and element-aware drop hand-listed.** Kind: non-generic. Severity: high. Effort: L.
- Instances:
  - `core/mir/lower/runtime_abi.rs:95,506,585,746-825`: mangled-name and C-symbol pairs, `FREE_FAMILY` repeated inside `CONSUME_HELPERS`.
  - `core/mir/lower/key_desc.rs:57,90,129`.
  - `core/mir/lower/drops.rs:1229` per-type-pair callee table, `:914` a second copy of the eligible-type list, `:1072` `unimplemented!` backstop.
  - `std/hash/runtime/hash.c:222-271,524`, `std/array/runtime/vec.c:631,641`. These leak `Hash[String, Array[_]]`, `Array[Array[String]]` and similar.
- Fix:
  - Ownership markers in the lib grammar, recorded on the Definition.
  - Interim: key each table once by C symbol.
  - A recursive `ruxen_elem_drop(kind)` next to `ruxen_elem_clone`; one drop callee per container; eligibility test is `heap_free_callee().is_some()`.
- Risk: double free and leak. Keep the `ORIG_FREE` naming that `drop_fixtures.rs` rewrites. Guards: `runtime_abi_parity.rs`, the RSS-capped compile.

**F-26 Residual runtime ABI tables triplicated and stale.** Kind: duplicate. Severity: high. Effort: M.
- Instances:
  - `core/codegen/cranelift/runtime_sigs.rs:44-205` and `core/codegen/llvm/runtime_decl.rs:33-225` disagree (V: `ruxen_vec_push` only in LLVM, `puts` and `ruxen_env_args` only in Cranelift). Both hold `.rx`-declared symbols such as `std/array/src/lib.rx:62,84` (V), which contract 1 forbids.
  - `bin/ruxen_repl/src/jit.rs:32-152,665-781` has an `extern` block and 76 `reg!` entries (V), although its dlsym fallback already resolves every `ruxen_*` symbol.
  - Also `core/codegen/cranelift/emit.rs:228` inlines `ruxen_string_eq`.
- Fix: one backend-neutral `AbiTy` table holding only symbols the compiler really emits; drop declared and dead symbols; delete the JIT table and keep only the capture and slot shims.
- Risk: wasm32 LLVM relies on the `vec_*` declarations for slot width; check derived `ty_to_llvm` first. Guard: `runtime_abi_derivation.rs`.

**F-27 Derive machinery fragmented and diverging.** Kind: duplicate. Severity: medium. Effort: M (L with the trait-family decision).
- Instances:
  - `core/typeck/mixins.rs:1133-1229,584` and `core/typeck/method_resolvers/resolver.rs:115` hard-code signatures that differ from the `std/core/src/lib.rx:564-594` mixins.
  - MIR predicate copies: `core/mir/lower/type_helpers.rs:232-637`; `derive.rs:417,849,987,1464`; `binops.rs:441`.
  - One if-chain per item kind: `core/mir/lower/mod.rs:795,851,893`. Classes silently miss Ord, PartialOrd and Default (V comment at `:786`).
  - "Hashable" vs "Hash" spelled at `derive.rs:532`, `core/typeck/mixins.rs:1170` and `core/implicit_includes/mod.rs:479`.
  - Ord, PartialOrd and PartialEq overlap Comparable (needs a design decision).
- Fix: full `Self`-typed `.rx` mixin signatures with derived impls registered through `Self` substitution; one `derived_nominal()` helper and a `DERIVES` registry over `DeriveField`; synthesise class `_cmp` and `_default` (or reject class `derive Ord` with a coded error).
- Guards: derive e2e (2xx, 49, 208, 216, 209), 703, B74.

**F-28 Compiler protocols encoded in names.** Kind: other-debt. Severity: medium. Effort: S–M.
- Instances:
  - Temporaries by `_t` prefix: `core/mir/nodes.rs:264` and `core/mir/lower/drops.rs:935` (V). A user local named `_total` follows the temporary rules.
  - Drop detected by the method name `"drop"`: `core/mir/lower/collect.rs:220`, `mod.rs:1366`. Membership by bare-or-suffix string match: `drops.rs:949,996`, `core/mir/lower/emit.rs:37`, `core/mir/lower/expr/closure.rs:586`, `mod.rs:1224`.
  - `__state`/`__sub_`: `mod.rs:1362,1389`.
  - Future and Stream by name (low): `mod.rs:1217,1350,1477`.
  - Linkage chosen by name (`main`, `_dynamic_`), with an LLVM→Cranelift module edge: `core/codegen/cranelift/mod.rs:270,704`, `core/codegen/llvm/emit/mod.rs:80`.
  - `Into` special case written 4 times, with an Include gap: `collect.rs:15-90`.
- Fix: `MirLocal.kind`; `MirFunction { linkage, is_entry }`; key drop classes by DefId; resolve lang items once to DefIds; one `impl_blocks_with_target` iterator.

### Phase G: consolidation

**F-29 Backend lowering policy duplicated (Cranelift, LLVM, REPL JIT).** Kind: duplicate. Severity: high. Effort: L.
- Instances:
  - Coercion, signedness and argument rules: `core/codegen/cranelift/emit.rs:113-766` vs `core/codegen/llvm/emit/instructions.rs:7-386`.
  - Type predicates and size tables: `core/codegen/cranelift/helpers.rs:19-205`, `core/codegen/llvm/emit/mod.rs:737-812`, `core/codegen/llvm/types.rs:13`, `core/codegen/layout.rs:368`.
  - Parameter word slots and FFI lookups: `core/codegen/cranelift/mod.rs:202-292`, `core/codegen/llvm/emit/calls.rs:324-395`, `bin/ruxen_repl/src/jit.rs:390,979`.
  - The JIT forks the batch driver: `jit.rs:348-656`.
  - Backend dispatch written 4 times: `core/codegen/mod.rs:608,776,910` (V) and `bin/ruxenc/src/compile.rs:501`. Link flags and C-compile helpers: `core/codegen/object.rs:76-266,833`.
- Fix: a backend-neutral `codegen::abi` (`AbiClass`, `Coercion` decisions, a `CalleeIndex`); make `CodeGen` generic over `M: Module`; add `Backend::emit_object`, `aggregate_link_flags` and `compile_c_sources`.
- Dependencies: F-7, F-26.
- Risk: the LLVM path is barely exercised. Add an LLVM CI run of the same fixtures.

**F-30 No shared front-end driver across binaries.** Kind: duplicate. Severity: high. Effort: L.
- Instances:
  - About 13 pipeline copies: `bin/ruxenc/src/compile.rs:415,439,534,651,712`, `bin/ruxenc/src/bench.rs:75`, `bin/ruxen_cli/src/build.rs:359,448,750`, `bin/ruxen_repl/src/eval.rs:753,882,961,1309`, `bin/ruxen_repl/src/display.rs:162`, `bin/ruxen_ide/src/analysis.rs:50`, `core/typeck/mod.rs:78,114,179`.
  - `ruxen check` (`check_single_file` at `build.rs:448`) never runs the borrow checker (V), so `ruxen check` passes code that `ruxen build` rejects.
  - `ruxenc` re-implements core's private bootstrap type-check without implicit-main synthesis.
  - Backend selection is written twice with different behaviour for `--release`.
- Fix: a real `ruxen_core::driver` (or revive the `compiler/ruxen_driver` crate) with `Frontend::check`, `lower`, `select_backend`, `has_errors` and `render_diagnostics`. Make `type_check_with_loaded_bootstrap` public.
- Risk: diagnostic order and wording change; adding borrow checking to `ruxen check` needs a CHANGELOG entry. Guards: the e2e cli, repl and lsp phases; `p05_e2e_check.rs`.

**F-31 Resolver copy-paste (FFI registration, impl lowering, helper pairs).** Kind: duplicate. Severity: medium. Effort: M.
- Instances:
  - The Lib and Extern arms are identical: `core/resolve/ffi_registration.rs:1264,1343` (V). Also `:102,501,845,1121`; the mixin path lacks the generic-param scope that classes push.
  - Three copies of impl-item lowering, with Send/Sync flags computed twice: `core/resolve/items.rs:230,586,938,305,1080`; `core/hir/types.rs:933,963`.
  - Helper pairs (low): `core/resolve/funcs.rs:50,87,318`, `core/resolve/helpers.rs:24,53,139,165`, `core/resolve/aliases.rs:26,56`, `core/resolve/symbols.rs:394,409`.
  - Use-declaration binding written three times: `funcs.rs:653,708,743`; `exprs.rs:201-254`; `types.rs:79,140`.
- Fix: `register_ffi_fn` and `register_lib_block`; `lower_inner_impls` everywhere plus an `AutoTraitFlags` struct; `DefKind::signature()` and `generic_params()`; `MixinRef::from_bound`.
- Guards: `runtime_abi_derivation.rs`, the E0722 tests.

**F-32 Parser and formatter internal duplication.** Kind: duplicate. Severity: medium. Effort: M.
- Instances:
  - Five body-member loops: `core/parser/classes.rs:41-1146`, `core/parser/items.rs:210-288`.
  - `def` header parsed twice (`core/parser/methods.rs:104,182,300`) and signatures rendered five times (`core/formatter/format_items.rs:212,732,1083`, `core/formatter/mod.rs:161`, `core/parser/printer/format.rs:77`). Also `bin/ruxen_ide/src/completion.rs:494` (a hand-built renderer, which ruxen/CLAUDE.md forbids) and `bin/ruxen_repl/src/display.rs:100` `format_type`.
  - Five function-type parsers: `core/parser/types.rs:100-603`, `methods.rs:413-474`.
  - Keyword, token, operator and numeric-suffix tables: `core/lexer/token.rs:378,444`, `core/parser/mod.rs:496,576`, `core/parser/expr/mod.rs:30,367`, `core/formatter/prec.rs:68`, `core/formatter/format_expr.rs:715,744`, `core/typeck/infer/ops.rs:502`, `bin/ruxen_repl/src/completion.rs:15`, `bin/ruxen_ide/src/rename.rs:298`.
  - `core/parser/printer/` is a second renderer of about 1,100 lines.
- Fix: a shared `BodyState` helper; `parse_func_def` calls `parse_method_signature`; one `format_signature` Doc that `render_fn_signature` also uses; single `KEYWORD_TABLE` and `OPS` tables; `--emit ast` as `{:#?}`, then delete the printer.
- Dependencies: F-5.

**F-33 Grammar re-implemented outside the parser.** Kind: duplicate. Severity: medium. Effort: M.
- Instances:
  - `core/module_loader.rs:438` autoload scanner misses `lib`/`case`/`until`/`unsafe`; `:303,481` runs full typeck on every fixpoint iteration.
  - `core/formatter/comments.rs:116-425` is a second lexer that mis-detects comments.
  - REPL incompleteness detected by matching message text: `core/parser/mod.rs:88-225,763`.
  - `bin/ruxen_repl/src/lib.rs:38-372` `split_repl_chunks`; `bin/ruxen_repl/src/validate.rs:15-80`.
  - `bin/ruxenc/src/test_runner.rs:361-456` `split_test_body` hoists by column-0 text.
  - `bin/ruxen_ide/src/code_actions.rs:228` uses `rfind("def")`.
- Fix: read `require`/`autoload` from the AST; lexer trivia (`with_trivia`); `Incomplete` and `hit_eof` as structured results; `Parser::is_item_start`; reuse `synthesize_implicit_main` for test wrappers; the HIR node finder for code actions.

**F-34 Parser desugars surface syntax.** Kind: non-generic. Severity: high. Effort: L.
- Instances:
  - `core/parser/expr/case_expr.rs:32` re-evaluates the subject on every branch (V); `:223,249` hardcode Range and `==` (V).
  - `core/parser/expr/control_flow.rs:97,130,265` (`unless`, `until`).
  - `core/parser/expr/operators.rs:46,133` (`||=` evaluates its left side twice).
  - `core/parser/methods.rs:268` (endless `def`).
- Fix: surface AST nodes, desugared in one resolve-time pass; `when v` becomes `v.===(subject)` with Range's `===` in `.rx`; bind the subject to a temporary once.
- Risk: language decision on `===`. Dependencies: F-16. Guards: case, unless and `||=` e2e fixtures; `syntax_parity.rs`.

**F-35 IDE definition-resolution copy-paste.** Kind: duplicate. Severity: medium. Effort: M.
- Instances: `bin/ruxen_ide/src/highlight.rs:101-484` and `bin/ruxen_ide/src/rename.rs:343-772` are near-identical (diff is comments only); `references.rs:106-456`; `use_index.rs:622-743`; `inlay_hints.rs:477,507`; `signature_help.rs:202,248`; `completion.rs:108-344`; `code_actions.rs:177`; `type_def.rs:135,206`. Synthetic-span helpers are duplicated: 4 local copies (V; the auditor said 6) next to the existing `document_symbols.rs:62`.
- Fix: one `ruxen_ide::resolve` module backed by F-20's recorded DefIds and F-16's visitor.
- Guards: IDE unit tests, `bin/ruxen_lsp/tests/server_integration.rs`.

**F-36 REPL replay design.** Kind: non-generic. Severity: high. Effort: L.
- Instances: `bin/ruxen_repl/src/eval.rs:198,287,435` heuristic walkers (`MultiAssign` is never handled, V) and `:1801,1810` session slots hold Int only; `bin/ruxen_repl/src/session.rs:245`; `bin/ruxen_repl/src/capture.rs:68`; about 40 per-function replay guards in fs, file, stdio, tcp, process, async_net and regex C runtimes.
- Fix: store every session binding in a word-sized slot, then remove the replay flag, the C guards and the walkers. Interim: rewrite the walkers on the AST `Visit`.
- Guards: `state_persistence`, `single_execution`, `split_chunks`; REPL e2e 89, 90, 108, 307, 404, 712.

**F-37 Build and link configuration duplicated; wrong-case manifest lookup.** Kind: duplicate. Severity: medium. Effort: M.
- Instances:
  - `bin/ruxenc/src/compile.rs:882` looks for `ruxen.toml` (lowercase) (V). On Linux it never finds `Ruxen.toml`, so the cache and `ruxen clean` resolve to a directory relative to cwd.
  - Project-root finders: `bin/ruxen_cli/src/build.rs:321`, `bin/ruxenc/src/test_runner.rs:242`. Source gathering and link inputs: `build.rs:489,627,880-935`, `test_runner.rs:307-358,571-610`.
  - `bin/ruxen_repl/build.rs:4-48,108-165` hardcodes PCRE2 and the Security framework; `bin/ruxen_cli/build.rs:65`; `core/codegen/object.rs:18-62,315,954`.
  - `RUXEN_MACOS_FRAMEWORKS` environment variable; four `[system_libs]` readers (`core/codegen/mod.rs:405`).
- Fix: shared `find_project_root`, `LinkInputs::for_project` and `link_requirements`; per-package `[runtime]` defines and excludes in `Ruxen.toml`.
- Status (batch 1): partial, rest deferred. Still open: the `RUXEN_MACOS_FRAMEWORKS` variable (`core/codegen/object.rs:28`) and the hardcoded PCRE2 sources and Security framework in `bin/ruxen_repl/build.rs`.

**F-38 C runtime missing shared helpers.** Kind: duplicate. Severity: medium. Effort: M.
- Instances:
  - Option and Result builders: 3 private copies (`std/json/runtime/json.c:778`, `std/regex/runtime/regex.c:49`, `std/io/runtime/bufio.c:90`) plus 42 inline tag writes.
  - errno → IoError classification copied with async message loss: `std/io/runtime/io_error.c:80`, `std/async_net/runtime/async_net.c:177`, `std/async_fs/runtime/async_fs.c:216,329`, `std/async_io/runtime/async_io.c:162`, plus 4 `invalid_input` helpers.
  - `{fd, closed}` handle struct 6 times and the EINTR close loop 9 times: `std/core/runtime/runtime.h:269`, `tcp.c:323,506-736`, `async_net.c:74-1114`, `async_fs.c:61,114`, `std/io/runtime/file.c:460,479`.
  - UTF-8 codec 5 times, including `std/fmt/runtime/fmt.c:118,144`, which writes `'?'` for non-ASCII characters.
  - About 90 hand-written malloc-or-panic sites; byte-buffer loops.
  - `std/future/runtime/*.c` has no internal header: weak declarations ×3, `RUXEN_POLL_READY_TAG` ×2, three drop-and-free helpers.
  - `std/future/runtime/reactor.c:448,521` "persistent" registration is identical to one-shot.
  - Misplaced translation units: string functions in `vec.c:111-894`, `vec.c:687` `process_run`, `file.c:543`, `io_error.c:308`.
- Fix: add to `runtime.h`/core `ruxen_option_*`, `ruxen_io_error_classify_errno`, `RuxenFdHandle`, `ruxen_utf8_encode`/`decode`, `ruxen_xmalloc`, `RuxenByteBuf`, `RuxenAsyncOp`; create `future_rt.h`.
- Risk: `drop_fixtures.rs` rewrites `malloc(` and `free(` textually, so keep those spellings inside the helpers.

**F-39 Per-type copies in runtime and stdlib.** Kind: duplicate. Severity: medium. Effort: M.
- Instances:
  - BufReader/BufWriter split per inner type: `std/io/runtime/bufio.c:120-500`, `std/bufio/src/lib.rx:29-73`.
  - `std/sync/runtime/atomic.c:17-156` I64 and Usize differ only by a cast.
  - Set re-implements Hash: `std/hash/runtime/hash.c:177-814`.
  - Stdout and Stderr pairs: `std/io/runtime/stdio.c:65-202`; `file.c:286` leaks the Vec data buffer.
  - Seven async step-future `.rx` classes: `std/async_net/src/*_future.rx`, `std/async_fs/src/*_future.rx`, `std/async_io/src/async_read_line_future.rx`. Each repeats init, poll and drop, and they disagree on what poll returns after Ready.
  - Schedule and Scope copies (low): `std/future/src/schedule.rx:167,319`, `std/future/src/scope.rx:321-359`.
- Fix: alias one C symbol from each class's lib block; one `StepFuture` with a single post-Ready contract.
- Dependencies: F-38. The generic `Atomic[T]` `.rx` class is out of scope (see drops).

**F-40 Stdlib `.rx` duplication.** Kind: duplicate. Severity: medium. Effort: M.
- Instances:
  - String: set membership 5 times (`std/string/src/string.rx:280-863`); number parsing 3 times (`:1153-1384`); character walks (`:118-1081`); magic codepoints instead of Char methods.
  - Int rounding prologue 4 times (`std/scalar/src/lib.rx:150-256,498-546,761`).
  - Delegating synonym bodies (`std/core/src/lib.rx:80-456`, `std/set/src/lib.rx:216`, `std/array/src/lib.rx:280`).
  - Array, Hash and Set re-declare Enumerable (`array/src/lib.rx:66-95`, `std/map/src/lib.rx:91`, `set/src/lib.rx:206`, `std/test/src/matcher.rx:106`).
  - ABI workarounds that type values as `Int`: `std/io/src/lib.rx:94-106`, `std/net/src/lib.rx:57-98`, `std/io/src/open_options.rx:14`, `std/http/src/lib.rx:33`.
- Fix: private helpers (`tr_set_position`, `skip_space`, `radix_prefix`, `matches_at?`), `alias` where it already works, Bool parameters now that the ABI keeps Bool as I8, and `Array[UInt8]` for byte buffers.
- Status (batch 1): partial, rest deferred. Still open: the Enumerable re-declarations in Array, Hash and Set listed above, and the instances batch 1 did not touch.

**F-41 Typeck leniency and unsoundness.** Kind: other-debt. Severity: medium. Effort: M.
- Instances: `core/typeck/unify.rs:288-319` lets `&T` unify with `T` (V, a documented TODO); `:249` TypeParam unifies with anything; `core/typeck/infer/collect.rs:110-162` returns a fresh type variable for unknown methods on non-scalar receivers.
- Fix: retry removing the Ref arms, since the iterator machinery the TODO blamed is gone. Make "no method `m`" a hard error for known home classes.
- Dependencies: F-10, F-21, F-22.
- Risk: latent errors will surface in fixtures. Run the full e2e suite and `variance.rs`.

**F-42 Async lowering duplication and wrong type names.** Kind: duplicate. Severity: medium. Effort: M.
- Instances:
  - `core/async_lowering/lower.rs:2263-2272` lists `"I8"`/`"U32"`/`"F64"` (V). The real names are `Int8`, `UInt32` and `Float32`, so `Int32`, `Char` and `String` outputs fail to lower.
  - Three cloned diagnostic walkers: `core/async_lowering/diagnostics.rs:119-420`; E1112 and E1116 key on spellings.
  - Linear and loop class synthesis duplicated, and Poll nodes hand-built about 15 times: `lower.rs:590-712` vs `:1095-1230`.
- Fix: `Option[T]` placeholders (the code comment names this fix) or a list derived from `primitives.rs`; one `AsyncScopeVisitor`; `synthesize_future_class` plus `poll_ready`/`poll_pending` helpers.

**F-43 Bootstrap stdlib lists maintained by hand.** Kind: duplicate. Severity: medium. Effort: M.
- Instances: `core/resolve/bootstrap.rs:78` `BOOTSTRAP_FILES`; `core/resolve/stdlib_embedded.rs:21,171` embedded tables (V); `bootstrap.rs:258,305` zips package names with parsed programs, which misaligns after one parse failure.
- Fix: generate the tables in `build.rs` (as `ruxen explain` already does) and return `(pkg, Program)` pairs.
- Dependencies: F-1 (sibling pin test), F-9.

**F-44 Diagnostics plumbing and small leftovers.** Kind: duplicate. Severity: low. Effort: S.
- Instances:
  - `core/borrow_check/errors.rs:4-60` titles have drifted from `core/diagnostics/codes.rs:306-365` (E1011, E1012, E1014).
  - `Diagnostic` has no help slot.
  - Errors classified by text: `bin/ruxen_ide/src/code_actions.rs:122`, `bin/ruxen_cli/src/build.rs:575`.
  - Lexer copies the remaining source per numeric literal (`core/lexer/tokens.rs:289`) and repeats the dispatch in `core/lexer/format_specs.rs:100`.
  - Stale `allow`s and unused parameters (`bin/ruxen_cli/src/resolve_deps.rs:83`, `code_actions.rs:24`, `bin/ruxenc/src/compile.rs:441`).
  - No `TopLevelItem::span()`.
- Fix: `title()` from REGISTRY; match on `d.code`; `NumericSuffix::as_str`; shared `lex_one_token`.

**F-45 Stale documentation.** Kind: other-debt. Severity: low. Effort: S.
- Instances:
  - `core/codegen/CONTEXT.md:14-64` contradicts contract 1 ("edit BOTH ABI tables") and mis-describes `runtime/` and the wasm core.
  - 14 `std/*/runtime/*.c` banners refer to a `runtime.c` unity build that no longer exists.
  - `runtime_table` references in `core/codegen/lang_intrinsics.rs:35` and stdlib headers (`std/sync/src/thread.rx:14`, `std/net/src/lib.rx:9`, `std/array/src/lib.rx:39`, …).
  - Typeck resolver headers.
  - `docs/architecture/repo-layout.md:22-78` and `../CLAUDE.md:65` describe `ruxen_driver`.
- Fix: update in the same commit as the owning family, per the workspace `CONTEXT.md` rule.

## Dropped candidates

- **Concurrency surface hard-wired to Int payloads** (`Queue`, `Race`, `Stream`, `Schedule`, `spawn`): a documented roadmap item, not cleanup. Track it as one ticket.
- **`block_on` as a compiler rewrite** (`core/async_lowering/lower.rs:2591-2900`): blocked on F-21 generic dispatch. Revisit afterwards.
- **Generic `Atomic[T]` `.rx` class**: needs a word-sized marker mixin. The C aliasing part stays in F-39.
- **Parser bare-call allowlist** (`puts`, `print`, …): changing it is a grammar design decision. The literal `&block` name and the lowercase primitive type names stay as lows in F-32.
- **`init` and `drop` in the default-Unit return list** (`core/typeck/infer/mod.rs:272`): language hooks, a legitimate closed set. The `display`/`display_all` part is kept in F-21.
- **Send/Sync, Future/Stream/Poll/Context and `BUILTIN_MACRO_NAMES` as name sets**: legitimate closed sets. Only their duplication or retired entries are kept (F-14, F-28, F-31).
- **`unreachable!()` at `core/module_loader.rs:419,422`**: correctly guarded, as the auditor noted.
- **`is_float_ty` duplicated in `core/typeck/infer/ops.rs:429`**: too small to be worth a change.
- **`.await` special-cased twice in typeck**: trivial. Folded into F-44, not tracked separately.
- **Unify `&T`/`T` hole as a release blocker**: downgraded to F-41 (medium). It is documented and tracked, and the fix has a high blast radius.
- **"`is_synthetic` copied 6 times"**: recounted to 4 local copies. Kept in F-35.

## Coverage gaps

- **Nothing was built, run or coverage-measured.** Every runtime-behaviour claim comes from reading the code. The installed toolchain is too old to parse current fixtures, so the fmt corruption could not be reproduced. Rebuild with `ruxen upgrade --from-source` before acting on F-4, F-5, F-6 and F-7.
- **Not audited by anyone:**
  - `bin/ruxen_lsp/src/server.rs` bodies (outline only).
  - The `ruxen_cli` package manager: `deps`, `lock`, `manifest`, `publish`, `scaffold`, `self_update`, `target`, `version`, `module_discovery` (grepped only).
  - `tests/`, `benches/`, the release-e2e harness scripts, `scripts/`, `docs/` (beyond cross-references), `examples/`.
  - The vendored `pcre2/`.
  - canvas, quiver and rondo (checked only as callers of dead-code candidates).
- **Partially audited:**
  - mir: `nodes.rs`, `mir/tests.rs`, `lower/impl_block.rs`, most of `expr/*.rs`, about 1,300 lines of `drops.rs`, about 1,700 lines of `mod.rs`.
  - resolve: `exprs.rs:800-1100`, `yield_scan.rs`, `implicit_block_params.rs`, `hir/nodes.rs`.
  - front: about 900 lines of `format_expr.rs`, `parser/patterns.rs`, `match_expr.rs`, `parser/ffi.rs`, the printer bodies.
  - typeck: large ranges of `mixins.rs`, `collect.rs`, `ops.rs`, `unify.rs` and `expr.rs`, plus all typeck and borrow-check test files.
  - backend: `core/codegen/object.rs:450-818` (the wasm C shim) and `:914-1100`, `target.rs`, `llvm/debug.rs`, `llvm/emit/instructions.rs:800-960`, the `async_lowering/cfg.rs` bodies.
  - runtime-c: `queue.c`, `test_clock.c`, `mutex.c`, `sharedsync.c`, `thread.c`, `signal.c`, `env.c`, `time.c`, `rand.c`, `base64.c`, `bench.c`, `process.c`, most of `json.c`, `regex.c` and `reactor.c`.
- **Method limits:** the dead-symbol scans are regex-based, so C functions with multi-line signatures and private Rust fns or types may be missed. Claims marked PLAUSIBLE by auditors (`collect`/`*Iter` being dead, double receiver lowering in closure inlining, the protected-keyword double emission, the rlib exports being write-only) were not confirmed.