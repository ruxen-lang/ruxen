# Plan to cut the Ruxen test suite to 1-5 minutes

The plan merges five audit inventories into one. It cuts the test run by deleting tests that duplicate e2e fixtures, moving stdlib behaviour tests into `library/std/<pkg>/tests`, merging integration-test binaries, and giving agents a rule to run only the tests their change touches.

The `wt-audit` checkout no longer existed when I ran. I read everything from commit `65aab1c` (branch `feat/concurrency-adoption`, 595 e2e cases) using `git show` and `git archive` into the scratchpad. Nothing in the repo was edited, and no tests were run.

## 1. Where the time goes today

| Area | Estimate | Reasoning |
|---|---|---|
| Rebuild and relink after any edit | 2-6 min per edit cycle | `compiler/ruxen_core/build.rs` has no `rerun-if-changed`, so editing any test or fixture in the crate reruns it, recompiles ruxen_core and relinks all 178 integration binaries (median 48 MB each). A cold lib build took 127.6 s. `src/ruxen_repl/build.rs` watches all of `library/std`, so any stdlib `.rx` edit rebuilds the 68-file C runtime. |
| Rust tests that compile, link and run a `.rx` program | 5-8 min | About 500 such tests at roughly 1 s each (measured: 5 tests took 4.8 s). `src/ruxenc/tests/installed_binary.rs` alone took 130.7 s. Test binaries run one after another, so 1-test binaries leave the other cores idle. |
| `run.sh` e2e sweep | 15-30 min or more | `JOBS=1` by default, over 595 fixtures. One compile measured 0.335 s, and the memory poller rounds each compile up by 0.25 s. The default command takes the slow path (about 30 `cc -c` per fixture) unless `RUXEN_WORKSPACE` is set. The REPL phase replays all 595 fixtures again (15 s timeout each), and the parity phase repeats `syntax_parity`. CI never runs `run.sh`. |
| `release_e2e_smoke` | about 3 min | Already parallel and already the CI gate (`pr-ready.yml`). |
| Unit tests (about 980) | seconds | In-process, one binary per crate. |

## 2. Harness changes

| Change | Expected saving |
|---|---|
| Add `cargo:rerun-if-changed=build.rs` to `compiler/ruxen_core/build.rs` | Stops the full ruxen_core rebuild on every test edit. Likely the largest single win per edit cycle. |
| In `src/ruxen_repl/build.rs:180`, watch each `library/std/<pkg>/runtime` instead of `library/std` | Stdlib `.rx` edits and new `tests/*.rx` files stop rebuilding the C runtime. |
| Set `autotests = false` and use one `tests/it/main.rs` per crate | 178 binaries become about 6: ruxen_core `it`, `release_e2e_smoke` (kept separate because CI calls it by name), the llvm-gated files as `#[cfg] mod`, plus one each for ide, cli, ruxenc and repl. |
| Prepare the merge | Process-wide env vars (`drop_fixtures.rs:447`, `RUXEN_RUNTIME_AR`) become an explicit path or a `OnceLock`. Shared helpers go in `common/mod.rs`, replacing about 57 copy-pasted `compile_and_run` helpers. |
| Make `release_e2e_smoke` the only fixture gate | Remove the `run.sh` compile phase from the done gate. Agents filter with `RUXEN_E2E_CASES`. |
| `run.sh` fixes | Default `JOBS=$(nproc)`, with a serial tail for 508, 518, 534, 536, 727 and 727b. Find `libruxenrt.a` automatically (the fast path). Use `prlimit` instead of the 0.25 s `ps` poll on Linux. Limit REPL parity to a curated list and take it out of the default phases. Cut the parity phase to a one-file smoke. |
| Fix `error_code_registry::collect_emitted_codes` | It walks a `crates/` folder that does not exist, so it passes without checking anything. Point it at `compiler/` and `src/`. That lets `implicit_codes_registered.rs` go. |
| `ruxen test` for stdlib packages | `src/ruxenc/src/test_runner.rs:307` must skip merging `src/lib.rx` for std packages; that merge causes `DuplicateDefinition("Set_reject")`. Bypassing it, a Set test passed in 0.22 s. Export `RUXEN_STDLIB_PATH`, and add `scripts/test-stdlib.sh` that runs one process per package in parallel. |
| CI | Drop `cargo build --workspace --all-targets` (it only adds 6 benches and plotters; run `cargo check --benches` in the lint job instead). Turn off criterion's `plotters` feature. Set `[profile.dev.package."*"] opt-level=2` (measure before and after). Adopt nextest with a `serial` group for drop_fixtures, publish_e2e and the port tests. Use mold or lld in CI: the committed `.cargo/config.toml` linker path only exists on this machine. |
| CI lanes | **Fast lane** on every push: unit, `it`, filtered smoke, `test-stdlib`. **Slow lane** at PR-ready: full `release_e2e_smoke` on both OSes, the llvm feature, and `run.sh` cli/lsp/binaries. |

## 3. Deletions

| Target | Reason | Still covered by |
|---|---|---|
| `enum_codegen_debug.rs`, `enum_mir_debug.rs`, `lex_sample.rs`, `parse_sample.rs`, `typecheck_sample.rs`, `borrow_check_sample.rs` | Milestone scaffolding. `borrow_check_sample` and `typecheck_sample` cannot fail (no assertion of zero errors). `lex_sample` pins retired tokens. | e2e 14, 18, 19, 05, 20, 72, 17, and `syntax_parity`. **Change:** first promote `fixtures/sample_program.rx` (501 lines) to one e2e case, because it is the only whole-program smoke. |
| `cranelift_share_pin`, `ffi_alias_single_entry`, `drop_registration_single_entry`, `registration_ctx_pin`, `send_sync_single_entry` | Grep the source for private names. | Build of `ruxen_repl/src/jit.rs`, `ffi_c_symbol_wired`, `drop_fixtures`, `trio_leak_pin`, `send_bound_hint` |
| Rust files that re-run e2e fixtures: `option_result_runtime`, `unreserved_idents_runtime`, `numeric_conversions_runtime`, `to_s_runtime`, `pattern_typing`, `q3`, `q1`, `q46`, `q26`, `q28`, `q33`, `q29`, `user_drop_runs`, `implicit_debug_formats_enum`, `implicit_partial_eq_returns_correct`, `mixin_defaults`, `mixin_lib_decl_propagation`, `module_class_qualified_type`, `spike_enumerable_mixin` | Same program and same output as an e2e fixture. | The named fixtures (810, 809, 740, 637, 643, 460, 615, 616, 647-654, 134, 200, 210, 212, 206, 86, 570, 646, 612) |
| The compile-and-run halves of `q17`, `q25` and `q31` (their E1118 negatives stay), plus `stdlib_bufio` `e2e_mirror_*` | They run the e2e fixtures. | 655-659, 474, 617-619, 652, 570, 574 |
| Resolution pins (no runtime run): `bootstrap_class_bodies`, `builtin_receiver_bridge`, `collection_builtins_resolve`, `builtin_anchor_class`, `lib_in_class_body`, and the async surface/executor/lowering HIR-shape pins | Private plumbing. Any e2e program using these classes breaks if they regress. | e2e 518, 570, 612, 1060-1065, 721-731 |
| Compile-and-run cases in `block_tier2` (16), `alias_keyword` (4), `bang_names` (2), `ffi_c_symbol_wired`, `lib_in_class_body_wired`, `bootstrap_prelude_merge`, `async_io` (5) | Same programs as the e2e fixtures. The negative tests stay. | 943-946, 1780, 470-472, 913, 1880-1882, 725, 726, 722, 728f. **Change:** fold `xs.map do _1 * 5 end` into 1780 first; no e2e case has a no-parameter `do` block using `_1`. |
| Typecheck-only "positive" files: `stdlib_map`, `stdlib_set`, `stdlib_fmt`, `stdlib_array_negatives`, `stdlib_iterator` (except E0700), `stdlib_fmt_display_dispatch`, and the 11 `const_generics` "does not emit X" tests (replace with one fixture that must typecheck with zero errors) | Weak or unable to fail. `vec_with_capacity_string_arg_is_currently_accepted_at_typeck` pins a known bug, so it becomes a ticket. | 401-411, 501-511, 521-527, 601-611, 070, 071, 139 |
| Tests that only pin retired syntax: the `&str` assertion in `string_literal_wrap`, `string_dot_from_is_now_an_unknown_method`, `flat_tcp_free_fns_removed`, and `installed_binary` `newtype`/`[Int; 3]`/`ref` | Rust syntax the language has dropped. | Nothing needed |
| `stdlib_file`, except `file_create_truncates_existing` | One test per fixture. **Change:** 511 does not test truncation, so that test moves (see section 4). | 510-519 |
| `stdlib_fs` (10), `stdlib_net` (7), `stdlib_time` (6), `stdlib_fmt_runtime` (8), `stdlib_array_runtime`, `stdlib_json`, `stdlib_rand` (5) | Same programs as the fixtures. | 530-538, 507, 540-545, 550-555, 070, 071, 143-147, 800, 801, 950, 590, 593, and `runtime_safety` |
| `parser_feature_parity::shipped_stdlib_and_examples_all_parse`, `formatter_corpus_roundtrip.rs` | Each checks a subset of what `syntax_parity` already checks. Update the guard named in `ruxen/CLAUDE.md`. | `syntax_parity::axis_compiler_corpus_parses` and `axis_fmt_reparse_identity` |
| `installed_binary.rs`: 59 of 63 tests and their fixtures | Same programs as the e2e cases (3 sampled and confirmed byte-identical). Keep the 3 install-layout tests. | e2e via smoke. Saves about 125 s. |
| `p05_e2e_check.rs`, `repl_tests.rs` (`check_balanced` tests a copy of the validator written inside the test file; two tests cannot fail), duplicate `--version` and `:type` tests, `package_manager.rs` (20 tests that repeat unit tests), 14 duplicate IDE tests, `tests_migrated.rs` | Duplicates, or tests that cannot fail. | The unit tests named in the inventories, `split_chunks.rs`, `installed_repl` |
| e2e cases 201, 85, 131, 132, 133, 1886, 935, 933, 728e, 701, 703, 105, 99, **419** (keep 422), and the two `.deferred` files | Byte-identical, a strict subset, or unable to fail (132 never hashes). | 139, 63, 50, 207, 206, 96, 01, 934, 728f, 702, 710, 521, 115, 422 |
| `#[ignore]` TODO placeholders (11) | Never run, so they cannot fail. | Tickets and spec deferral notes |

**Totals:**
- About 335 Rust `#[test]` deleted.
- About 40 integration files deleted outright.
- About 335 compile-and-run tests leave `cargo test`.
- 16 e2e files deleted.

## 4. Moves into `library/std/<pkg>/tests` (Tester `.rx` files)

| Source | Package |
|---|---|
| Rest of `stdlib_fs` (13, including the atomic-write failure, which 535 does not cover) and `stdlib_file::file_create_truncates_existing` | fs, io |
| Rest of `stdlib_time` (5; use `it_panics` for `duration_since_future`), `stdlib_rand` (3), `stdlib_path` (4, not covered anywhere else) | time, rand, path |
| Rest of `stdlib_fmt_runtime` (3 Formatter tests), `stdlib_bufio` (3 File tests) | fmt, bufio |
| Rest of `stdlib_net` (8; both ends in Ruxen, port 0), `stdlib_process` `command_*`, `stdlib_env`, `stdlib_io` IoError tests (collapse to 2) | net, process, env, io |
| `stdlib_test` matcher and capture tests (keep the summary, fork and `it_panics` checks outside the framework) | test |
| About 225 e2e stdlib fixtures: strings (about 45 into 6 files), arrays, hash, set, iter, option/result, scalar, kernel p/pp, fs, net, bufio, io, time, rand, process, regex, json, base64, http, env, sync, and future/executor 412-449 | the matching packages |
| 576 and 577 (BufReader.Tcp / BufWriter.Tcp) | bufio, with a real peer. **Change:** these are the only compile cover for the Tcp suffix, so do not delete them before they move. |

**Missing `ruxen test` support:**
- The `test_runner.rs:307` fix for std packages.
- `RUXEN_STDLIB_PATH` wiring.
- The `scripts/test-stdlib.sh` runner, with a CI step.
- An "exit code and stdin piping" mode doesn't exist. Until it does, `stdin_lines_*`, process exit codes, 1182, 1183, 1887 and 518 stay in Rust or e2e.

Not checked: whether 949 (`http`) uses the network.

## 5. Merges

- **e2e merges, 51 files into 14:**
  - 1720-1729 into 3 files
  - 1730-1739 into 2
  - 1860-1866 and 1869 into 1
  - 1880-1885 into 1
  - 1000-1004 into 1
  - 1850-1852 into 1
  - 631 and 632
  - 126 and 209
  - 57 and 129
  - Fixture number collision: two cases start with `213_`, and one of them has no `.out`. Rename it to `213b`, add the `.out`, then delete `implicit_debug_formats_struct.rs`.
- **New e2e fixtures replacing Rust tests that print to stdout:**
  - `any_fn_dyn_dispatch` (4 tests)
  - derived Ord/Hashable through a generic bound
  - async enum-variant tail with a local crossing an await
  - q2 `Option[any Fn]`
  - literal escapes and suffixes
  - stdout tests from `ruby_block_semantics` and `yield_self`
  - `struct_method_field_codegen` leftovers
  - `sample_program.rx`
- **Rust files merged into named modules of `tests/it`:**
  - `send_sync` (3 files)
  - `repo_lints` (7 static checks)
  - `stdlib_diagnostics` (E0615, E0700, E0714, E1702/E1704, use-after-move)
  - `diagnostic_hints` (4)
  - `runtime_tag_stability` (2)
  - `qualified_names` (4)
- **Parser AST-shape tests** go into `parser/tests.rs`, including the 7 `parse_repl_input` tests from `repl_tests`. `render_fn_signature` moves into the formatter's unit tests, and the lexer proptests move out of `runtime_safety`.
- **Bulk merges:** 17 ruxen_ide binaries become 1, and 8 ruxen_cli binaries become 1.
- **Optional table tests:** the 17 layout tests and 16 Copy/Move tests.

## 6. Run only what's needed (rule for agents)

| Changed path | Run |
|---|---|
| `ruxen_core/src/{lexer,parser}/**` | `cargo test -p ruxen_core --lib lexer:: parser::`, then `--test it syntax_parity` and `--test it formatter`, then smoke with `RUXEN_E2E_CASES` set to fixtures that use the construct (found by grep) |
| `ruxen_core/src/{resolve,typeck,hir}/**` | `--lib <phase>::` plus `--test it <feature_mod>::` plus matching e2e cases |
| `ruxen_core/src/{mir,borrow_check,async_lowering,codegen}/**` | `--lib <phase>::` plus `it drop_fixtures::` (for drop or MIR) plus matching e2e cases |
| `ruxen_core/src/formatter/**` | `--lib formatter::` plus `it syntax_parity::axis_fmt` plus `-p ruxen_ide` format tests |
| `library/std/<pkg>/src/*.rx` | `scripts/test-stdlib.sh <pkg>` |
| `library/std/<pkg>/runtime/*.c` | the line above, plus `it runtime_abi_derivation` and `runtime_safety` |
| `diagnostics/codes.rs`, `docs/errors/**` | `it repo_lints::error_code_registry` plus cli `explain` |
| `src/ruxen_ide/**` | `-p ruxen_ide`. For `src/ruxen_lsp/**`, add `-p ruxen_lsp`. |
| `src/ruxen_cli/**`, `src/ruxenc/**`, `src/ruxen_repl/**` | `-p <crate>` |
| `tests/release-e2e/cases/NNN_*` | `RUXEN_E2E_CASES=NNN_*` |

Ship this as `scripts/affected-tests.sh` (it maps `git diff --name-only` to the commands above) and point `ruxen/CLAUDE.md` at it. The full workspace run plus the full smoke run once, at the done gate or in CI's slow lane.

## 7. Order of execution and risks

1. Harness first, because it is safe and has the biggest effect: the `build.rs` rerun fix, `JOBS`, the `run.sh` fast path and poller, the scanner fix, and the CI build-step removal.
2. Fix `ruxen test` for std packages and add `test-stdlib.sh`.
3. Add the new e2e fixtures and the `213b` `.out`. Promote `sample_program.rx`. Fold the `do _1` case into 1780.
4. Delete the Rust duplicates and the e2e duplicates.
5. Move stdlib tests one package at a time. Delete a source fixture only after its `.rx` test is green.
6. Collapse binaries into `tests/it` and switch to nextest.
7. Update `CLAUDE.md` (guard name, done gate, test locations) and `CHANGELOG`.

**Risks:**
- Leak checks happen only in `drop_fixtures`, since e2e has no leak mode. Keep its 34 tests until `run.sh` has one.
- One shared `it` binary shares process-wide env vars across tests. Fix `set_var` before merging.
- Port and tmp-path collisions once tests run in parallel. Mitigate with the serial tail list and port 0.
- Stdlib `.rx` tests are untested territory today: zero files exist, and one Set test is the only proof the runner works.
- The CI runner does not have sibling repos, so `syntax_parity`'s corpus there covers only this repo.
- Not verified: the n-z and e2e "PLAUSIBLE" items. All timing figures except the measured ones are estimates; measure before and after step 1.

**Corrections made during my 15+ spot checks:**
- **Keep 422:** it is a superset of 419 (it adds the check that the winner's drop must not fire), so delete 419 instead.
- **Keep 576 and 577:** they are the only compile cover for BufReader.Tcp and BufWriter.Tcp.
- **Keep 728d:** it is the single-await loop case. Only 728e goes, and `async_io` now points at 728f.
- **`stdlib_file`:** keep its truncation test, because 511 does not cover it.
- **`block_tier2`:** the no-parameter `do _1` case has no e2e cover yet, so fold it into 1780 before deleting.
- **`sample_program.rx`:** promote it to e2e rather than dropping it.

**Confirmed as they stood:**
- `user_drop_runs` matches 134.
- `to_s_runtime` matches 810.
- Three `installed_binary` fixtures are byte-identical to their e2e cases.
- `repl_tests` tests a copy of the validator, not production code.
- `typecheck_sample` and `borrow_check_sample` cannot fail.
- `formatter_corpus_roundtrip` checks a subset of `syntax_parity`'s corpus.
- 132, 85, 133, 99, 1886 and 201 are safe to delete.

Files are in `/tmp/claude-1000/-home-sheraz-Documents-ruxen-lang-ruxen/050e9991-43dd-44a4-930d-94a7a7074127/scratchpad/c/` (an extract of `65aab1c`; read-only copy).