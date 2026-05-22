# v1 prompts — actual progress status

Last audited: 2026-05-22. Status reflects in-tree code state, not
just doc checkboxes (which were not maintained as work landed).

Each remaining `*.md` file in this directory carries its own status
header summarising current state + evidence pointers.

## Session 2026-05-22 — CI + correctness sweep

Six commits landed (`b6abe74 … 79be087`). None of the remaining v1
prompts moved out of their bucket; the work was greening the suite and
shoring up correctness gaps surfaced by the
[`docs/quality_review.md`](../../quality_review.md).

| Area | Pre-session | Post-session |
|---|---|---|
| `release_e2e_smoke -- --ignored` | 287 / 291 (4 failing) | **291 / 291** |
| `cargo test -p riven_core --tests` (macOS) | 9 failing | **1066 / 1066** |
| `cargo test --workspace` (linux, gcc) | red on build + 2 test failures | **1643 / 1643** |
| `cargo build --workspace --all-targets` | red (`riven_repl` non-exhaustive match + `Future_dynamic_poll` link miss) | **green** |
| `cargo +1.91 build --workspace` (MSRV) | red (same chain) | **green** |
| `cargo fmt --all -- --check` | 205 diffs in 74 files | **green** |

Compiler fixes shipped this session (each unblocks a documented
soundness or correctness pin):

- `mir/lower/function.rs` — primitive-receiver `self` type for
  `extension Int { … }` (Cranelift was dropping the zero-sized
  param while callers still passed it; `100_where_clause` +
  `103_generic_constraint` were stuck).
- `resolve/items.rs:332` — `resolve_class` now preserves
  `DefKind::TypeAlias` instead of stomping it for `String`. Closes
  `project_riven_resolve_class_stomps_typealias.md` at the write
  site; the read-side patches in `resolve/types.rs:516-548` are now
  redundant (left in for defence-in-depth).
- `typeck/infer/collect.rs` — `lookup_on_type_param_bounds` peels
  `Ref/RefMut/RefLifetime` so `def f[T: Mixin](a: &T)` dispatches.
- `library/std/{core,hash}/src/lib.rvn` — `Hashable.hash_code`,
  `Ord.cmp`, `PartialOrd.partial_cmp` now carry their real return
  types (`-> Int`) instead of defaulting to `Unit`.
- `mir/lower/drops.rs` — Command builder receiver is force-tainted on
  `arg/args/env/current_dir`, ending the SIGABRT-on-exit double-free
  that affected every `Command.new(…).arg(…).status` chain.
- `library/std/sync/runtime/atomic.c` — bool `fetch_and/or` rewritten
  as compare-exchange loops (gcc rejects the Clang-only
  `atomic_fetch_and(&_Atomic _Bool, …)` extension).
- `riven_repl/src/jit.rs` — `MirInst::DataAddr` arm added; non-
  exhaustive match was breaking `cargo build --workspace --all-targets`.
- `riven_repl/{build.rs,runtime_stubs.c}` — weak
  `Future_dynamic_poll` stub satisfies the librivenrt link without
  changing AOT codegen behaviour.
- `riven_ide/src/line_index.rs` — UTF-8 char-boundary safety via a
  stable `floor_char_boundary` helper. Closes the v1.1 follow-up
  noted in the previous audit (`analysis_of_sample_program` was
  panicking on the `─` banner chars in `sample_program.rvn`).

Two stale-test sweeps:

- `Task` → `Todo` rename across `compiler/riven_core/tests/fixtures/{class_methods,mini_sample,sample_program}.rvn`
  (collided with the stdlib's async `class Task`).
- E1100 / E1101 / E1102 / E1110 / E1112 / E1115 / E1116 / E1117 /
  E1118 added to the `EXPLAINS` table in `riven_cli/src/explain.rs`
  (the `.md` files already existed; the table was just out of sync
  with the central registry).

## Cleanup pass (2026-05-21)

The following prompts were **deleted from this directory** because
their work is 100% shipped. The original prompt content is preserved
in git history (`git log -- docs/prompts/v1/<name>.md` to recover).

| Deleted | Title | Evidence in tree |
|---|---|---|
| 01 | Phase 1 remainder | Drop infra, structural mixins, error code registry — all in `compiler/riven_core/src/{mir/lower/drops.rs,implicit_includes/,diagnostics/codes.rs}` |
| 02 | stdlib String | `library/std/string/{src,runtime}/` |
| 03 | stdlib Array | `library/std/array/{src,runtime}/` |
| 04 | stdlib Map/Set | `library/std/{map,hash,set}/` |
| 05 | stdlib Iterator | `library/std/iter/`; the last unchecked bullet referenced an obsolete pre-restructure path |
| 06 | stdlib io/fmt | `library/std/{io,fmt,fs,env,process,path,bufio}/`; ExitStatus + Metadata accessors moved C→Riven via `layout c` this session |
| 06.5 | sync I/O completeness | File, FS, TcpListener, TcpStream, BufReader, BufWriter, OpenOptions, Metadata + typed FFI return surface |
| 06.9 | dyn-Fn closure dispatch | `Ty::AnyMixin`, closure inline-expansion MIR pass, pin tests, e2e fixture 600 |
| 06.93 | module-qualified class resolution | `BufReader.File` / `BufReader.Tcp` etc. resolve via `type_registry` qualified-name keys |
| 06.95 | stdlib packagization | Per-package `library/std/<pkg>/{src,runtime,Riven.toml}` for 25 packages; `auto_populate_std_submodules_from_packages` derives `std.<pkg>` items from BOOTSTRAP_FILES |
| 07 | const generics | Tier-2 surface in `parser/ast.rs`; pin tests in `const_generics.rs`; e2e fixtures `072_*` and `073_*` |
| 10 | LSP | All 17 spec §5 capabilities + UseIndex reverse-index. ~230 capability tests + 12 LSP integration tests. Wave 1 (8 agents) + Wave 2 (3 agents) |
| 13 | benchmarking | `library/std/bench/` pure-Riven `Bencher` + `rivenc bench` subcommand + 5 criterion compile-pipeline benches. JSON/baseline/MAD deferred to v1.5 |
| 14 | concurrency | Thread, Mutex, MutexGuard, SharedSync, Atomic*, Sender/Receiver, JoinHandle. Riven-level Mutex bench at `tests/benches/sync_mutex.rvn` runs at **15 ns/iter** — 70× under the 1 µs/op DoD budget (100k ops in 1.5 ms vs the 100 ms target) |
| 15 | async | Future/Poll/Context/Waker + async def/.await + block_on + reactor + AsyncFile/TcpStream/TcpListener + Task.spawn/join/yield_now + AsyncStdin (`library/std/async_io/`). AsyncStdout/Stderr deferred to v1.1 |
| 06.75 | repo restructure | Layout done + `typeck/infer.rs` 2279→5-file split (mod 399 / expr 948 / collect 550 / helpers 345 / ops 171) + `codegen/runtime.rs` 739→runtime/ tree (mod 55 / 4 symbol files + 2 test files, each ≤ 273) — all under the spec's per-file caps |
| 06.8 | stdlib self-hosting | `resolve/stdlib/mod.rs` 580→91 LOC + 5 per-namespace files (primitives, modules, type_constructors, option, result, each ≤ 112) |

## Remaining prompts (in tree)

| # | Title | Status | One-line summary |
|---|---|---|---|
| 08 | HRTBs / `some Mixin` | 🟡 Partial | `some Mixin` shipped; `for<'a>` HRTBs deferred to v1.5/v2 per original prompt |
| 09 | GATs / `any Mixin` | 🟡 Partial | `any Mixin` + mixin vtables Phase A/B/C shipped; GATs themselves deferred to v1.5/v2 |
| 11 | incremental | 🟡 Partial | rivenc cache + `--force` + `clean` subcommand work; formal query-layer design TODO |
| 12 | diagnostics polish | ⬜ Not started | E-codes ladder complete; hint engine / did-you-mean polish TODO |
| 16 | no_std / wasm | ⬜ Not started | No wasm32 codegen path |
| 17 | cross-compile / ABI | ⬜ Not started | Single-platform builds only |
| 18 | debugger / DWARF | ⬜ Not started | No debuginfo emission |
| 19 | test framework | ⬜ Not started | Tests are Rust-side over `.rvn` fixtures; no Riven-native runner |
| 20 | rivendoc | ⬜ Not started | Only spec in `docs/requirements/tier3_04_doc_generator.md` |
| 21 | MIR optimizer | ⬜ Not started | MIR has lowering, no opt pass tree |
| 22 | pkg manager / workspaces | 🟡 Partial | Per-package Riven.toml shipped; workspace/resolver/lockfile/registry TODO |
| 23 | language reference | ⬜ Not started | Spec-driven `docs/specs/` exist; unified manual not assembled |
| 24 | edition mechanism | ⬜ Not started | No `edition = "2026"` consumer in toolchain |
| 25 | release checklist | 📋 Tracking | Gates on all above |

## Wave-front

- ✅ **Done**: phase 1, phase 2 stdlib, phase 2.5 repo + self-hosting cleanup (06.75 / 06.8), phase 3 (07 const generics + 10 LSP + 13 benchmarking), phase 4 (14 concurrency + 15 async)
- 🟡 **Partial**: language polish (08 HRTBs / 09 GATs / 11 incremental), package manifests (22)
- ⬜ **Genuinely TODO**: 12 diagnostics polish, 16 no_std/wasm, 17 cross-compile, 18 debugger/DWARF, 19 test framework, 20 rivendoc, 21 MIR optimizer, 23 language reference, 24 editions, 25 release checklist

**Closest-to-shipping next chunks:**
1. **19 test framework** — biggest user-facing v1 gap; unlocks user-side `def test_*` runner. Same pattern as prompt 13's `rivenc bench` (pure Riven harness + tiny Rust CLI wrapper)
2. **The four real soundness bugs from `docs/quality_review.md` §1.3** — tuple-float-roundtrip (`parser/expr/calls.rs:72-96`), `def __drop` collector (`mir/lower/collect.rs:223+`), `derive Clone` parent-field walk (`mir/lower/derive.rs:757-793`), `unify` Ref auto-deref (`typeck/unify.rs:294-301`). Each is localised and pin-testable.
3. **12 diagnostics polish** — error codes are exhaustive but hints/suggestions need engine work
4. **22 pkg manager workspace/resolver** — turns stdlib's per-package model into something user packages can opt into

**v1.1 follow-ups still open:**
- AsyncStdout/Stderr — wait for demand
- ~~UTF-8 char-boundary bug in `line_index.rs:36`~~ — **fixed 2026-05-22** in commit `79be087` (stable `floor_char_boundary` helper)
- Quality review Wave 2 (security): four CVE-class holes in `src/riven_cli/src/resolve_deps.rs` (argument injection on `git clone`, path-dep traversal, `dlsym(RTLD_DEFAULT)` open allowlist). Pre-1.0 is the right time.
