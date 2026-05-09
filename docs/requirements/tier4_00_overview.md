# Tier 4 Roadmap — Ecosystem & Release Engineering

Companion index for the Tier-4 requirements documents. Read this first.

Tier 4 is about the *outside* of the compiler — the surfaces that make Riven a real ecosystem someone can depend on: a package manager that scales past `{ path = ".." }` and `{ git = "..." }`, a cross-compilation story, a WASM target, a no-std build mode, a stable C ABI for users embedding Riven in other stacks, and all the release-engineering hygiene (CI, examples, `LICENSE`, `CHANGELOG.md`, …) that turns a repo into a project.

## The docs

| # | Feature | Doc |
|---|---------|-----|
| 01 | Package manager extensions (workspaces, features, publish, registry, yank, lockfiles, semver) | [tier4_01_package_manager.md](tier4_01_package_manager.md) |
| 02 | Cross-compilation (`--target`, sysroot, triple parsing) | [tier4_02_cross_compilation.md](tier4_02_cross_compilation.md) |
| 03 | WASM target (`wasm32-unknown-unknown`, WASI, WIT) | [tier4_03_wasm_target.md](tier4_03_wasm_target.md) |
| 04 | no_std / embedded mode (panic strategy, core/std split, no-alloc) | [tier4_04_no_std_embedded.md](tier4_04_no_std_embedded.md) |
| 05 | Stable ABI / cbindgen (C-header emission) | [tier4_05_stable_abi_cbindgen.md](tier4_05_stable_abi_cbindgen.md) |
| 06 | CI (GitHub Actions: test, lint, coverage, fuzz, MSRV) | [tier4_06_ci.md](tier4_06_ci.md) |
| 07 | `examples/` directory | [tier4_07_examples.md](tier4_07_examples.md) |
| 08 | Repo hygiene (`LICENSE`, `CONTRIBUTING`, `CHANGELOG`, `SECURITY`, `CODE_OF_CONDUCT`) | [tier4_08_repo_hygiene.md](tier4_08_repo_hygiene.md) |

## Current-state summary (the one-pager)

- **Package manager**: `Riven.toml` exists (`crates/riven-cli/src/manifest.rs:7-21`) with `[package]`, `[dependencies]`, `[dev-dependencies]`, `[build]`, `[[bin]]`, `[profile.*]`. Registry dependencies are parsed but rejected at resolve time (`resolve_deps.rs:100-108`). **No `[workspace]`, no `[features]`, no `riven publish`, no registry, no `riven yank`.** `Riven.lock` exists with git-revision + checksum (`lock.rs:17-27`).
- **Cross-compilation**: **zero support.** Cranelift backend hard-codes the host machine via `cranelift_native::builder()` (`crates/riven-core/src/codegen/cranelift.rs:50`). LLVM backend calls `TargetMachine::get_default_triple()` (`crates/riven-core/src/codegen/llvm/mod.rs:42`). No `--target` CLI flag anywhere (`crates/riven-cli/src/cli.rs:24-114`, `crates/rivenc/src/main.rs:40-67`). The linker is unconditionally `cc` (`crates/riven-core/src/codegen/object.rs:20,64`).
- **WASM target**: absent. No `wasm32` anywhere except test-name collisions. `emit_executable` always invokes `cc` with `-lc -lm` (`object.rs:64-70`) — fundamentally incompatible with `wasm32-unknown-unknown`.
- **no_std / embedded**: absent. The runtime `runtime.c` (`crates/riven-core/runtime/runtime.c`, 426 lines) unconditionally pulls in `stdio.h`, `stdlib.h`, `string.h`, `stdint.h` and links `-lc -lm`. `riven_panic` calls `fprintf(stderr, …)` + `abort()`. No `#[panic_handler]` equivalent, no `panic = "abort"` knob, no no-alloc path.
- **Stable ABI / cbindgen**: absent for the *Riven surface*. Riven-side `extern "C"` + `lib` blocks exist (`parser/mod.rs:1570-1723`) for **importing** C; there is no generator that emits a `.h` header from Riven's `pub` items. `@[repr(C)]` is parsed but conflated with `@[derive]` (see tier-1 B2).
- **CI**: only a release workflow (`/.github/workflows/release.yml`, 144 lines) that fires on `v*` tags. **No `ci.yml`.** No lint job, no coverage, no fuzzing, no MSRV enforcement. The workspace `Cargo.toml` is 4 lines with no `rust-version` field (`Cargo.toml:1-4`). Every crate is `edition = "2021"`.
- **`examples/`**: **the directory does not exist.** `crates/riven-core/tests/fixtures/` has 13 small `.rvn` test fixtures but nothing larger than ~50 lines, and none are runnable as projects with `riven run`.
- **Repo hygiene**: `README.md` exists. **`LICENSE` is missing** (the release workflow does `cp LICENSE* … 2>/dev/null || true` — so it silently ships with no license). `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `CHANGELOG.md`, `SECURITY.md` all missing. README.md line 294-296: `## License\n\nTBD`.

## Cross-doc dependency graph

```
               ┌──────────────────────────────────────┐
               │ 06: CI (ship FIRST — unblocks        │
               │     everything else, protects master)│
               └─────────────────┬────────────────────┘
                                 │
         ┌───────────────────────┼──────────────────────────────────┐
         │                       │                                  │
         ▼                       ▼                                  ▼
┌────────────────────┐  ┌────────────────────┐          ┌──────────────────────┐
│ 08: Repo hygiene   │  │ 01: Package mgr    │          │ 02: Cross-compilation│
│ (LICENSE, CoC,     │  │  1a: workspaces    │          │  (--target, sysroot, │
│  CHANGELOG,        │  │  1b: features      │          │   triple parsing)    │
│  CONTRIBUTING,     │  │  1c: registry +    │          └──────────┬───────────┘
│  SECURITY)         │  │      publish       │                     │
└────────────────────┘  │  1d: yank, audit   │                     ▼
                        └─────────┬──────────┘          ┌──────────────────────┐
                                  │                     │ 04: no_std / embedded│
                                  ▼                     │  (panic strategy,    │
                        ┌────────────────────┐          │   core/std split,    │
                        │ 07: examples/      │          │   no-alloc)          │
                        │  (CLI, threaded    │          └──────────┬───────────┘
                        │   server, WASM     │                     │
                        │   toy, game)       │                     ▼
                        └────────────────────┘          ┌──────────────────────┐
                                                        │ 03: WASM target      │
                                                        │  (wasm32-unknown-    │
                                                        │   unknown, WASI opt) │
                                                        └──────────────────────┘

 (Independent track, orthogonal:)
               ┌──────────────────────────────────────┐
               │ 05: Stable ABI / cbindgen            │
               │  (C-header emission from pub items)  │
               └──────────────────────────────────────┘
```

Key dependencies:

- **CI blocks nothing but protects everything.** It must ship before any of the other tier-4 work so that the remaining deltas land against a green, enforced baseline. Called out as a separate recommendation below.
- **WASM depends on no_std.** `wasm32-unknown-unknown` has no libc — that means no `cc -lc -lm`, no `fprintf`, no `malloc` from the host. Either we ship `dlmalloc`/`wee_alloc` inside the runtime, or we go no-alloc. Either way the work of untangling `runtime.c`'s assumptions is the same work the no-std doc describes. **Do doc 04 first, then doc 03.**
- **WASI is weaker than no_std.** `wasm32-wasi` has `libc` via `wasi-libc`, so the existing `runtime.c` *mostly* compiles unchanged — WASI is a good interim target that exercises the cross-compilation path without forcing no-std.
- **Cross-compilation blocks WASM.** You cannot target wasm32 without first teaching the toolchain how to accept a `--target` flag, pick the right linker, and drop `-lc -lm` when the target doesn't have them. Doc 02 comes before doc 03.
- **Registry depends on publish and yank.** Registry (doc 01) is the largest single subsystem — hosting, auth, upload protocol, index format. It ships last within the package-manager track.
- **Workspaces unblock the stdlib.** If tier-1 ships stdlib as a sibling crate (see tier1_01_stdlib.md §7.8), it wants to live in a workspace next to the user's project. Workspaces should ship *before* the stdlib crate split.
- **`examples/` depends on nothing but benefits from everything.** A threaded-web-server example depends on tier-1 concurrency; a WASM example depends on doc 03. But a plain CLI tool and a tiny game can ship the day the compiler is stable — doc 07 should start growing in parallel.
- **Stable ABI / cbindgen is orthogonal.** It doesn't block anything tier-4 and isn't blocked by anything tier-4. It ships whenever it ships.

## Recommended implementation order

**Phase 4-0 — ship CI immediately (1 week).** Doc 06.

1. `ci.yml` on push-to-master and every PR: `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test --workspace`, `cargo test --workspace --features llvm` on the LLVM matrix entry.
2. MSRV: pin `rust-version = "1.78"` in the root `Cargo.toml` (current compiler builds against it; revisit yearly).
3. Coverage via `cargo-llvm-cov` uploading to Codecov — informational, not gating.
4. Fuzz skeleton under `fuzz/` using `cargo-fuzz`: one target each for the lexer and parser. Run time-bounded (60s) on every PR; longer runs on a nightly schedule.

**Phase 4-1 — repo hygiene (0.5 week).** Doc 08.

1. `LICENSE-MIT` + `LICENSE-APACHE` (dual-license, matching Rust/most of the Rust ecosystem).
2. `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` (Contributor Covenant 2.1), `SECURITY.md` (private disclosure via email), `CHANGELOG.md` (Keep-a-Changelog format starting at v0.1).
3. Update `README.md:294-296` — replace `TBD` with the real license line.
4. Add GitHub issue + PR templates under `.github/`.

**Phase 4-2 — package manager, iteration 1: workspaces + features (2-3 weeks).** Doc 01 phase 1a-1b.

1. `[workspace]` parsing, path-dep discovery rooted at the workspace root, shared `Riven.lock`.
2. `[features]` table: optional-dep activation, feature unification, `--features`/`--no-default-features` flags.
3. `riven check --workspace` / `riven build -p <crate>`.

**Phase 4-3 — cross-compilation (2-3 weeks).** Doc 02.

1. Add `--target <triple>` to `riven build/run/check` and `rivenc`. Parse via `target-lexicon` (already a dependency).
2. Thread the triple into Cranelift via `cranelift_codegen::isa::lookup_by_name` and into LLVM via `TargetMachine::new` — replacing the host-only paths.
3. Pick the right linker: `cc` for unix targets, `lld` for wasm32, configurable via `[target.<triple>] linker = "…"` in `Riven.toml`.
4. Sysroot layout: `~/.riven/lib/runtime/<triple>/runtime.o` — precompiled `runtime.c` per target, fetched on demand by a new `riven target add <triple>` subcommand (Rustup analogue).

**Phase 4-4 — no_std / embedded (2-3 weeks).** Doc 04.

1. `[package] no-std = true` key → skip stdlib prelude, ship only `core`.
2. `panic = "abort" | "unwind"` in `[profile.*]`. Default abort. No unwinding in v1.
3. Split `runtime.c` into `runtime_core.c` (no libc, no malloc) and `runtime_std.c` (everything else). `core` only needs the former.
4. Expose a `@[panic_handler]` attribute on a user-supplied function when `no-std = true`.

**Phase 4-5 — WASM (2 weeks, after 4-3 and 4-4).** Doc 03.

1. `wasm32-unknown-unknown` as a target. `runtime_core.c` with a tiny bump-allocator shipped inside the runtime.
2. Entry point: export `_start` / user-named exports via `@[wasm_export]`; host functions imported via `@[wasm_import("module", "name")]`.
3. `wasm32-wasi` as a second target (easier: existing `runtime.c` Just Works with `wasi-libc`).
4. WIT / Component Model explicitly future-work — call it out in the doc and don't ship.

**Phase 4-6 — examples/ (0.5 week per example).** Doc 07.

Run in parallel with 4-1 onward. Minimum set:
- `examples/01-cli-utility/` — a word-count clone, synchronous.
- `examples/02-tcp-echo-server/` — single-threaded TCP echo, exercises `std::net`.
- `examples/03-threaded-server/` — multi-threaded HTTP echo (depends on tier-1 concurrency).
- `examples/04-wasm-hello/` — tiny wasm32 demo with an HTML harness (depends on doc 03).
- `examples/05-snake-game/` — terminal-based Snake with a single git dependency on a hypothetical `termio` piece (demonstrates the package manager).

**Phase 4-7 — package manager, iteration 2: registry + publish (4-6 weeks).** Doc 01 phase 1c-1d.

1. `riven publish` uploads a tarball + manifest + checksum to the registry.
2. Index format: git-backed sparse index (crates.io `sparse+` protocol; simplest thing that works at scale).
3. Registry server: an initial self-hosted instance running a small Rust service backed by S3-compatible object storage + Postgres. Defer open-source-ing the server until the protocol stabilizes.
4. `riven yank <piece> <version>` marks a version unresolvable by new `riven update`s but installable if pinned in a lock file (Cargo semantics).
5. `riven audit` checksums every downloaded tarball against the index.

**Phase 4-8 — stable ABI / cbindgen (2-3 weeks, independent).** Doc 05.

1. `rivenc --emit=c-header` walks the HIR, finds `pub extern "C"` items, emits a `.h` file.
2. Stability rules: only `@[repr(C)]` structs, only `extern "C"` functions, no generics, no `Option`/`Result` (emit as `*T` / `int` with a sibling error-struct).
3. Version-bake the header: `riven_abi_version()` compiles in the compiler version at emission time, so consumers can `riven_abi_version() != EXPECTED` at runtime.

## Total estimate

~16-21 weeks of focused work for one engineer.

- **4-0** (CI): 1 week. Ship immediately.
- **4-1** (repo hygiene): 0.5 week. Can be same PR as CI.
- **4-2** (workspaces + features): 2-3 weeks.
- **4-3** (cross-compilation): 2-3 weeks.
- **4-4** (no_std): 2-3 weeks.
- **4-5** (WASM): 2 weeks, unblocks after 4-3 and 4-4.
- **4-6** (examples): parallel, 0.5 week per example × 5.
- **4-7** (registry + publish): 4-6 weeks.
- **4-8** (cbindgen): 2-3 weeks, independent.

Parallelize 4-3 / 4-4 / 4-8 across engineers once 4-2 ships.

## Critical questions for the project lead

These surfaced across the docs and need a single ruling before implementation begins.

1. **Registry choice.** Three options:
   - **a) Host our own.** Full control. Real infrastructure cost (hosting, monitoring, abuse response, CDN). Time-to-launch: months.
   - **b) Piggyback on crates.io.** Culturally wrong — different language, different runtime, different ecosystem. crates.io will reject a new language's packages and should.
   - **c) Go-style: git-URL based, no central registry.** `riven add foo --git https://github.com/…` is all there is. Simplest path. Already works in the current code (doc 01 §2). Discoverability is worse, but v1 doesn't need discoverability.
   - **Recommend**: ship **(c) only** for v1. Leave the manifest registry fields wired so that a future (a) can land without a manifest break. This defers the largest chunk of the tier-4 budget and is the same call Go made at 1.0.
2. **License.** **Recommend: dual-license MIT OR Apache-2.0**, matching Rust, Cargo, tokio, and 80% of the Rust-adjacent ecosystem. This is the most-compatible license combination for downstream users (MIT for permissive linking, Apache-2.0 for explicit patent grant). See doc 08.
3. **MSRV policy.** **Recommend: pin `rust-version = "1.78"` at the workspace root.** 1.78 released May 2024, is currently stable everywhere, and matches what `cranelift-codegen 0.130` already requires. Bump on a 6-month cadence. Enforce in CI with `dtolnay/rust-toolchain@1.78`.
4. **Fuzzing.** **Recommend: `cargo-fuzz` with `libfuzzer-sys`.** Lexer + parser are prime targets (they accept arbitrary bytes). Borrow-check and type-check are secondary (they need a valid AST, so harness complexity is higher). Run 60s/target on every PR, 1h/target on nightly cron. See doc 06 §5.
5. **CI provider.** **Recommend: GitHub Actions** — the release workflow already uses it, runners are free for public repos, matrix support is fine. Don't split across providers.
6. **Examples: in-tree or separate repo?** **Recommend: in-tree `examples/`** at the repo root. Rust is in-tree, Go is in-tree, Python is in-tree. The "separate repo" pattern (Node.js) harms discoverability. See doc 07.
7. **WASM: which toolchain?** `wasm-ld` (bundled with LLVM) or `lld` (bundled with Rust)? **Recommend: `lld`**, which is bundled with `rustup`'s default Rust install and already ships with every dev machine that builds Riven from source. Fall back to `wasm-ld` if `lld` is absent.
8. **no_std: ship a `core` crate or just a feature flag?** **Recommend: feature flag in v1, crate split in v2.** Tier-1 stdlib doc (§6.3) already recommends this. The `core` *subdirectory* under `std::core::*` is a mechanical `mv` once stable.
9. **Panic strategy default.** `abort` or `unwind`? **Recommend: `abort`** in v1. Unwinding requires landing-pad emission (MIR lowering change), per-function DWARF CFI, and an unwinder (libunwind / LLVM builtins). All deferred to v2.

## Pre-existing issues that block tier-4 work

- **README.md:294-296 claims `## License: TBD`.** This must be resolved before tier-4 can promise dual-licensing — it's the entry point new contributors see. Unblock with doc 08.
- **The release workflow `cp LICENSE* "${STAGE}/" 2>/dev/null || true` silently succeeds when `LICENSE` doesn't exist** (`.github/workflows/release.yml:96`). Ship doc 08 first so releases include the license.
- **`Cargo.toml` has no `rust-version` field.** A contributor can land code that only builds on nightly and nothing will catch it. Unblock with doc 06.
- **Registry dependencies are parsed in `manifest.rs:51-57` but rejected at resolve time with `"registry dependencies are not yet supported"` (`resolve_deps.rs:104-108, 116-120`).** Either delete the registry branch (it's aspirational and misleading) or implement it. Doc 01 implements it.
- **`@[repr(C)]` and `@[derive]` are conflated into a single `derive_traits: Vec<String>` field.** Tier-1 B2 must be fixed before doc 05 (cbindgen) can run: cbindgen needs to see "is this a repr(C) struct?" as a first-class question, not a string match.
