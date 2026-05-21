# v1 prompts — actual progress status

Last audited: 2026-05-21. Status reflects in-tree code state, not
just doc checkboxes (which were not maintained as work landed).

Each remaining `*.md` file in this directory carries its own status
header summarising current state + evidence pointers.

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
3. **12 diagnostics polish** — error codes are exhaustive but hints/suggestions need engine work
4. **22 pkg manager workspace/resolver** — turns stdlib's per-package model into something user packages can opt into
5. **v1.1 follow-ups identified this session**: (a) AsyncStdout/Stderr if a demand actually shows up; (b) UTF-8 char-boundary bug in `line_index.rs:36` that crashes `analysis_of_sample_program`
