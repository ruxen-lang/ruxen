# Riven — Full-Stack Roadmap

Companion index for all tier-1 through tier-5 requirements docs. Read this first.

## Doc index (38 files)

| Tier | Focus | Docs |
|------|-------|------|
| **1** | Blocks real-world use | [roadmap](tier1_00_roadmap.md) · [stdlib](tier1_01_stdlib.md) · [concurrency](tier1_02_concurrency.md) · [async](tier1_03_async.md) · [drop/copy/clone](tier1_04_drop_copy_clone.md) · [derive+macros](tier1_05_derive_macros.md) |
| **2** | Type-system features | [overview](tier2_00_overview.md) · [assoc types](tier2_01_assoc_types.md) · [const generics](tier2_02_const_generics.md) · [HRTBs](tier2_03_hrtbs.md) · [impl trait + specialization](tier2_04_impl_trait_and_specialization.md) · [GATs](tier2_05_gats.md) · [trait objects](tier2_06_trait_objects.md) · [variance](tier2_07_variance.md) |
| **3** | Tooling & DX | [overview](tier3_00_overview.md) · [LSP](tier3_01_lsp.md) · [debugger](tier3_02_debugger.md) · [test framework](tier3_03_test_framework.md) · [docgen](tier3_04_doc_generator.md) · [bench](tier3_05_benchmarking.md) · [incremental](tier3_06_incremental_compile.md) · [MIR opts](tier3_07_mir_optimizations.md) · [proptest](tier3_08_property_testing.md) |
| **4** | Ecosystem & release | [overview](tier4_00_overview.md) · [package manager](tier4_01_package_manager.md) · [cross-compile](tier4_02_cross_compilation.md) · [WASM](tier4_03_wasm_target.md) · [no_std](tier4_04_no_std_embedded.md) · [stable ABI](tier4_05_stable_abi_cbindgen.md) · [CI](tier4_06_ci.md) · [examples](tier4_07_examples.md) · [repo hygiene](tier4_08_repo_hygiene.md) |
| **5** | Spec & stability | [overview](tier5_00_overview.md) · [reference](tier5_01_language_reference.md) · [editions](tier5_02_edition_stability.md) · [deprecation](tier5_03_deprecation_stability_attrs.md) · [error codes](tier5_04_error_code_registry.md) · [suggestions](tier5_05_diagnostic_suggestions.md) |

## Pre-existing bugs surfaced across the research (fix before anything else)

The research agents independently uncovered broken or aspirational-but-nonfunctional code in the current tree. These are blocking every tier downstream.

| # | Severity | Description | Location | Doc |
|---|----------|-------------|----------|-----|
| **P0.1** | Legal | `README.md:294-296` says "License: TBD"; release workflow already ships tarballs. `release.yml:96` silently no-ops on missing `LICENSE*`. | root | tier4_08 |
| **P0.2** | Correctness | `MirInst::Drop` is emitted but both codegen backends silently discard it. Every program leaks heap memory until exit. | `codegen/cranelift.rs:692-698`, `codegen/llvm/emit.rs:790-792` | tier1_04 |
| **P0.3** | Correctness | `@[derive(...)]` attribute errors out — only `@[repr]`/`@[link]` are dispatched. Separate body-level `derive Trait` syntax is parsed but no pass consumes it. `Ty::is_copy()` ignores `derive_traits`. | `parser/mod.rs:473-511`, `hir/types.rs:189-221` | tier1_05 |
| **P0.4** | Design | `@[repr(C)]` is stuffed into the same `Vec<String>` field as `derive Trait` names. | `parser/mod.rs:499-503` | tier1_05 |
| **P0.5** | Correctness | `?T..._method` codegen fallback maps unresolved generic method calls to `riven_noop_passthrough`. Some currently-passing tests are no-ops. | `codegen/runtime.rs` | tier1_01 |
| **P0.6** | Design | `Hash[K,V]` collection name collides with the conventional `Hash` trait. Both stdlib and derive docs recommend renaming to `HashMap`. | `resolve/mod.rs:200` | tier1_01 / tier1_05 |
| **P0.7** | Correctness | String literals flow as `Ty::String`; lowering `String::drop` to `free()` would double-free. | `mir/lower.rs` + runtime | tier1_04 |
| **P0.8** | UX | Manifest parses registry dependencies (`manifest.rs:51-57`) but `resolve_deps.rs:100-108` hard-rejects them with "not yet supported". | `crates/riven-cli/src/` | tier4_01 |
| **P0.9** | Policy | `Cargo.toml` workspace root is 4 lines with no `rust-version`. Contributor can land nightly-only Rust. | root | tier4_06 |
| **P0.10** | Docs | `CLAUDE.md` claims proptest coverage. The only two proptest uses are `prop_assert!(expected_len <= 100)` and `prop_assert_eq!(n, n)` tautologies. | `tests/runtime_safety.rs:68-86` | tier3_08 |
| **P0.11** | Dead code | `package.edition` field exists and is tested, but `grep edition` across `riven-core` returns zero hits — entirely inert. | `crates/riven-cli/src/manifest.rs:29` | tier5_02 |
| **P0.12** | Dead code | `async`/`await`/`spawn`/`actor`/`send`/`receive` reserved in lexer but never consumed by the parser. | `lexer/token.rs:83-85,:127-130` | tier1_02 / tier1_03 |
| **P0.13** | UX | Doc comments `##` are lexed as `TokenKind::DocComment` but discarded at four parser sites. | `parser/mod.rs:455-458,:884-887,:1112-1115,:1187-1190` | tier3_04 |
| **P0.14** | Correctness | `DWARF` emission is a 3-line stub. `--backend=llvm` debug builds have no line info. | `codegen/llvm/debug.rs:1-3` | tier3_02 |
| **P0.15** | Soundness | Variance rules for built-in type constructors (`&mut T` invariant in T, `Vec[T]` invariant, `Option[T]` covariant) are encoded as *comments only*. No fixture proves them. | `typeck/coerce.rs:108-109` | tier2_07 |

## Cross-tier dependency graph

```
                            ┌──────────────────────────────────┐
                            │ Phase 0 — pre-flight fixes       │
                            │ P0.1 LICENSE, P0.2 Drop,         │
                            │ P0.3-P0.4 derive untangle,       │
                            │ P0.5 ?T..., P0.6 Hash rename,    │
                            │ P0.9 MSRV, P0.13 doc comments    │
                            └──────────┬───────────────────────┘
                                       │
           ┌───────────────────────────┼────────────────────────────┐
           ▼                           ▼                            ▼
  ┌──────────────┐          ┌──────────────────┐          ┌─────────────────┐
  │ T4 CI        │          │ T1 Drop/Copy/    │          │ T5 error-code   │
  │ (ships day 1)│          │    Clone + Derive│          │    registry     │
  └──────┬───────┘          └────────┬─────────┘          └──────┬──────────┘
         │                           │                           │
         ▼                           ▼                           ▼
  ┌──────────────┐          ┌──────────────────┐          ┌─────────────────┐
  │ T4 cbindgen  │          │ T1 stdlib 1a     │          │ T5 suggestions  │
  │   (ABI docs) │          │ (io/fmt/strings/ │          │   framework     │
  └──────────────┘          │  collections)    │          └──────┬──────────┘
                            └────────┬─────────┘                 │
                                     │                           │
                                     ▼                           │
                          ┌────────────────────┐                 │
                          │ T1 stdlib 1b/1c    │                 │
                          │ + T3 test framework│◄────────────────┘
                          └────────┬───────────┘
                                   │
                                   ▼
                          ┌────────────────────┐
                          │ T1 concurrency     │
                          │ + T2 variance/dyn  │
                          └────────┬───────────┘
                                   │
                       ┌───────────┴─────────────┐
                       ▼                         ▼
              ┌────────────────┐        ┌─────────────────┐
              │ T1 async       │        │ T3 LSP          │
              │                │        │ + T3 incremental│
              └────────────────┘        └─────────┬───────┘
                                                  │
                                                  ▼
                                         ┌─────────────────┐
                                         │ T2 rest,        │
                                         │ T3 debugger,    │
                                         │ T4 wasm/cross,  │
                                         │ T5 reference    │
                                         └─────────────────┘
```

Critical orderings extracted from the individual docs:

- **Drop must work before stdlib ships.** Every heap-backed stdlib type (`String`, `Vec`, `HashMap`, `File`, `TcpStream`) leaks until exit without P0.2 fixed.
- **Derive must work before stdlib ships.** Users hand-writing `impl Debug` for every struct is a non-starter.
- **Associated types before `Iterator`.** Tier-1 stdlib's `Iterator` trait needs tier-2.01. These land together in practice.
- **Incremental compile before LSP completion.** LSP can ship with debounced full-file re-analysis first (tier-3.01 phase 1), then migrate to the tier-3.06 query layer.
- **no_std before WASM.** `wasm32-unknown-unknown` is a special case of no_std — tier-4.04 before tier-4.03.
- **CI before everything else tier-4.** `ci.yml` is a 0.5-week task that catches regressions for every subsequent subsystem.
- **Error-code registry before per-compiler-phase error work.** Tier-5.04 is the namespace owner; tier-1 docs reserve `E1011-E1016`, tier-1.05 reserves `E0601-E0609`, tier-2.07 reserves `E0705/E0706`. Register them once, then wire diagnostics.
- **Debugger requires LLVM backend.** Cranelift DWARF is not production-grade. Accept tier-3.02 as `--backend=llvm` only for v1.

## Recommended implementation sequence

### Phase 0 — pre-flight (2-3 weeks, can start today)

Fixes P0.1, P0.3, P0.4, P0.6, P0.9, P0.10, P0.11, P0.13 — anything that's either legally/ethically urgent or blocking a single-doc dependency. Include:

1. **License** (P0.1): pick MIT OR Apache-2.0 (tier-4.08 recommends dual), add `LICENSE-MIT` + `LICENSE-APACHE`, fix README.
2. **MSRV** (P0.9): pin `rust-version = "1.78"` at workspace root, add MSRV check to CI.
3. **CI bootstrap** (tier-4.06 phase 1): ship `ci.yml` running `cargo test` + `cargo clippy` + MSRV check. 0.5 weeks.
4. **Attribute untangle** (P0.3, P0.4): wire `@[derive(...)]` dispatch, widen `ast::Attribute.args` to `Vec<AttrArg>`, separate `derive_traits` from `repr` storage, add `derive_traits` to class/enum.
5. **Hash rename** (P0.6): `Hash[K,V]` → `HashMap[K,V]`. Update tutorial + fixtures. First `EditionLint` canary once tier-5.02 ships.
6. **Doc-comment capture** (P0.13): stop discarding `##` at 4 parser sites; thread into HIR. Unblocks both tier-3.04 (rivendoc) and tier-3.01 hover enrichment.
7. **Edition removal or wiring** (P0.11): either delete the inert `edition` field from the manifest or gate a single behavior on it as a smoke test.
8. **Proptest cleanup** (P0.10): either add real properties per tier-3.08, or remove the claim from `CLAUDE.md`.

### Phase 1 — correctness foundations (4-6 weeks)

Fixes the remaining P0s that are actual semantic bugs, and lands the trait infrastructure stdlib depends on.

1. Tier-1.04 Drop infrastructure + MIR drop elaboration + real codegen (P0.2). Closes the heap-leak hole.
2. Tier-1.04 Copy/Clone traits + `Ty::is_copy_with(&SymbolTable)`.
3. Tier-1.05 builtin-derive infrastructure + `@[derive(Debug, Clone)]` + `@[derive(Copy, PartialEq)]`.
4. Tier-1.04 phase 4d: built-in drops for `String`/`Vec`/`Option`/`Result` (requires P0.7 string-literal-ownership fix first).
5. Tier-1.05 phase 5c: remaining derives (`Eq`/`Hash`/`Default`/`Ord`/`PartialOrd`).
6. Tier-5.04 error-code registry bootstrap (unifies diagnostic namespaces).
7. P0.5 `?T..._method` removal — accept test regressions, fix them.

### Phase 2 — stdlib (6-8 weeks)

Tier-1.01 phases 1a, 1b, 1c. Overlap tier-2.01 (associated types) to land `Iterator` with a real `Item` type. Write stdlib in Riven-plus-FFI as the docs specify. Keep key types int64-slot-erased until monomorphization lands (tier-2.02 const-generics dependency — noted).

Overlaps nicely with tier-3.03 (test framework) because now users have a stdlib to test *and* a test framework to drive it.

### Phase 3 — type-system deepening + DX (6-8 weeks, heavily parallel)

| Track | Content | Engineer-weeks |
|-------|---------|----------------|
| Types | Tier-2.01 assoc types, tier-2.05 GATs, tier-2.06 trait objects, tier-2.07 variance | ~12 |
| DX | Tier-3.01 LSP completion + diagnostics-on-edit, tier-3.06 incremental compile, tier-3.05 benchmarking | ~10 |
| Infra | Tier-5.03 deprecation/stability attributes, tier-5.05 suggestions framework | ~5 |

Total ~27 engineer-weeks, achievable in ~8 wall-clock weeks with 3-4 engineers.

### Phase 4 — concurrency, async, ecosystem (8-12 weeks)

1. Tier-1.02 concurrency (Send/Sync + Thread + Mutex + Arc + channels + atomics).
2. Tier-1.03 async (Future trait → async/await syntax → single-threaded executor → async I/O).
3. Tier-4.04 no_std/embedded split (blocks WASM).
4. Tier-4.03 WASM target.
5. Tier-4.02 general `--target` support.
6. Tier-4.05 stable-ABI / cbindgen (requires P0.3-P0.4 fixed).
7. Tier-2.02 const generics (unblocks real generic collections) + tier-2.03 HRTBs + tier-2.04 impl-trait.
8. Tier-3.02 debugger (LLVM-only, DWARF emission).

### Phase 5 — long-term health (ongoing from day 1)

Can run in parallel to every other phase because it's documentation-heavy.

- Tier-4.07 examples (at least 5 in-tree: CLI, TCP echo, threaded HTTP, WASM toy, small game).
- Tier-4.08 CONTRIBUTING / CoC / SECURITY / CHANGELOG (1-day task each).
- Tier-4.01 package-manager extensions (workspaces first, publish/registry last — git-URL registry recommended for v1).
- Tier-5.01 language reference (start now, grow with every feature).
- Tier-5.02 edition-stability mechanism (formalize once the Hash-rename edition-lint canary ships).
- Tier-3.08 proptest (if not done in phase 0).
- Tier-3.07 MIR optimizations (small passes can land anytime `simplify_cfg` always on, others at opt≥1).
- Tier-3.04 rivendoc (requires P0.13 doc-comment capture).

## Total estimate

| Phase | Weeks (serial) | Weeks (3-4 engineers) |
|-------|----------------|----------------------|
| 0 | 2-3 | 1-2 |
| 1 | 4-6 | 2-3 |
| 2 | 6-8 | 3-4 |
| 3 | 27 eng-weeks | 6-8 |
| 4 | 40+ eng-weeks | 10-14 |
| 5 | continuous | continuous |
| **Total** | ~18 months solo | ~6-8 months small team |

## Open decisions the project lead must make (blocking implementation)

Consolidated from all five tier overviews:

1. **License** — MIT OR Apache-2.0 dual (recommended) or single. [tier4_08]
2. **MSRV** — 1.78 proposed. Bump cadence? [tier4_06]
3. **Registry model** — git-URL-only (Go-style, recommended) vs self-hosted vs piggyback. [tier4_01]
4. **Module syntax** — Ruby `.` vs Rust `::` vs both. [tier1_01]
5. **Panic strategy** — abort-only (recommended v1) vs unwind vs both. [tier4_04]
6. **Editions — ship them?** Rust says yes, Go says no. [tier5_02]
7. **core vs std split** — pre-adapt for future no_std or defer. [tier1_01 / tier4_04]
8. **Async surface** — `.await` postfix only, or with `await expr` prefix too. [tier1_03]
9. **`Pin` for futures** — skip with `!Move` (recommended) or commit to `Pin`. [tier1_03]
10. **Reserved actor keywords** — deliver an actor model or un-reserve `actor`/`spawn`/`send`/`receive`. [tier1_02 / tier1_03]
11. **Variance declaration** — inference-only (recommended) vs user annotation. [tier2_07]
12. **Specialization in v1** — Rust's `min_specialization` is still unstable after a decade; ship it or defer. [tier2_04]
13. **Attribute arg grammar** — `String`-based (today) vs structured `Vec<AttrArg>`. [tier5_03]
14. **Error-code namespace** — `E0001`-style (Rust parity) or Riven-specific prefix. [tier5_04]
15. **`--explain` storage** — markdown under `docs/errors/` vs embedded in the binary. [tier5_04]
16. **Grammar formalism** — EBNF vs PEG vs ANTLR. [tier5_01]
17. **Doc-comment syntax** — `##` (already lexed) vs `///` (Rust parity). Recommended: keep `##`. [tier3_04]

## Meta observations

1. **The research surfaced 15 pre-existing issues that none of the individual tier conversations would have exposed.** Phase 0 is not overhead — it's debt repayment that unblocks everything else.
2. **Tiers are not strictly ordered.** Tier-4 CI must ship in Phase 0. Tier-5 error-code registry must ship before tier-1 diagnostic work. Tier-3 LSP wiring the existing `formatter` is a 30-line tier-3.01 win that should land in Phase 0.
3. **Three reserved-but-unused design primitives** (`async`/`await`, `actor`/`spawn`/`send`/`receive`, `edition`) hint at design intent that has not been delivered. Either commit or un-reserve.
4. **Several fixes unlock disproportionate value.** Fixing the `@[derive]` wiring (P0.3) unblocks tier-1.04 Copy/Clone, tier-1.05 derive, tier-4.05 cbindgen, and part of tier-5.03 stability attrs.
5. **Total scope is tractable.** ~6-8 months with a small team is a realistic path from current-state to a language users can write production code in — provided the open decisions above are made soon.
