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

## Remaining prompts (in tree)

| # | Title | Status | One-line summary |
|---|---|---|---|
| 06.75 | repo restructure | 🟡 Mostly shipped | Layout done; `typeck/infer.rs` (2279 LOC) + `codegen/runtime.rs` (739 LOC) still over budget |
| 06.8 | stdlib self-hosting | 🟡 Mostly shipped | All stdlib in Riven; `resolve/stdlib/mod.rs` is 580 LOC (target 400); pin tests scattered rather than in `extern_c_binding.rs` |
| 08 | HRTBs / `some Mixin` | 🟡 Partial | `some Mixin` shipped; `for<'a>` HRTBs deferred to v1.5/v2 per original prompt |
| 09 | GATs / `any Mixin` | 🟡 Partial | `any Mixin` + mixin vtables Phase A/B/C shipped; GATs themselves deferred to v1.5/v2 |
| 10 | LSP | ✅ Shipped | All 17 spec §5 capabilities: diagnostics, hover, goto_def, goto_type_definition, semantic_tokens_full, completion, signature_help, document_symbol, workspace_symbol, inlay_hint, folding_range, document_formatting + range_formatting, code_action, references, document_highlight, prepare_rename + rename. Plus UseIndex reverse-index. ~230 capability test cases across 14 modules + 12 LSP integration tests. One pre-existing UTF-8 char-boundary failure in `analysis_of_sample_program` unrelated |
| 11 | incremental | 🟡 Partial | rivenc cache + `--force` + `clean` subcommand work; formal query-layer design TODO |
| 14 | concurrency | ✅ All primitives | Thread, Mutex, MutexGuard, SharedSync, Atomic*, Sender/Receiver, JoinHandle shipped; bench pending on prompt 13 |
| 15 | async | ✅ Sub-phases 1–5 | Future/Poll/Context/Waker, async def + .await, block_on, reactor, AsyncFile/AsyncTcpStream/AsyncTcpListener, Task.spawn/join/yield_now; AsyncStdin variants not shipped |
| 12 | diagnostics polish | ⬜ Not started | E-codes ladder complete; hint engine / did-you-mean polish TODO |
| 13 | benchmarking | ✅ Shipped | `library/std/bench/` pure-Riven Bencher + `rivenc bench` subcommand + 5 criterion compile-pipeline benches in `src/rivenc/benches/`. JSON/baseline/MAD deferred to v1.5 |
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

- ✅ **Done**: phase 1, phase 2 stdlib, phase 4 (concurrency + async)
- 🟡 **Partial**: phase 3 language polish (08/09/10/11), repo cleanup tails (06.75/06.8), package manifests (22)
- ⬜ **Genuinely TODO**: phase 3 diagnostics+bench (12/13), phase 4 platform (16/17/18), phase 5 tooling (19/20/21), language ref + editions (23/24), release checklist (25)

**Closest-to-shipping next chunks:**
1. Finish 06.75 (split `typeck/infer.rs` and `codegen/runtime.rs`) and 06.8 (slim `resolve/stdlib/mod.rs`) — pure cleanup, mechanical
2. **19 test framework** — biggest user-facing v1 gap; unlocks user-side `def test_*` runner
3. **12 diagnostics polish** — error codes are exhaustive but hints/suggestions need engine work
4. **22 pkg manager workspace/resolver** — turns stdlib's per-package model into something user packages can opt into
