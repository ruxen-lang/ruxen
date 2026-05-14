# Riven implementation prompts

Self-contained briefs for agents (or humans) executing the remaining v1
roadmap. Every prompt is TDD-discipline first: write the failing test,
implement, prove the suite is green workspace-wide, commit.

## Layout

```
docs/prompts/
├── README.md                           # this file
├── 00_universal_rules.md               # TDD, mem caps, anti-shortcuts — READ FIRST
├── v1/
│   ├── 01_phase1_remainder.md          # P0.7, T1.05 derive, T5.04 phase 3, P0.12 un-reserve
│   ├── 02_phase2_stdlib_string.md
│   ├── 03_phase2_stdlib_vec.md
│   ├── 04_phase2_stdlib_hashmap.md
│   ├── 05_phase2_stdlib_iterator.md
│   ├── 06_phase2_stdlib_io_fmt.md
│   ├── 07_phase3_const_generics.md
│   ├── 08_phase3_hrtbs_some_mixin.md
│   ├── 09_phase3_gats_any_mixin.md
│   ├── 10_phase3_lsp.md
│   ├── 11_phase3_incremental.md
│   ├── 12_phase3_diagnostics_polish.md  # T5.03 + T5.05
│   ├── 13_phase3_benchmarking.md
│   ├── 14_phase4_concurrency.md
│   ├── 15_phase4_async.md
│   ├── 16_phase4_no_std_wasm.md
│   ├── 17_phase4_cross_compile_abi.md
│   ├── 18_phase4_debugger_dwarf.md
│   ├── 19_phase5_test_framework.md
│   ├── 20_phase5_rivendoc.md
│   ├── 21_phase5_mir_opts.md
│   ├── 22_phase5_pkg_mgr_workspaces.md
│   ├── 23_phase5_language_reference.md
│   ├── 24_phase5_edition_mechanism.md
│   └── 25_v1_release_checklist.md
└── v2/
    └── 01_actors_library_then_language.md
```

## Execution order

Strict dependency chain inside v1:

```
phase 1 remainder ── phase 2 stdlib ── phase 3 types/DX ── phase 4 concurrency ── phase 4 async ── phase 4 platform ── phase 5 polish ── release
```

Each prompt declares its own `Depends on` block. Skip nothing. Do not
parallelize prompts that share the same crate's lowering or codegen
files unless the prompt explicitly green-lights it.

## How to use

1. Read `00_universal_rules.md` — non-negotiable invariants for every
   prompt.
2. Pick the next prompt by number.
3. Hand it (verbatim, no edits) to a coding agent (Claude Code,
   Cursor, etc.) or work it yourself.
4. Mark `[x]` in the prompt's `Definition of done` checklist as you
   land each item.
5. Commit per-prompt; one PR per prompt unless explicitly batched.
6. Move to next prompt only after CI is green and all DoD boxes are
   checked.

## What this is not

- Not a design document. Designs live in `docs/requirements/tier*.md`.
- Not a test plan. Each prompt's TDD section names the tests to write.
- Not a project plan. Time estimates live in `ROADMAP.md`.

## Anti-goals

- **Skipping tests.** Every prompt requires red-green-refactor. No
  feature is "done" without a failing test that now passes.
- **Mocking inputs the compiler will see in production.** No fake
  Symbol tables, no synthetic HIR — drive everything end-to-end from
  source `.rvn` fixtures where possible.
- **Aspirational stubs.** No `riven_noop_passthrough` fallbacks
  (P0.5 lesson). If a method is not implemented, the call must error
  loudly at compile time.
