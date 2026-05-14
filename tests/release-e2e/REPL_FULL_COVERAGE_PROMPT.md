# Prompt: Bring the Riven REPL to full rivenc parity

You are working in `/Users/hassan/.projects/riven/`. Rivenc (the batch compiler) handles every tutorial feature correctly; the REPL (`crates/riven-repl/`) handles a subset. The goal of this task is to close the gap so every `.rvn` fixture under `tests/release-e2e/cases/` that rivenc compiles also runs correctly through `riven-repl` via piped stdin.

## Hard rules — read first

- **Memory cap.** Every `rivenc` invocation must go through `./tests/release-e2e/scripts/rivenc-capped` (30 s wall-clock + 8 GiB RSS kill). For direct testing, prefer `cargo test --release --test installed_binary -- <filter>` or `cargo test --release -p riven-repl -- <filter>` with an **explicit filter**. Never run the full workspace test suite unfiltered.
- **Watchdog.** A background watchdog at `/tmp/mem_watchdog.pid` kills any rivenc / rustc / cargo / ld / clang / riven-repl process that exceeds 8 GiB RSS, and deletes any file over 1 GiB under `target/`, `/tmp/`, or `<repo>/tmp/`. If it fires on your change, stop and investigate — don't retry unbounded. If the watchdog isn't running, start it: `nohup tests/release-e2e/scripts/mem_watchdog.sh >/tmp/mem_watchdog.log 2>&1 &`.
- **No commits.** Leave all changes unstaged on `master`. The user reviews via `git diff`.
- **After each cargo test**, verify `find target/release/deps /tmp /Users/hassan/.projects/riven/tmp -size +500M 2>/dev/null` is empty.
- **Build with LLVM feature**. The shipped workspace uses the LLVM backend for `riven-cli` / `rivenc`:
  ```
  export LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18
  export PATH=/opt/homebrew/opt/llvm@18/bin:$PATH
  export LIBRARY_PATH=/opt/homebrew/opt/zstd/lib:/opt/homebrew/opt/llvm@18/lib
  cargo build --release --workspace --features "rivenc/llvm riven-cli/llvm"
  ```
  (The REPL itself doesn't use LLVM — just Cranelift JIT — but a workspace build must keep LLVM deps resolvable.)

## Architecture recap (skim before editing)

- `crates/riven-repl/src/main.rs` — stdin loop. Detects non-TTY and reads the whole stream, splitting into chunks via lexer-balance (see `split_repl_chunks`).
- `crates/riven-repl/src/session.rs` — persistent state: `func_defs: Vec<FuncDef>`, `let_bindings: Vec<LetBinding>`, `type_items: Vec<TopLevelItem>` (class/struct/enum/trait/impl/const/etc.), plus `env` (ReplEnv) and `jit` (JITCodeGen).
- `crates/riven-repl/src/eval.rs` — per-input pipeline: lex → parse → typecheck → borrow-check → MIR lower → JIT compile + execute. The `build_program` helper replays all prior `type_items` + `func_defs` into each new program before the wrapper fn.
- `crates/riven-repl/src/jit.rs` — Cranelift JIT, mirrors the batch codegen at `crates/riven-core/src/codegen/cranelift.rs` but with per-instruction differences.
- Fixtures live at `tests/release-e2e/cases/NN_name.rvn`; expected stdouts at `tests/release-e2e/expected/NN_name.out`. The REPL translator at `tests/release-e2e/scripts/translate_to_repl.py` strips the `def main ... end` wrapper.
- The harness is `tests/release-e2e/run.sh`. The relevant function is `test_repl_cases` + the `REPL_KNOWN_SKIP=(...)` list. **Your success criterion is: every fixture currently in `REPL_KNOWN_SKIP` should move to the PASS column, and the `REPL_KNOWN_SKIP` array should become empty.**

## The 47 gaps, grouped by root cause

Each group below lists the fixtures, the root cause, and a concrete fix direction. Work them in the recommended order — earlier groups unblock later ones.

### Group A — JIT should mirror batch codegen (single fix, biggest payoff)

Root cause: `crates/riven-repl/src/jit.rs` diverges from `crates/riven-core/src/codegen/cranelift.rs` on several instruction paths. Every `MirInst` variant must lower identically in both backends. The batch codegen is correct; port its logic.

**Fixtures (15):**
- `11_match_guards` — match guard lowering in JIT
- `28_closures`, `89_closure_capture_immut`, `90_closure_capture_mut`, `91_move_closure`, `92_closure_as_arg`, `93_yield_block` — closure indirect-call + captures struct (see batch impl in `mir/lower.rs` + `codegen/cranelift.rs`)
- `48_borrow_mut` — `RefMut` for `&mut String` emits `stack_addr` through the recent Ref/RefMut ABI fix (check `cranelift.rs::run_function_inner`'s pre-scan for `is_string_mir_ty` and mirror in jit.rs)
- `34_char_literal`, `40_int_sized`, `41_uint_types` — narrow-int result readback. The JIT wrapper's return value is currently reinterpreted as i64 for display; see `display::format_result` / `jit::run_function_inner`. For Char, use `riven_char_to_string`; for Int32/UInt8, widen before display.
- `54_loop_break_value` — loop + break-with-value `LoopFrame { result_local }`
- `59_do_end_block` — do-end-as-expression returns tail value
- `70_method_chain` — methods returning `Self` in JIT

**Fix strategy.** Walk every `MirInst::...` arm in `jit.rs`'s emission and diff it against `cranelift.rs`. Bring missing arms over. Start with `match` guards and closures — those unblock the most fixtures. Write a tiny cargo test per repaired arm (one `e2e_NN_name` filter at a time) to verify without running the full suite.

### Group B — `let mut` reassignment persistence across inputs

Root cause: `session.let_bindings` stores each binding's ORIGINAL RHS. When the user types `y = y + 1` in a later input, the replayed `let mut y = 5` resets `y` before each eval.

**Fixtures (4):** `06_let_mut`, `15_class_mut`, `52_nll_basic`, `70_method_chain` (the mutation side; method chaining itself is Group A).

**Fix strategy.** After each successful eval, read the current value of every mutable binding out of the JIT and update `session.let_bindings[i]` to use a literal initializer matching the current value. Implementation sketch:

- Extend `ReplEnv` / `JITCodeGen` so bindings live in actual stack cells, not rebuild-each-eval locals. Alternatively, at end of each eval, execute a synthetic wrapper that returns each mutable binding's current value, then patch the session's `LetBinding` initializer to a literal of that value.
- For heap values (String, Vec, class instances), storing a literal is hard. Safer: promote each mutable let to a **heap-allocated cell** (like closures' cell promotion in `mir/lower.rs::promote_to_cell`) on first declaration; the cell's address becomes the binding. All subsequent reads/writes go through the cell. Replay in future evals = restore the cell pointer, not re-run the initializer.

Use Group A's closure capture-cell pattern as prior art (see `CaptureKind::ByRef`).

### Group C — Trait / impl resolution in JIT

Root cause: trait method dispatch and default-body monomorphization work in batch codegen (see `mir/lower.rs::collect_trait_default_methods` and `impl`-block method synthesis), but the REPL compiles the trait/impl pair in one pass and may not re-emit synthesized method bodies when new impls arrive.

**Fixtures (10):** `21_mixins`, `22_mixin_default`, `79_mixin_inherit`, `80_mixin_assoc_type`, `81_mixin_static_method`, `82_some_mixin_param`, `83_any_mixin_param`, `84_multi_bound`, `86_mixin_default_method_used`, `87_mixin_override_default`.

**Fix strategy.** In `eval.rs`, the `other` branch already lowers the full replayed program through MIR + JIT, but `session.jit.is_declared(&mir_func.name)` may skip re-emitting a default-method monomorphization that was synthesized AFTER a class impl was added. Ensure the JIT compiles every MIR function it hasn't seen yet — including those with mangled names like `Bot_greet` or `Cat_speak`. Log what's being compiled during a failing fixture with `--verbose` and cross-check against a batch compile's `--emit=mir`.

### Group D — Runtime method resolution in JIT

Root cause: Vec/Hash/Set/String method calls rely on runtime helpers registered in `crates/riven-core/src/codegen/runtime.rs`. The JIT may not import every runtime symbol, or the resolver maps methods to names the JIT doesn't know.

**Fixtures (6):** `45_string_methods`, `57_while_let_pop`, `104_hash_basic`, `105_set_basic`, `106_string_chars`, `107_array_push_pop`.

**Fix strategy.** In `jit.rs`'s runtime-symbol registration section, mirror every entry from `crates/riven-core/src/codegen/cranelift.rs::declare_runtime_funcs` (or whatever the batch uses). Grep both files for `riven_string_*`, `riven_vec_*`, `riven_hash_*`, `riven_set_*` and ensure parity.

### Group E — Generic / where-clause method resolution in JIT

**Fixtures (2):** `100_where_clause`, `103_generic_constraint`.

Root cause: `typeck/traits.rs::lookup_method_on_bounds` + `mir/lower.rs::unique_bound_impl` handle multi-bound generics in batch. The REPL's replay pass may drop type-variable context mid-stream.

**Fix strategy.** After Group C, re-run these and see if they pass as a side effect. If not, trace which mir function's MIR isn't being emitted; it's likely a monomorphization of `Int_to_display` or similar that the JIT isn't picking up on the replay.

### Group F — Error handling in REPL context

**Fixtures (5):** `94_custom_error_enum`, `95_error_into_conversion`, `96_panic_basic`, `97_expect_ok`, `99_map_option`.

Root causes:
- `panic!` calls `riven_panic` → `exit(101)`, which terminates the REPL process mid-session. For fixture 96, that's the correct semantic — the REPL should print the panic's stdout, then exit. The harness's translator keeps `panic!(...)` as a top-level statement; verify the REPL actually runs it and flushes stdout before exit.
- Custom Error enum + `impl Error for MyErr` relies on Group C.
- `Option.map`, `Result.expect!`, `Into::into` via `?` rely on Group A (closures) + Group D (runtime).

**Fix strategy.** After Groups A+C+D, retest. For `96_panic_basic`, ensure `riven_puts` flushes stdout before `riven_panic` runs exit — the C runtime may need `fflush(stdout)` inside `riven_panic` and every printing helper.

### Group G — Chunker / lexer edge cases

**Fixtures (5):** `29_comments`, `37_multiline_string`, `109_fmt_off`, `110_nested_block_comment`, `111_doc_comments`.

Root cause: `main.rs::split_repl_chunks` uses the lexer's token balance to decide when to emit a chunk. Nested block comments (`#= ... =#`), multi-line strings (`"""..."""`), and `# fmt: off` blocks may tokenize in ways that leave the splitter's balance off.

**Fix strategy.** Extend `split_repl_chunks` to treat open multi-line-string and open nested-block-comment as "incomplete" the same way an unclosed `def` is. Then test each fixture individually — they should each emit as a single chunk.

## Workflow for each group

1. Pick a fixture from the group. Read it and its expected output.
2. Translate it to REPL input:
   ```
   python3 tests/release-e2e/scripts/translate_to_repl.py < tests/release-e2e/cases/NN_name.rvn
   ```
3. Pipe it through the REPL and see the current behavior:
   ```
   python3 tests/release-e2e/scripts/translate_to_repl.py < tests/release-e2e/cases/NN_name.rvn \
     | timeout 15 ./target/release/riven-repl 2>&1 | sed -E 's/\x1b\[[0-9;]*m//g'
   ```
4. Compare against expected. The divergence usually points to a specific MIR instruction, runtime fn, or replay path.
5. Make the smallest possible fix in `crates/riven-repl/` (and occasionally `crates/riven-core/` — preferred only when the batch path already uses the logic and the REPL is missing it).
6. After your fix, remove that fixture from `REPL_KNOWN_SKIP` in `tests/release-e2e/run.sh`.
7. Re-run: `./tests/release-e2e/run.sh` and confirm no regressions AND the fixture now passes.

## Success criteria

- `REPL_KNOWN_SKIP=()` in `tests/release-e2e/run.sh` (empty list).
- `./tests/release-e2e/run.sh` exits 0.
- The summary shows `rivenc/--backend=llvm` and `repl/session` and all 111 `repl-case/*` lines as PASS.
- No fixture is marked SKIP.
- Full harness: **274 / 274 passing** (163 rivenc-side + 111 REPL-side).

## Before you start

Build the workspace and confirm the current state:
```
export LLVM_SYS_180_PREFIX=/opt/homebrew/opt/llvm@18
export PATH=/opt/homebrew/opt/llvm@18/bin:$PATH
export LIBRARY_PATH=/opt/homebrew/opt/zstd/lib:/opt/homebrew/opt/llvm@18/lib
cargo build --release --workspace --features "rivenc/llvm riven-cli/llvm"
```

Then run the harness once to baseline. Current result should be 227/227 passing with 47 REPL-case skips. Your job is to eliminate those skips.

## Output to produce

When done, write a short report:
- Which groups you addressed.
- Files touched (paths only).
- Final harness totals: `grep 'total:' tests/release-e2e/results/full_run.txt`.
- Any fixture that resisted the fix and why.
